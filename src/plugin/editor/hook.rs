//! `Server.SendBlock` chain override for the in-world editor.
//!
//! `SendBlock` is the engine's single block-action hook: it fires for
//! every block the local player places or breaks (in both singleplayer
//! and multiplayer). We splice our own function into that slot, save the
//! previous one, and chain through to it when the editor isn't consuming
//! the click. The reload-safe install/uninstall mirrors the
//! `track_source` `Protocol.Handlers` chain pattern, with one extra case
//! for reconnect (`MPConnection_Init` re-points `SendBlock` back to the
//! engine original, dropping us -- we re-splice when the live slot equals
//! the original we saved).

use std::{cell::Cell, os::raw::c_int, ptr};

use classicube_sys::{BlockID, Server};

use crate::plugin::{editor, is_plugin_active};

type SendBlockFn = unsafe extern "C" fn(c_int, c_int, c_int, BlockID, BlockID);

// `None` = our hook is not installed. `Some(prior)` = installed; `prior`
// is the `Server.SendBlock` value we displaced (the engine original, or
// whatever was on top when we installed). See install/uninstall for the
// chain-survival reasoning.
thread_local!(
    static OLD_SEND_BLOCK: Cell<Option<SendBlockFn>> = const { Cell::new(None) };
);

extern "C" fn send_block_hook(x: c_int, y: c_int, z: c_int, old: BlockID, now: BlockID) {
    if is_plugin_active() && editor::consume_click(x, y, z, old) {
        // Editor recorded a corner + reverted the local block; suppress
        // the server-notify so the click never reaches the world.
        return;
    }
    OLD_SEND_BLOCK.with(|c| {
        if let Some(f) = c.get() {
            // SAFETY: `f` is the engine's original `SendBlock` (or the
            // next hook in the chain), called with the same arguments
            // the engine passed us, on the main thread.
            unsafe {
                f(x, y, z, old, now);
            }
        }
    });
}

fn is_our_handler(handler: Option<SendBlockFn>) -> bool {
    handler.is_some_and(|h| ptr::fn_addr_eq(h, send_block_hook as SendBlockFn))
}

fn same_fn(a: Option<SendBlockFn>, b: Option<SendBlockFn>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => ptr::fn_addr_eq(x, y),
        (None, None) => true,
        _ => false,
    }
}

/// Read the live `Server.SendBlock` slot. The raw pointer is bound to a
/// local before deref (rather than `(*(&raw const Server))...` inline) to
/// avoid both the `static_mut_refs` lint (no `&'static mut`) and clippy's
/// `deref_addrof` -- the same shape `lss_storage` / `track_source` use.
fn current_slot() -> Option<SendBlockFn> {
    let server_ptr = &raw const Server;
    // SAFETY: `Server` is the engine's `static mut`; main-thread access.
    unsafe { (*server_ptr).SendBlock }
}

fn set_slot(f: Option<SendBlockFn>) {
    let server_ptr = &raw mut Server;
    // SAFETY: as above; we own the slot while our hook is on top.
    unsafe {
        (*server_ptr).SendBlock = f;
    }
}

/// Splice our hook into `Server.SendBlock`, saving the displaced fn.
/// Idempotent, and safe to call repeatedly (e.g. re-asserted on
/// `on_new_map_loaded`).
pub(super) fn install() {
    let current = current_slot();

    // Already on top -- nothing to do.
    if is_our_handler(current) {
        return;
    }

    match OLD_SEND_BLOCK.with(Cell::get) {
        None => {
            // First install: capture the original and splice in.
            set_slot(Some(send_block_hook));
            OLD_SEND_BLOCK.with(|c| c.set(current));
        }
        Some(saved) => {
            if same_fn(current, Some(saved)) {
                // Reconnect / world reload (`MPConnection_Init`) put the
                // engine original -- the same fn we already saved -- back
                // in the slot, dropping our hook. Re-splice; OLD already
                // holds the correct original.
                set_slot(Some(send_block_hook));
            }
            // Else: a foreign handler is on top of us. Re-pushing would
            // build a cycle (our OLD -> original, their OLD -> us). Leave
            // the chain alone; our hook is still reachable through theirs.
        }
    }
}

/// Splice our hook back out of `Server.SendBlock`, restoring the saved
/// fn -- but only while we're still on top. If a foreign handler stacked
/// over us, leave the chain intact (overwriting would drop their hook)
/// and keep `OLD_SEND_BLOCK` populated so our still-reachable
/// `send_block_hook` can fall through to the original while
/// `is_plugin_active()` is false.
pub(super) fn uninstall() {
    if is_our_handler(current_slot()) {
        let prior = OLD_SEND_BLOCK.with(Cell::take);
        set_slot(prior);
    }
}
