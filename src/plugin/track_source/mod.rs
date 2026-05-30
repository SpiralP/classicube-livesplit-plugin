pub mod decode;
pub mod encode;

use std::{cell::Cell, ptr, slice};

use classicube_sys::{
    MsgType_MSG_TYPE_NORMAL, Net_Handler, OPCODE__OPCODE_MESSAGE, Protocol, Server,
    UNSAFE_GetString,
};
use tracing::debug;

use crate::{
    chat_print,
    plugin::{
        is_plugin_active,
        module::Module,
        splits,
        track_source::decode::{FrameOutcome, feed_chat_line},
    },
};

// Semantics: `None` = our hook is not installed; `Some(prior)` =
// installed, `prior` is what `Protocol.Handlers[OPCODE_MESSAGE]` held
// before we patched it. See the install/uninstall pair for the
// chain-survival reasoning.
thread_local!(
    static OLD_MESSAGE_HANDLER: Cell<Net_Handler> = const { Cell::new(None) };
);

extern "C" fn message_handler(data: *mut u8) {
    if is_plugin_active() {
        let bytes = unsafe { slice::from_raw_parts(data, 65) };
        let message_type = bytes[0];

        if message_type == MsgType_MSG_TYPE_NORMAL as u8 {
            let text = unsafe { UNSAFE_GetString(&bytes[1..]) }.to_string();
            match feed_chat_line(&text) {
                FrameOutcome::NotOurs => {
                    // Fall through to the chain so the line renders.
                }
                FrameOutcome::ParseError(msg) => {
                    chat_print(&format!("&cLiveSplit: malformed track frame: {msg}"));
                    // Fall through so the raw line still renders.
                }
                FrameOutcome::Buffered => {
                    // Mid-track frame; suppress.
                    return;
                }
                FrameOutcome::Loaded(track) => {
                    debug!(
                        name = track.name,
                        checkpoints = track.checkpoints.len(),
                        "received chat-protocol track"
                    );
                    if splits::load_track(track) {
                        return;
                    }
                    // load_track returned false: plugin mid-teardown.
                    // Fall through so the line renders normally rather
                    // than silently disappearing.
                }
            }
        }
    }

    OLD_MESSAGE_HANDLER.with(|cell| {
        if let Some(f) = cell.get() {
            unsafe {
                f(data);
            }
        }
    });
}

fn is_our_handler(handler: Net_Handler) -> bool {
    handler.is_some_and(|h| ptr::fn_addr_eq(h, message_handler as unsafe extern "C" fn(*mut u8)))
}

fn install_message_handler() {
    let current = unsafe { Protocol.Handlers[OPCODE__OPCODE_MESSAGE as usize] };

    // Already at the top of the chain — nothing to do.
    if is_our_handler(current) {
        return;
    }

    // We previously installed ourselves and another plugin has since
    // stacked its own hook on top. Re-pushing to the top would set
    //   slot = us, OLD_MESSAGE_HANDLER = other_plugin
    // while other_plugin's saved "old" still points at us — infinite
    // recursion through our own handler. Leave the chain alone; we're
    // still reachable via the existing chain.
    if OLD_MESSAGE_HANDLER.with(Cell::get).is_some() {
        return;
    }

    unsafe {
        Protocol.Handlers[OPCODE__OPCODE_MESSAGE as usize] = Some(message_handler);
    }
    OLD_MESSAGE_HANDLER.with(|cell| cell.set(current));
}

fn uninstall_message_handler() {
    let current = unsafe { Protocol.Handlers[OPCODE__OPCODE_MESSAGE as usize] };

    if is_our_handler(current) {
        // We're still on top — safe to splice ourselves out.
        let prior = OLD_MESSAGE_HANDLER.with(Cell::take);
        unsafe {
            Protocol.Handlers[OPCODE__OPCODE_MESSAGE as usize] = prior;
        }
    }
    // Else: another plugin stacked on top of us. Overwriting the slot
    // would drop their hook out of the chain. Leave Protocol.Handlers
    // alone, and keep OLD_MESSAGE_HANDLER populated — our
    // message_handler is still reachable via the chain and needs OLD
    // to fall through to the original while is_plugin_active() is
    // false.
}

pub struct TrackSourceModule;

impl TrackSourceModule {
    pub fn init() -> Self {
        // Singleplayer has no Protocol layer (no server, no incoming
        // OPCODE_MESSAGE packets). Tracks come from servers, so we
        // skip the install entirely in SP.
        if unsafe { Server.IsSinglePlayer } == 0 {
            install_message_handler();
        }
        Self
    }
}

impl Module for TrackSourceModule {
    fn free(&mut self) {
        uninstall_message_handler();
    }
}
