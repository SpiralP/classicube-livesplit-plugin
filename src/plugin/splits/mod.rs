pub mod fixture;
pub mod geometry;

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use classicube_helpers::{
    entities::{ENTITY_SELF_ID, Entity},
    tick::TickEventHandler,
};
use tracing::{debug, info};

use crate::{
    chat_print,
    plugin::{
        editor,
        livesplit::{self, Command, TimerEvent},
        map,
        module::Module,
        pause_triggers,
        splits::geometry::{
            Aabb, Boundary, CheckpointKind, RetypeTarget, SplitsState, Track, Trigger, observe_map,
            step, validate_pause_resume_pairing,
        },
    },
};

thread_local! {
    /// Shared with the tick closure so chat-command accessors (`load_fixture`,
    /// `reset_run`, `print_splits`) can mutate the same state without going
    /// back through `MAIN_MODULE` — which would already be borrowed mutably
    /// whenever those callbacks fire from the game thread.
    static STATE: RefCell<Option<Rc<RefCell<SplitsState>>>> = const { RefCell::new(None) };

    /// Post-load notification slot. `LssStorageModule` registers a fn
    /// here at init so it can persist newly-loaded tracks to disk
    /// without `splits::load_track` taking a direct dep on storage.
    /// Invoked after a successful `load_track` / `load_fixture` with
    /// the just-loaded track and the resolved starting map.
    static LOAD_CALLBACK: Cell<Option<LoadCallback>> = const { Cell::new(None) };
}

pub type LoadCallback = fn(&Track, Option<&str>);

pub fn set_load_callback(f: LoadCallback) {
    LOAD_CALLBACK.set(Some(f));
}

pub fn clear_load_callback() {
    LOAD_CALLBACK.set(None);
}

pub struct SplitsModule {
    state: Rc<RefCell<SplitsState>>,
    // Owned for its Drop side-effect: TickEventHandler::Drop unregisters
    // the closure from the helpers crate's TICK_CALLBACK_HANDLERS list.
    _tick: TickEventHandler,
}

