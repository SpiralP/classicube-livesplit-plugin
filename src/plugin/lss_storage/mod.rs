pub mod path;
pub mod payload;
pub mod read;
pub mod write;

use classicube_helpers::{async_manager, tick::TickEventHandler};
use classicube_sys::Server;
use tracing::debug;

use crate::{
    chat_print,
    plugin::{
        module::Module,
        splits::{self, geometry::Track},
    },
};

pub struct LssStorageModule {
    // Owned for its Drop side-effect: TickEventHandler::Drop unregisters
    // the closure from the helpers crate's tick callback list.
    _tick: TickEventHandler,
}

impl LssStorageModule {
    pub fn init() -> Self {
        splits::set_load_callback(on_track_loaded);

        // Autoload is driven from the tick rather than `on_new_map_loaded`
        // for the same reason `splits::observe_map` is: at the moment
        // `MapLoaded` fires on multiplayer, `World.Name` has been zeroed
        // by `World_SetNewMap` and the server hasn't yet pushed the
        // updated `"On <map>"` tab-list group, so the name resolved at
        // the event would be the *previous* map's. Polling until both
        // signals settle gives us the correct destination directory.
        let mut tick = TickEventHandler::new();
        let mut last_seen_map: Option<String> = None;
        tick.on(move |_event| {
            let cur = splits::current_map();
            if last_seen_map == cur {
                return;
            }
            last_seen_map.clone_from(&cur);
            let Some(map) = cur else {
                return;
            };
            if splits::run_in_progress() {
                debug!("autoload skipped: run in progress");
                return;
            }
            let Some(server) = current_server_display() else {
                return;
            };
            async_manager::spawn(read::try_autoload(server, map));
        });

        Self { _tick: tick }
    }
}

impl Module for LssStorageModule {
    fn free(&mut self) {
        splits::clear_load_callback();
        debug!("LssStorageModule freed; load callback cleared");
    }
}

fn on_track_loaded(track: &Track, starting_map: Option<&str>) {
    let Some(server) = current_server_display() else {
        debug!("save skipped: no server name");
        return;
    };
    let Some(map) = starting_map else {
        debug!("save skipped: no starting map");
        return;
    };
    let track = track.clone();
    let map = map.to_owned();
    // Background autosave: silent on the no-op (dedup) case.
    async_manager::spawn(write::save_track(track, server, map, false));
}

/// Write the currently loaded track to disk on demand
/// (`/client LiveSplit save`), reusing the autosave writer. Files the
/// track under `(server, starting_map)` -- the map the track is scoped
/// to, not the live world. Announces the no-op case so a manual save
/// with no changes is clean feedback rather than silence. Not gated on
/// `require_connected()` -- file I/O is local.
pub fn save_current_track() {
    let Some(track) = splits::current_track() else {
        chat_print("&cLiveSplit: no track loaded to save");
        return;
    };
    let Some(server) = current_server_display() else {
        chat_print("&cLiveSplit: cannot resolve server name to save under");
        return;
    };
    let Some(map) = splits::starting_map() else {
        chat_print("&cLiveSplit: no starting map for this track (load it on a map first)");
        return;
    };
    async_manager::spawn(write::save_track(track, server, map, true));
}

/// Resolve the unsanitized display server name. Returns
/// `"singleplayer"` placeholder when in singleplayer mode (the wire
/// `Server.Name` is empty there). Color codes are left in place for
/// the writer's display-only consumer; the path sanitizer strips
/// them later when building filesystem paths.
fn current_server_display() -> Option<String> {
    // SAFETY: `Server` is the engine's `static mut _ServerConnectionData`.
    // We're called on the main game thread (via `on_track_loaded` from
    // `splits::load_track`, or via `on_new_map_loaded` dispatch). `&raw`
    // avoids creating an `&'static mut` ref (Rust 2024 `static_mut_refs`
    // lint). `cc_string`'s `Display` impl copies through the buffer.
    let server_ptr = &raw const Server;
    let is_sp = unsafe { (*server_ptr).IsSinglePlayer } != 0;
    if is_sp {
        return Some("singleplayer".to_owned());
    }
    let name = unsafe { (*server_ptr).Name.to_string() };
    if name.is_empty() { None } else { Some(name) }
}
