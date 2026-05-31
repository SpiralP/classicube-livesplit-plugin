pub mod fixture;
pub mod geometry;

use std::{cell::RefCell, rc::Rc};

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
        livesplit,
        module::Module,
        splits::geometry::{CheckpointKind, SplitsState, Track, observe_map, step},
    },
};

thread_local! {
    /// Shared with the tick closure so chat-command accessors (`load_fixture`,
    /// `reset_run`, `print_status`) can mutate the same state without going
    /// back through `MAIN_MODULE` — which would already be borrowed mutably
    /// whenever those callbacks fire from the game thread.
    static STATE: RefCell<Option<Rc<RefCell<SplitsState>>>> = const { RefCell::new(None) };
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
                let pos = entity.get_position();
                let world = read_world_name();
                let mut state = state.borrow_mut();
                // Map-change detection runs before the AABB walk so a
                // `MapLoaded` Split / End advances `next_index` first;
                // `step` then sees the updated cursor for the same tick.
                observe_map(&mut state, world.as_deref(), livesplit::send);
                step(&mut state, pos, world.as_deref(), livesplit::send);
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
    let n = track.checkpoints.len();
    let name = track.name.clone();
    let starting_map = read_world_name();
    info!(?starting_map, "loading fixture track:\n{track:#?}");
    if with_state(|s| s.load(track, starting_map)).is_none() {
        chat_print("&eLiveSplit: plugin not active");
        return;
    }
    chat_print(&format!(
        "&aLiveSplit: loaded track \"{name}\" ({n} checkpoints)"
    ));
}

/// Load a track received over the chat protocol. Returns `false` only
/// when the plugin is mid-teardown (`STATE` is `None`); the caller
/// (track_source receiver) treats `false` as "don't suppress the
/// source chat line — plugin isn't active to handle it."
pub fn load_track(track: Track) -> bool {
    let n = track.checkpoints.len();
    let name = track.name.clone();
    let starting_map = read_world_name();
    info!(?starting_map, "loading track from chat:\n{track:#?}");
    if with_state(|s| s.load(track, starting_map)).is_none() {
        return false;
    }
    chat_print(&format!(
        "&aLiveSplit: loaded track \"{name}\" ({n} checkpoints) from chat"
    ));
    true
}

/// Snapshot the currently-loaded `Track` for the chat-encode debug
/// command. `None` if no track is loaded or the plugin is mid-teardown.
pub fn current_track() -> Option<Track> {
    with_state(|s| s.track.clone()).flatten()
}

/// Cheap "is a track loaded?" probe for callers that just want to gate
/// behavior (e.g. `PauseTriggersModule` skipping pause/resume when
/// there's nothing to time). Returns `false` if the plugin is
/// mid-teardown.
pub fn track_loaded() -> bool {
    with_state(|s| s.track.is_some()).unwrap_or(false)
}

pub fn reset_run() {
    with_state(SplitsState::rearm);
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

pub fn print_status() {
    let Some(snapshot) = with_state(|s| {
        s.track.as_ref().map(|t| {
            let total = t.checkpoints.len();
            let name = t.name.clone();
            let fired = s.fired.iter().filter(|b| **b).count();
            let next = t.checkpoints.get(s.next_index).map(|cp| {
                let kind = match cp.kind {
                    CheckpointKind::Start => "Start",
                    CheckpointKind::Split => "Split",
                    CheckpointKind::End => "End",
                };
                let label = cp.label.as_str();
                format!("{kind} \"{label}\"")
            });
            (name, total, fired, next)
        })
    }) else {
        chat_print("&eLiveSplit: plugin not active");
        return;
    };
    let Some((name, total, fired, next)) = snapshot else {
        chat_print("&eLiveSplit: no track loaded (try /client LiveSplit loadtest)");
        return;
    };
    chat_print(&format!(
        "&aLiveSplit: track \"{name}\" - {fired}/{total} fired"
    ));
    if let Some(next) = next {
        chat_print(&format!("&e  next: {next}"));
    } else {
        chat_print("&e  run complete");
    }
}