impl SplitsModule {
    pub fn init() -> Self {
        let state = Rc::new(RefCell::new(SplitsState::default()));
        STATE.with_borrow_mut(|s| *s = Some(state.clone()));

        // Map-change detection is owned by `MapModule`; it fires
        // `on_map_change` on a settled-map edge before this module's tick
        // runs in the same frame (see `plugin::mod` ordering).
        map::set_splits_callback(on_map_change);

        let mut tick = TickEventHandler::new();
        {
            let state = state.clone();
            tick.on(move |_event| {
                // Local player isn't materialized until a map is loaded.
                let Some(entity) = (unsafe { Entity::from_id(ENTITY_SELF_ID) }) else {
                    return;
                };
                // Build the player's feet-anchored model collision box: X/Z
                // centered on the feet, Y from feet to feet+height, using the
                // live `Entity.Size` (already multiplied by `ModelScale`) so
                // detection matches the server's message-block walkthrough
                // collision even for custom/scaled models. Falls back to the
                // default human size when the engine hasn't populated `Size`
                // yet (e.g. before the model loads).
                let feet = entity.get_position();
                let raw_size = entity.get_inner().Size;
                let size = if raw_size.x <= 0.0 || raw_size.y <= 0.0 || raw_size.z <= 0.0 {
                    geometry::DEFAULT_PLAYER_SIZE
                } else {
                    raw_size
                };
                let player_box = geometry::player_bounds(feet, size);
                let world = map::current_map();
                let mut state = state.borrow_mut();
                // When disconnected from every timer, AABB / MapLoaded
                // triggers chat-print once and don't actually fire: we
                // snapshot run-progress state pre-step and roll it back
                // post-step so a `Start` / `Split` that nothing received
                // doesn't leave the cursor advanced. Edge state
                // (`last_inside`, `last_seen_map`) is allowed to advance
                // either way so re-entries trigger correctly when a
                // timer connects later. Pause/resume on map load stays
                // silent via `livesplit::send` directly in
                // `PauseTriggersModule` (background load-remover, not a
                // user-visible event).
                let connected = livesplit::any_connected();
                let editing = editor::is_enabled();
                // Authoring isn't a timed attempt. While edit mode is on,
                // detection still runs (so edge latches stay fresh and
                // toggling `edit off` while standing in a box doesn't
                // spuriously fire), but nothing is committed: no
                // Start/Split/Pause is sent and fired[]/next_index roll back.
                // `commit` folds both gates: a timer is listening AND we're
                // not authoring.
                let commit = connected && !editing;
                let snapshot = (!commit).then(|| (state.fired.clone(), state.next_index));
                // `any_fired` is shared between three closures
                // (`send`, `on_pause`, `on_resume`) and they need to
                // be live simultaneously while `step()` runs. A
                // `Cell<bool>` sidesteps the multi-mutable-borrow
                // conflict; `Cell` is fine in a thread-local context.
                let any_fired = Cell::new(false);
                let send = |cmd: Command| {
                    any_fired.set(true);
                    if commit {
                        livesplit::send(cmd);
                    }
                };
                // Pause/Resume kinds fire `Command::Split` via `send`
                // (advancing the LSO segment cursor) AND call the
                // pause-counter callbacks. Gate both on `commit`
                // the same way: when not committing, the snapshot rolls
                // back `fired[]`/`next_index`, so a Pause/Resume checkpoint
                // a player walked through doesn't bump the counter.
                // Edge state (`last_inside`, `last_seen_map`) still advances
                // so re-entries work after reconnect / leaving edit mode.
                let on_pause = || {
                    any_fired.set(true);
                    if commit {
                        pause_triggers::pause_add();
                    }
                };
                let on_resume = || {
                    any_fired.set(true);
                    if commit {
                        pause_triggers::pause_sub();
                    }
                };
                // Map-change detection (`observe_map` -> `step_on_map_loaded`)
                // is driven by `MapModule`'s edge tick via `on_map_change`,
                // which runs *before* this tick in the same frame (map is
                // constructed before splits), so the AABB walk below already
                // sees any `next_index` a `MapLoaded` Split / End advanced.
                step(
                    &mut state,
                    player_box,
                    world.as_deref(),
                    &send,
                    on_pause,
                    on_resume,
                );
                if any_fired.get()
                    && let Some((fired, next_index)) = snapshot
                {
                    state.fired = fired;
                    state.next_index = next_index;
                    // Silent while authoring; only warn on a genuine
                    // disconnected fire.
                    if !editing {
                        chat_print(
                            "&cLiveSplit: split fired but no timer connected (run /client \
                             LiveSplit status)",
                        );
                    }
                }
            });
        }
        Self { state, _tick: tick }
    }
}

impl Module for SplitsModule {
    fn free(&mut self) {
        // Stop receiving map-change edges before dropping our state, so the
        // map tick can't call `on_map_change` against a torn-down `STATE`.
        map::clear_splits_callback();
        // Drop the loaded track so any closure call between now and the
        // TickEventHandler::Drop a moment later is a no-op (step()
        // short-circuits on no track).
        self.state.borrow_mut().track = None;
        STATE.with_borrow_mut(|s| {
            s.take();
        });
        debug!("SplitsModule freed; track cleared, map callback cleared");
    }

    fn reset(&mut self) {
        // Drop the in-memory track on disconnect / local-map-load so the
        // next autoload or chat broadcast starts from a clean slate.
        // `with_timer_reset` notifies a connected timer if `unload()`
        // (which zeroes next_index) aborted an active run.
        with_timer_reset("", || with_state(|s| s.unload()));
    }

