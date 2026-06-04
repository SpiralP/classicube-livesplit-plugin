pub mod fixture;
pub mod geometry;

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use classicube_helpers::{
    entities::{ENTITY_SELF_ID, Entity},
    tab_list::TabListEntry,
    tick::TickEventHandler,
};
use classicube_sys::World;
use tracing::{debug, info};

use crate::{
    chat_print,
    plugin::{
        livesplit::{self, Command, TimerEvent},
        module::Module,
        pause_triggers,
        splits::geometry::{
            Aabb, Boundary, CheckpointKind, RetypeTarget, SplitsState, Track, observe_map, step,
            validate_pause_resume_pairing,
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
                let world = read_world_name();
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
                let snapshot = (!connected).then(|| (state.fired.clone(), state.next_index));
                // `any_fired` is shared between three closures
                // (`send`, `on_pause`, `on_resume`) and they need to
                // be live simultaneously while `step()` runs. A
                // `Cell<bool>` sidesteps the multi-mutable-borrow
                // conflict; `Cell` is fine in a thread-local context.
                let any_fired = Cell::new(false);
                let send = |cmd: Command| {
                    any_fired.set(true);
                    if connected {
                        livesplit::send(cmd);
                    }
                };
                // Pause/Resume kinds fire `Command::Split` via `send`
                // (advancing the LSO segment cursor) AND call the
                // pause-counter callbacks. Gate both on `connected`
                // the same way: when disconnected, the disconnect
                // snapshot rolls back `fired[]`/`next_index`, so a
                // Pause/Resume checkpoint a player walked through
                // without a timer attached doesn't bump the counter
                // either. Edge state (`last_inside`, `last_seen_map`)
                // still advances so re-entries work after reconnect.
                let on_pause = || {
                    any_fired.set(true);
                    if connected {
                        pause_triggers::pause_add();
                    }
                };
                let on_resume = || {
                    any_fired.set(true);
                    if connected {
                        pause_triggers::pause_sub();
                    }
                };
                // Map-change detection runs before the AABB walk so a
                // `MapLoaded` Split / End advances `next_index` first;
                // `step` then sees the updated cursor for the same tick.
                observe_map(&mut state, world.as_deref(), &send);
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
                    drop(state);
                    chat_print(
                        "&cLiveSplit: split fired but no timer connected (run /client LiveSplit \
                         status)",
                    );
                }
            });
        }
        Self { state, _tick: tick }
    }
}

