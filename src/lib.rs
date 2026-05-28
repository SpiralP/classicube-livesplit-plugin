pub mod chat;
pub mod plugin;

use std::{os::raw::c_int, ptr};

use classicube_helpers::time;
use classicube_sys::IGameComponent;

pub use crate::chat::chat_print;

extern "C" fn init() {
    time!("plugin::initialize()", 5000, {
        plugin::initialize();
    });
}

extern "C" fn free() {
    time!("plugin::free()", 1000, {
        plugin::free();
    });
}

#[tracing::instrument]
extern "C" fn reset() {
    time!("plugin::reset()", 1000, {
        plugin::reset();
    });
}

#[tracing::instrument]
extern "C" fn on_new_map() {
    time!("plugin::on_new_map()", 1000, {
        plugin::on_new_map();
    });
}

#[tracing::instrument]
extern "C" fn on_new_map_loaded() {
    time!("plugin::on_new_map_loaded()", 1000, {
        plugin::on_new_map_loaded();
    });
}

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static Plugin_ApiVersion: c_int = 1;

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static mut Plugin_Component: IGameComponent = IGameComponent {
    // Called when the game is being loaded.
    Init: Some(init),
    // Called when the component is being freed. (e.g. due to game being closed)
    Free: Some(free),
    // Called to reset the component's state. (e.g. reconnecting to server)
    Reset: Some(reset),
    // Called to update the component's state when the user begins loading a new map.
    OnNewMap: Some(on_new_map),
    // Called to update the component's state when the user has finished loading a new map.
    OnNewMapLoaded: Some(on_new_map_loaded),
    // Next component in linked list of components.
    next: ptr::null_mut(),
};