    fn on_new_map_loaded(&mut self) {
        // Post-teleport `last_inside[]` reset so edge-triggered AABB
        // detection works for boxes the player spawns inside. The
        // matching `step_on_map_loaded` call lives in the tick closure
        // (via `observe_map`) because at the moment this event fires
        // the engine has zeroed `World.Name` and the server hasn't yet
        // pushed the updated tab-list group, so the map name resolved
        // here would be stale on multiplayer.
        self.state.borrow_mut().last_inside.fill(false);
    }
}

/// Map-change callback registered with `MapModule`, fired on a settled-map
/// edge (see `map::observe`). Runs the connected/snapshot/rollback dance --
/// the map-load counterpart of the AABB `step` dance in the tick -- around
/// [`observe_map`]: when no timer is connected, a `MapLoaded` Start / Split
/// / End "fires" once (chat warning) but `fired[]` / `next_index` are rolled
/// back so a later-connecting timer can start the run fresh; edge state
/// (`last_seen_map`) still advances either way.
///
/// Fires *before* this module's AABB-`step` tick in the same frame (`map`
/// is constructed before `splits`), so `step` sees the cursor this already
/// advanced -- preserving the former back-to-back `observe_map` then `step`.
/// No-op when the plugin is mid-teardown (`with_state` returns `None`).
fn on_map_change(map_name: &str) {
    let editing = editor::is_enabled();
    let connected = livesplit::any_connected();
    // Same `commit` gate as the AABB tick: while editing, detection dry-runs
    // (edge latch `last_seen_map` still advances) but nothing is committed.
    let commit = connected && !editing;
    let rolled_back = with_state(|state| {
        let snapshot = (!commit).then(|| (state.fired.clone(), state.next_index));
        let any_fired = Cell::new(false);
        let send = |cmd: Command| {
            any_fired.set(true);
            if commit {
                livesplit::send(cmd);
            }
        };
        observe_map(state, Some(map_name), &send);
        if any_fired.get()
            && let Some((fired, next_index)) = snapshot
        {
            state.fired = fired;
            state.next_index = next_index;
            true
        } else {
            false
        }
    });
    if rolled_back == Some(true) && !editing {
        chat_print(
            "&cLiveSplit: split fired but no timer connected (run /client LiveSplit status)",
        );
    }

    // Leaving the track's maps drops it so stale HUD boxes / overlay don't
    // linger on an unrelated map. Edit mode is the exception: an authoring
    // session keeps the track (and stays enabled) so the user can extend it
    // onto the new map. `with_timer_reset` handles a connected timer if the
    // unload aborts a begun run; `observe_map` already reset an in-progress
    // run above, so this only fires for a *finished* run that hasn't been
    // cleared yet.
    if !editing && with_state(|s| s.is_off_track(map_name)).unwrap_or(false) {
        with_timer_reset("after leaving the track's maps", || {
            with_state(|s| s.unload());
        });
        chat_print("&eLiveSplit: left the track's maps -- track unloaded");
    }
}

fn with_state<R>(f: impl FnOnce(&mut SplitsState) -> R) -> Option<R> {
    STATE.with_borrow(|s| {
        s.as_ref().map(|state| {
            let mut state = state.borrow_mut();
            f(&mut state)
        })
    })
}

pub fn load_fixture() {
    let track = fixture::loadtest();
    if let Err(e) = validate_pause_resume_pairing(&track) {
        chat_print(&format!("&cLiveSplit: fixture track invalid: {e}"));
        return;
    }
    let n = track.checkpoints.len();
    let name = track.name.clone();
    let starting_map = map::current_map();
    info!(?starting_map, "loading fixture track:\n{track:#?}");
    if with_timer_reset("to load new track", || {
        with_state(|s| s.load(track.clone(), starting_map.clone()))
    })
    .is_none()
    {
        chat_print("&eLiveSplit: plugin not active");
        return;
    }
    if let Some(cb) = LOAD_CALLBACK.get() {
        cb(&track, starting_map.as_deref());
    }
    chat_print(&format!(
        "&aLiveSplit: loaded track \"{name}\" ({n} checkpoints)"
    ));
}