impl Module for SplitsModule {
    fn free(&mut self) {
        // Drop the loaded track so any closure call between now and the
        // TickEventHandler::Drop a moment later is a no-op (step()
        // short-circuits on no track).
        self.state.borrow_mut().track = None;
        STATE.with_borrow_mut(|s| {
            s.take();
        });
        debug!("SplitsModule freed; track cleared");
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

/// Snapshot the current map name. In singleplayer / file-loaded worlds
/// the engine populates `World.Name` directly. The classic / CPE
/// network protocol carries no map-name packet, so on multiplayer
/// `World.Name` is always empty; MCGalaxy and compatible servers
/// instead put `"On <mapname>"` in the local player's tab-list group
/// (it's the section header the tab UI groups players by). Read that
/// and strip the prefix.
fn read_world_name() -> Option<String> {
    // SAFETY: `World` is the engine's `static mut _WorldData`. We're
    // called from `on_new_map_loaded` / the tick callback on the main
    // thread. `cc_string`'s `Display` impl copies through the buffer
    // pointer into an owned `String`. `&raw const` avoids creating an
    // `&'static mut` (the Rust 2024 `static_mut_refs` lint).
    let world_ptr = &raw const World;
    let name = unsafe { (*world_ptr).Name.to_string() };
    if !name.is_empty() {
        return Some(name);
    }

    let entry = unsafe { TabListEntry::from_id(ENTITY_SELF_ID) }?;
    let group = entry.get_group();
    let map = group.strip_prefix("On ")?.trim();
    if map.is_empty() {
        None
    } else {
        Some(map.to_owned())
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
    let starting_map = read_world_name();
    info!(?starting_map, "loading fixture track:\n{track:#?}");
    if with_state(|s| s.load(track.clone(), starting_map.clone())).is_none() {
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
    let starting_map = read_world_name();
    info!(?starting_map, source, "loading track:\n{track:#?}");
    if with_state(|s| s.load(track.clone(), starting_map.clone())).is_none() {
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

/// Snapshot the currently-loaded `Track` for the chat-encode debug
/// command. `None` if no track is loaded or the plugin is mid-teardown.
pub fn current_track() -> Option<Track> {
    with_state(|s| s.track.clone()).flatten()
}

/// AABB checkpoints visible on the player's current map, paired with
/// their kind, label, and an "is the next eligible checkpoint" flag, in
/// checkpoint order -- the boxes the HUD should draw, the text it floats
/// above them, and which one to highlight as the run's next target.
/// Resolves the live map name via `read_world_name()` and walks the
/// loaded track's implicit scope (see `geometry::aabbs_on_map`). Empty
/// when no track is loaded, the plugin is mid-teardown, or the current
/// map can't be resolved.
///
/// The next-flag is keyed off `SplitsState.next_index`, so it highlights
/// from the moment a track loads: pre-run (`next_index == 0`) the Start
/// checkpoint is flagged; post-run (`next_index == n`) nothing matches.
pub fn visible_aabbs() -> Vec<(usize, CheckpointKind, Aabb, String, bool)> {
    // Resolve the map name outside `with_state`: `read_world_name()`
    // reads the engine `World` static + tab-list, never `STATE`, so
    // keeping it out of the closure avoids nesting a borrow.
    let world = read_world_name();
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

/// Snapshot the current map name (engine `World.Name` in singleplayer
/// / file-loaded, tab-list group prefix on multiplayer). Returns
/// `None` if neither source resolves a non-empty name.
pub fn current_map() -> Option<String> {
    read_world_name()
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

/// Tell a connected timer to reset after a load / structural edit
/// aborted an in-progress run. The caller already re-armed local state
/// (a fresh `load_track`, or an editor mutation); this just keeps the
/// timer in sync. Silent + no-op when nothing was running or no timer
/// is attached (the plugin is usable offline).
pub fn reset_timer_if_was_running(was_in_progress: bool) {
    if was_in_progress && livesplit::any_connected() {
        livesplit::send(Command::Reset { save_attempt: None });
        chat_print("&eLiveSplit: run reset to allow edit");
    }
}

/// Add an editor-placed AABB checkpoint, returning the index it
/// landed at. Samples `run_in_progress()` **before** the mutation (which
/// re-arms the cursor to 0) so an aborted run can notify the timer.
/// Chat-prints the outcome. `None` if the plugin is mid-teardown or the
/// mutation failed.
///
/// A `None` `target` (bare `edit add`) resolves to the end of the
/// player's current map section via `geometry::append_index_for_section`
/// (just before that section's terminating `MapLoaded`, or before `End` on
/// the last/only section). An explicit `Some(i)` is passed through verbatim
/// for `add_checkpoint` to clamp.
pub fn editor_add(aabb: Aabb, label: String, target: Option<usize>) -> Option<usize> {
    let was_in_progress = run_in_progress();
    // Resolve the live world outside the borrow: `read_world_name()` reads
    // the engine `World` static + tab-list, never `STATE` (same reason
    // `visible_aabbs()` resolves it first).
    let world = read_world_name();
    match with_state(|s| {
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
            reset_timer_if_was_running(was_in_progress);
            chat_print(&format!("&aLiveSplit: added checkpoint #{idx}"));
            Some(idx)
        }
    }
}

/// Remove the checkpoint at `i`. Like [`editor_add`], samples
/// run-progress before mutating and notifies a connected timer if it
/// aborted a run. Returns `true` on success.
pub fn editor_remove(i: usize) -> bool {
    let was_in_progress = run_in_progress();
    match with_state(|s| s.remove_checkpoint(i)) {
        None => {
            chat_print("&eLiveSplit: plugin not active");
            false
        }
        Some(Err(e)) => {
            chat_print(&format!("&cLiveSplit: cannot remove checkpoint: {e}"));
            false
        }
        Some(Ok(())) => {
            reset_timer_if_was_running(was_in_progress);
            chat_print(&format!("&aLiveSplit: removed checkpoint #{i}"));
            true
        }
    }
}

/// Reorder: move the checkpoint at `from` to index `to`. Like
/// [`editor_add`], samples run-progress before mutating and notifies a
/// connected timer if it aborted a run. `from == to` is a friendly no-op
/// (the mutator is pure remove+insert, so the guard lives here). Returns
/// `true` on success.
pub fn editor_reindex(from: usize, to: usize) -> bool {
    if from == to {
        chat_print(&format!(
            "&eLiveSplit: checkpoint #{from} is already at index #{to}"
        ));
        return false;
    }
    let was_in_progress = run_in_progress();
    match with_state(|s| s.move_checkpoint(from, to)) {
        None => {
            chat_print("&eLiveSplit: plugin not active");
            false
        }
        Some(Err(e)) => {
            chat_print(&format!("&cLiveSplit: cannot move checkpoint: {e}"));
            false
        }
        Some(Ok(())) => {
            reset_timer_if_was_running(was_in_progress);
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

/// Re-draw the AABB of the existing checkpoint at `i` (`edit redraw`).
/// Like [`editor_add`], samples run-progress before mutating and
/// notifies a connected timer if it aborted a run. Returns `true` on
/// success.
pub fn editor_relocate(i: usize, aabb: Aabb) -> bool {
    let was_in_progress = run_in_progress();
    match with_state(|s| s.set_trigger(i, aabb)) {
        None => {
            chat_print("&eLiveSplit: plugin not active");
            false
        }
        Some(Err(e)) => {
            chat_print(&format!("&cLiveSplit: cannot redraw checkpoint: {e}"));
            false
        }
        Some(Ok(())) => {
            reset_timer_if_was_running(was_in_progress);
            chat_print(&format!("&aLiveSplit: redrew checkpoint #{i}"));
            true
        }
    }
}

/// Retype the checkpoint at `i` (`edit kind <i> ...`). Like
/// [`editor_relocate`], samples run-progress before the mutation (which
/// re-arms the cursor) and notifies a connected timer if it aborted a
/// run. Pairing isn't validated here -- the mutator defers it to the
/// save/load gates (see [`geometry::SplitsState::set_kind`]). Returns
/// `true` on success.
pub fn editor_set_kind(i: usize, target: RetypeTarget) -> bool {
    let was_in_progress = run_in_progress();
    match with_state(|s| s.set_kind(i, target)) {
        None => {
            chat_print("&eLiveSplit: plugin not active");
            false
        }
        Some(Err(e)) => {
            chat_print(&format!("&cLiveSplit: cannot retype checkpoint: {e}"));
            false
        }
        Some(Ok(())) => {
            reset_timer_if_was_running(was_in_progress);
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
/// latches, and re-arms the run. Like [`editor_reindex`], samples
/// run-progress before mutating and notifies a connected timer if it
/// aborted a run. `Ok(false)` (already the boundary) is a friendly no-op.
/// Returns `true` when a real move happened.
pub fn editor_set_boundary(i: usize, which: Boundary) -> bool {
    let was_in_progress = run_in_progress();
    let name = match which {
        Boundary::Start => "Start",
        Boundary::End => "End",
    };
    match with_state(|s| s.move_to_boundary(i, which)) {
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
            reset_timer_if_was_running(was_in_progress);
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
/// [`geometry::format_splits`]).
pub fn print_splits() {
    let Some(lines) = with_state(|s| {
        s.track
            .as_ref()
            .map(|t| geometry::format_splits(t, &s.fired, s.next_index))
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
