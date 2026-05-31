pub mod fixture;
pub mod geometry;

use std::{cell::RefCell, rc::Rc};

use classicube_helpers::{
    entities::{ENTITY_SELF_ID, Entity},
    tick::TickEventHandler,
};
use classicube_sys::World;
use tracing::debug;

use crate::{
    chat_print,
    plugin::{
        livesplit,
        module::Module,
        splits::geometry::{CheckpointKind, SplitsState, Track, step, step_on_map_loaded},
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
        // detection works for boxes the player spawns inside. Do **not**
        // touch `fired[]` or `next_index` — a Start AABB re-arms in
        // place via `step()`, and `MapLoaded` Start re-arms via
        // `step_on_map_loaded`. Clobbering `fired[]` here would also
        // break multi-map runs whose splits live in `MapLoaded`
        // checkpoints on incidental map changes.
        self.state.borrow_mut().last_inside.fill(false);

        if let Some(name) = read_world_name() {
            let mut state = self.state.borrow_mut();
            step_on_map_loaded(&mut state, &name, livesplit::send);
        }
    }
}

/// Snapshot the engine's `World.Name` as a Rust `String`. Returns
/// `None` if the name is empty (singleplayer pre-load edge case).
fn read_world_name() -> Option<String> {
    // SAFETY: `World` is the engine's `static mut _WorldData`. We're
    // called from `on_new_map_loaded`, which fires on the game's main
    // thread after the engine has populated `Name`. `cc_string`'s
    // `Display` impl reads through the buffer pointer into a heap
    // `String`, so we don't hold a borrow past this expression.
    // `&raw const` instead of `&` avoids creating an `&static mut`
    // reference (the Rust 2024 `static_mut_refs` lint).
    let world_ptr = &raw const World;
    let name = unsafe { (*world_ptr).Name.to_string() };
    if name.is_empty() { None } else { Some(name) }
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
        "&aLiveSplit: track \"{name}\" — {fired}/{total} fired"
    ));
    if let Some(next) = next {
        chat_print(&format!("&e  next: {next}"));
    } else {
        chat_print("&e  run complete");
    }
}