/// Load a track from any non-fixture source. `source` is a short
/// human-readable label appended to the success chat-print (e.g.
/// `"chat"`, `"disk"`). Returns `false` only when the plugin is
/// mid-teardown (`STATE` is `None`); the caller (track_source receiver
/// / lss-storage autoload) treats `false` as "don't suppress the
/// source chat line — plugin isn't active to handle it."
pub fn load_track(track: Track, source: &str) -> bool {
    if let Err(e) = validate_pause_resume_pairing(&track) {
        // Mirror the chat-decoder error style. Return `false` so the
        // chat-protocol receiver doesn't suppress the source line —
        // failure to load = don't swallow the trigger.
        chat_print(&format!("&cLiveSplit: refusing to load track: {e}"));
        return false;
    }
    let n = track.checkpoints.len();
    let name = track.name.clone();
    let starting_map = map::current_map();
    info!(?starting_map, source, "loading track:\n{track:#?}");
    if with_timer_reset("to load new track", || {
        with_state(|s| s.load(track.clone(), starting_map.clone()))
    })
    .is_none()
    {
        return false;
    }
    if let Some(cb) = LOAD_CALLBACK.get() {
        cb(&track, starting_map.as_deref());
    }
    chat_print(&format!(
        "&aLiveSplit: loaded track \"{name}\" ({n} checkpoints) from {source}"
    ));
    true
}

/// Create and load a fresh empty track named `name` (editor `edit new`).
/// Scoped to the current world (`map::current_map()`), cursor re-armed to 0.
/// Fires `LOAD_CALLBACK` like the other load paths -- the lss autosave
/// short-circuits on the empty track (see `on_track_loaded`). Returns
/// `false` only if the plugin is mid-teardown.
pub fn new_track(name: String) -> bool {
    let track = Track {
        name: name.clone(),
        checkpoints: Vec::new(),
    };
    // An empty track trivially passes pause/resume pairing; no validate call.
    let starting_map = map::current_map();
    info!(?starting_map, "creating new empty track \"{name}\"");
    if with_timer_reset("to load new track", || {
        with_state(|s| s.load(track.clone(), starting_map.clone()))
    })
    .is_none()
    {
        chat_print("&eLiveSplit: plugin not active");
        return false;
    }
    if let Some(cb) = LOAD_CALLBACK.get() {
        cb(&track, starting_map.as_deref());
    }
    chat_print(&format!(
        "&aLiveSplit: new track \"{name}\" -- place Start then splits with /client LiveSplit edit \
         add"
    ));
    true
}

/// Snapshot the currently-loaded `Track` for the chat-encode debug
/// command. `None` if no track is loaded or the plugin is mid-teardown.
pub fn current_track() -> Option<Track> {
    with_state(|s| s.track.clone()).flatten()
}

/// Kind, label, and map-transition flag of every non-Start checkpoint in the
/// loaded track, in checkpoint order -- i.e. the rows that will each become a
/// `Command::Split`. The `bool` is `true` for a `Trigger::MapLoaded`
/// checkpoint (used by the timer overlay to color map-transition rows purple).
/// Mirrors what the built-in timer's `do_start` populates `split_rows` from,
/// so the overlay can show the same list (with blank times) before a run
/// begins. Empty when no track is loaded, the track has only a Start (or is
/// empty), or the plugin is mid-teardown.
pub fn checkpoint_rows() -> Vec<(CheckpointKind, String, bool)> {
    with_state(|s| {
        s.track.as_ref().map_or_else(Vec::new, |t| {
            t.checkpoints
                .get(1..)
                .unwrap_or_default()
                .iter()
                .map(|cp| {
                    (
                        cp.kind,
                        cp.label.clone(),
                        matches!(cp.trigger, Trigger::MapLoaded(_)),
                    )
                })
                .collect()
        })
    })
    .unwrap_or_default()
}

/// AABB checkpoints visible on the player's current map, paired with
/// their kind, label, and an "is the next eligible checkpoint" flag, in
/// checkpoint order -- the boxes the HUD should draw, the text it floats
/// above them, and which one to highlight as the run's next target.
/// Resolves the live map name via `map::current_map()` and walks the
/// loaded track's implicit scope (see `geometry::aabbs_on_map`). Empty
/// when no track is loaded, the plugin is mid-teardown, or the current
/// map can't be resolved.
///
/// The next-flag is keyed off `SplitsState.next_index`, so it highlights
/// from the moment a track loads: pre-run (`next_index == 0`) the Start
/// checkpoint is flagged; post-run (`next_index == n`) nothing matches.
pub fn visible_aabbs() -> Vec<(usize, CheckpointKind, Aabb, String, bool)> {
    // Resolve the map name outside `with_state`: `map::current_map()`
    // reads the engine `World` static + tab-list, never `STATE`, so
    // keeping it out of the closure avoids nesting a borrow.
    let world = map::current_map();
    with_state(|s| {
        let next_index = Some(s.next_index);
        s.track.as_ref().map_or_else(Vec::new, |t| {
            geometry::aabbs_on_map(t, s.starting_map.as_deref(), world.as_deref(), next_index)
        })
    })
    .unwrap_or_default()
}

/// Cheap "is a track loaded?" probe for callers that just want to gate
/// behavior (e.g. `PauseTriggersModule` skipping pause/resume when
/// there's nothing to time). Returns `false` if the plugin is
/// mid-teardown.
pub fn track_loaded() -> bool {
    with_state(|s| s.track.is_some()).unwrap_or(false)
}

/// Is a run mid-flight? True iff `next_index > 0` AND a final
/// checkpoint hasn't fired yet. Used by `LssStorageModule` to gate
/// auto-load on `on_new_map_loaded` (don't clobber an in-progress
/// run with a track from the destination map's directory).
pub fn run_in_progress() -> bool {
    with_state(|s| s.next_index > 0 && s.next_index < s.fired.len()).unwrap_or(false)
}

/// Has the run cursor advanced past Start -- i.e. a run is mid-flight
/// *or* already finished (`next_index > 0`)? Broader than
/// [`run_in_progress`], which excludes a finished run. Used by
/// [`with_timer_reset`] so loading a new track / editing after
/// *finishing* a run still resets connected timers (the built-in overlay
/// and any external LiveSplit), instead of leaving the completed run on
/// screen until the next Start. `false` when mid-teardown.
fn run_begun() -> bool {
    with_state(|s| s.next_index > 0).unwrap_or(false)
}

/// Whether `map_name` belongs to the currently-loaded track's map set
/// (`starting_map` or any `Trigger::MapLoaded` target). `false` when no
/// track is loaded or mid-teardown. Used by the autoload tick to avoid
/// clobbering an open editor's multi-map track when the player crosses
/// into another of its maps.
pub fn track_includes_map(map_name: &str) -> bool {
    with_state(|s| s.includes_map(map_name)).unwrap_or(false)
}

/// Snapshot the live map name. Thin re-export of [`map::current_map`] kept
/// for the existing `splits::current_map()` callers (editor `set_kind_map`,
/// the lss-storage load command). Distinct from [`starting_map`] (the world
/// the loaded track is scoped to, which may differ after a map change).
pub fn current_map() -> Option<String> {
    map::current_map()
}

/// The map the loaded track is scoped to -- the world name captured at
/// `load()` time. Distinct from [`current_map`] (the live world, which
/// may differ after a map change). `None` if no track is loaded or the
/// plugin is mid-teardown. Used by `/client LiveSplit save` to file the
/// track under its scope map.
pub fn starting_map() -> Option<String> {
    with_state(|s| s.starting_map.clone()).flatten()
}

pub fn reset_run() {
    with_state(SplitsState::rearm);
}

/// Apply a timer-originated event (from the LSO read loop, hopped to the
/// main thread). `Reset` re-arms; `SplitUndone` walks the cursor back one.
/// Forward auto-events never reach here (filtered in `TimerEvent::from_wire`).
///
/// This funnels UI button / hotkey / chat `undosplit` through one path: the
/// chat `undosplit` arm only sends `Command::UndoSplit` and leaves the cursor
/// alone, so the echoed `SplitUndone` drives the walk-back for free. Degrades
/// to a no-op if the plugin is mid-teardown (`with_state` returns `None`).
pub fn on_timer_event(ev: TimerEvent) {
    match ev {
        TimerEvent::Reset => reset_run(),
        TimerEvent::SplitUndone => {
            let walked = with_state(|s| {
                let before = s.next_index;
                s.undo_one();
                (before, s.next_index)
            });
            if let Some((before, after)) = walked
                && before != after
            {
                chat_print(&format!(
                    "&eLiveSplit: timer undid split; cursor #{before} -> #{after}"
                ));
            }
        }
    }
}

/// Run a mutation that may re-arm the run cursor (a fresh `load_track`,
/// or an editor edit), then keep a connected timer in sync: if the
/// mutation aborted or replaced a run that had begun, reset the timer too.
///
/// `reason` is appended to the chat notification: non-empty produces
/// `"run reset {reason}"`, empty produces `"run reset"`.
///
/// Brackets the sample-before / notify-after dance into one call so it
/// can't be split or mis-ordered. Detection is purely state-based: every
/// re-arming mutator zeroes the cursor on success and leaves it untouched
/// on failure (each bails before mutating, and `move_checkpoint` rolls
/// back), so "had begun, now re-armed" == "a run got aborted or replaced"
/// -- failures and no-ops fall through without a reset. Keyed on
/// [`run_begun`] (`next_index > 0`), **not** [`run_in_progress`], so a
/// *finished* run is reset too: loading a new track after completing a run
/// clears the old splits off connected timers instead of stranding them
/// there until the next Start. Silent + no-op when nothing had begun or no
/// timer is attached (the plugin is usable offline). Returns the
/// mutation's own result unchanged.
pub fn with_timer_reset<R>(reason: &str, mutate: impl FnOnce() -> R) -> R {
    let was_begun = run_begun();
    let out = mutate();
    if was_begun && !run_begun() && livesplit::any_connected() {
        livesplit::send(Command::Reset { save_attempt: None });
        if reason.is_empty() {
            chat_print("&eLiveSplit: run reset");
        } else {
            chat_print(&format!("&eLiveSplit: run reset {reason}"));
        }
    }
    out
}

/// Add an editor-placed AABB checkpoint, returning the index it
/// landed at. The mutation runs inside [`with_timer_reset`], so an
/// aborted in-progress run resets a connected timer. Chat-prints the
/// outcome. `None` if the plugin is mid-teardown or the mutation failed.
///
/// A `None` `target` (bare `edit add`) resolves to the end of the
/// player's current map section via `geometry::append_index_for_section`:
/// just before that section's terminating `MapLoaded` (or before a `Pause`
/// sitting directly before it), or **after `End`** on the last/only section
/// (so the new checkpoint becomes the new finish). An explicit `Some(i)` is
/// passed through verbatim for `add_checkpoint` to clamp.
pub fn editor_add(aabb: Aabb, label: String, target: Option<usize>) -> Option<usize> {
    // Resolve the live world outside the borrow: `map::current_map()` reads
    // the engine `World` static + tab-list, never `STATE` (same reason
    // `visible_aabbs()` resolves it first).
    let world = map::current_map();
    match with_timer_reset("to allow edit", || {
        with_state(|s| {
            let resolved = target.or_else(|| {
                s.track.as_ref().map(|t| {
                    geometry::append_index_for_section(
                        &t.checkpoints,
                        s.starting_map.as_deref(),
                        world.as_deref(),
                    )
                })
            });
            s.add_checkpoint(aabb, label, resolved)
        })
    }) {
        None => {
            chat_print("&eLiveSplit: plugin not active");
            None
        }
        Some(Err(e)) => {
            chat_print(&format!("&cLiveSplit: cannot add checkpoint: {e}"));
            None
        }
        Some(Ok(idx)) => {
            chat_print(&format!("&aLiveSplit: added checkpoint #{idx}"));
            Some(idx)
        }
    }
}

/// Remove the checkpoint at `i`. Like [`editor_add`], runs inside
/// [`with_timer_reset`] so a connected timer resets if the edit aborted
/// a run. Returns `true` on success.
pub fn editor_remove(i: usize) -> bool {
    match with_timer_reset("to allow edit", || with_state(|s| s.remove_checkpoint(i))) {
        None => {
            chat_print("&eLiveSplit: plugin not active");
            false
        }
        Some(Err(e)) => {
            chat_print(&format!("&cLiveSplit: cannot remove checkpoint: {e}"));
            false
        }
        Some(Ok(())) => {
            chat_print(&format!("&aLiveSplit: removed checkpoint #{i}"));
            true
        }
    }
}

/// Reorder: move the checkpoint at `from` to index `to`. Like
/// [`editor_add`], runs inside [`with_timer_reset`] so a connected timer
/// resets if the edit aborted a run. `from == to` is a friendly no-op
/// (the mutator is pure remove+insert, so the guard lives here). Returns
/// `true` on success.
pub fn editor_reindex(from: usize, to: usize) -> bool {
    if from == to {
        chat_print(&format!(
            "&eLiveSplit: checkpoint #{from} is already at index #{to}"
        ));
        return false;
    }
    match with_timer_reset("to allow edit", || {
        with_state(|s| s.move_checkpoint(from, to))
    }) {
        None => {
            chat_print("&eLiveSplit: plugin not active");
            false
        }
        Some(Err(e)) => {
            chat_print(&format!("&cLiveSplit: cannot move checkpoint: {e}"));
            false
        }
        Some(Ok(())) => {
            chat_print(&format!("&aLiveSplit: moved checkpoint #{from} to #{to}"));
            true
        }
    }
}

/// Relabel the checkpoint at `i`. Non-structural, so it never re-arms
/// the run or touches the timer. Returns `true` on success.
pub fn editor_set_label(i: usize, text: String) -> bool {
    match with_state(|s| s.set_label(i, text)) {
        None => {
            chat_print("&eLiveSplit: plugin not active");
            false
        }
        Some(Err(e)) => {
            chat_print(&format!("&cLiveSplit: cannot set label: {e}"));
            false
        }
        Some(Ok(())) => {
            chat_print(&format!("&aLiveSplit: set label of checkpoint #{i}"));
            true
        }
    }
}

/// Rename the loaded track (`edit rename`). Non-structural, so it never
/// re-arms the run or touches the timer. Returns `true` on success.
pub fn editor_rename(name: String) -> bool {
    match with_state(|s| s.set_name(name.clone())) {
        None => {
            chat_print("&eLiveSplit: plugin not active");
            false
        }
        Some(Err(e)) => {
            chat_print(&format!("&cLiveSplit: cannot rename track: {e}"));
            false
        }
        Some(Ok(())) => {
            chat_print(&format!("&aLiveSplit: renamed track to \"{name}\""));
            true
        }
    }
}

/// Re-draw the AABB of the existing checkpoint at `i` (`edit redraw`).
/// Like [`editor_add`], runs inside [`with_timer_reset`] so a connected
/// timer resets if the edit aborted a run. Returns `true` on success.
pub fn editor_relocate(i: usize, aabb: Aabb) -> bool {
    match with_timer_reset("to allow edit", || with_state(|s| s.set_trigger(i, aabb))) {
        None => {
            chat_print("&eLiveSplit: plugin not active");
            false
        }
        Some(Err(e)) => {
            chat_print(&format!("&cLiveSplit: cannot redraw checkpoint: {e}"));
            false
        }
        Some(Ok(())) => {
            chat_print(&format!("&aLiveSplit: redrew checkpoint #{i}"));
            true
        }
    }
}

/// Retype the checkpoint at `i` (`edit kind <i> ...`). Like
/// [`editor_relocate`], runs inside [`with_timer_reset`] so a connected
/// timer resets if the edit aborted a run. Pairing isn't validated here
/// -- the mutator defers it to the save/load gates (see
/// [`geometry::SplitsState::set_kind`]). Returns `true` on success.
pub fn editor_set_kind(i: usize, target: RetypeTarget) -> bool {
    match with_timer_reset("to allow edit", || with_state(|s| s.set_kind(i, target))) {
        None => {
            chat_print("&eLiveSplit: plugin not active");
            false
        }
        Some(Err(e)) => {
            chat_print(&format!("&cLiveSplit: cannot retype checkpoint: {e}"));
            false
        }
        Some(Ok(())) => {
            chat_print(&format!("&aLiveSplit: retyped checkpoint #{i}"));
            true
        }
    }
}

/// Move the checkpoint at `i` into the `Start` / `End` boundary slot
/// (`edit kind <i> start|end`). Delegates to
/// [`geometry::SplitsState::move_to_boundary`], which re-derives boundary
/// kinds, demotes the displaced former boundary to `Split`, validates
/// pause/resume pairing (rolling back on inversion), reallocates the
/// latches, and re-arms the run. Like [`editor_reindex`], runs inside
/// [`with_timer_reset`] so a connected timer resets if the edit aborted a
/// run. `Ok(false)` (already the boundary) is a friendly no-op -- it
/// leaves the cursor untouched, so `with_timer_reset` doesn't fire.
/// Returns `true` when a real move happened.
pub fn editor_set_boundary(i: usize, which: Boundary) -> bool {
    let name = match which {
        Boundary::Start => "Start",
        Boundary::End => "End",
    };
    match with_timer_reset("to allow edit", || {
        with_state(|s| s.move_to_boundary(i, which))
    }) {
        None => {
            chat_print("&eLiveSplit: plugin not active");
            false
        }
        Some(Err(e)) => {
            chat_print(&format!("&cLiveSplit: cannot retype checkpoint: {e}"));
            false
        }
        Some(Ok(false)) => {
            chat_print(&format!(
                "&eLiveSplit: checkpoint #{i} is already the {name}"
            ));
            false
        }
        Some(Ok(true)) => {
            chat_print(&format!("&aLiveSplit: made checkpoint #{i} the {name}"));
            true
        }
    }
}

pub fn clear_track() {
    let outcome = with_state(|s| {
        let had_track = s.track.is_some();
        s.unload();
        had_track
    });
    match outcome {
        Some(true) => chat_print("&aLiveSplit: track cleared"),
        Some(false) => chat_print("&eLiveSplit: no track loaded"),
        None => chat_print("&eLiveSplit: plugin not active"),
    }
}

/// Chat-print the full checkpoint list for `/client LiveSplit splits`: a
/// header line plus one line per checkpoint (see
/// [`geometry::format_splits`]). `suppress_next` hides the next-target
/// `&e> ... &e<` marker -- passed `true` in edit mode, where there's no live
/// run to target.
pub fn print_splits(suppress_next: bool) {
    let Some(lines) = with_state(|s| {
        let next = (!suppress_next).then_some(s.next_index);
        s.track
            .as_ref()
            .map(|t| geometry::format_splits(t, &s.fired, next, true))
    }) else {
        chat_print("&eLiveSplit: plugin not active");
        return;
    };
    let Some(lines) = lines else {
        chat_print("&eLiveSplit: no track loaded (try /client LiveSplit loadtest)");
        return;
    };
    for line in &lines {
        chat_print(line);
    }
}
