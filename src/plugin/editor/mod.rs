//! In-world track editor: place / remove / relabel checkpoints with chat
//! commands and block clicks.
//!
//! `/client LiveSplit edit on` enables edit mode and installs a
//! `Server.SendBlock` override ([`hook`]). While armed (via
//! `edit add`), the next two block clicks are consumed as the two
//! corners of a checkpoint AABB instead of building/breaking world blocks
//! -- the clicked block is reverted locally and never sent to the server.
//! The committed checkpoint flows through `splits::editor_add`, the
//! same `Track` mutation path the chat/`.lss` sources feed into, and
//! becomes visible through the existing `/client LiveSplit show` HUD on
//! the next tick.
//!
//! A **rubber-band preview** (selection 255, white translucent) shows the
//! tentative AABB while the player aims after clicking corner A; it updates
//! every frame via [`preview`] and disappears on commit or cancel.
//! Corner-A marker, selection highlight, and edit-mode status overlay are
//! still deferred to the track-editor HUD work.

pub mod hook;
mod preview;

use std::{cell::RefCell, os::raw::c_int};

use classicube_helpers::tick::TickEventHandler;
use classicube_sys::{BlockID, Game_UpdateBlock, IVec3};
use tracing::debug;

use crate::{
    chat_print,
    plugin::{
        module::Module,
        splits::{
            self,
            geometry::{self, CheckpointKind, RetypeTarget},
        },
    },
};

/// Placeholder label committed when a checkpoint is placed via a block
/// click. Non-empty so the chat encoder / `.lss` writer accept it (both
/// require non-empty labels); override with
/// `/client LiveSplit edit label <i> <text>`.
const PLACEHOLDER_LABEL: &str = "checkpoint";

/// Which mutation a pending two-click capture commits to.
#[derive(Clone, Copy)]
enum PendingOp {
    /// `edit add [i]`: insert a checkpoint. `None` appends to the
    /// current map section; `Some(i)` inserts at index `i`.
    Add(Option<usize>),
    /// `edit redraw <i>`: replace the AABB of the existing checkpoint at
    /// `i`, keeping its kind / label / position.
    Redraw(usize),
}

/// A two-click capture armed via `edit add` / `edit redraw`, waiting
/// for its two corner clicks. `corner_a` is `None` until the first click
/// lands.
struct Pending {
    /// Which mutation the two corners commit to once both are clicked.
    op: PendingOp,
    corner_a: Option<IVec3>,
}

struct EditorState {
    /// Edit mode on/off (`edit on` / `edit off`). When off, the
    /// `SendBlock` hook (if still installed) passes every click through.
    enabled: bool,
    /// `Some` once `edit add` arms a placement; `None` otherwise.
    pending: Option<Pending>,
}

thread_local! {
    static EDITOR_STATE: RefCell<EditorState> = const {
        RefCell::new(EditorState {
            enabled: false,
            pending: None,
        })
    };
}

/// `edit on` / `edit off`. Installs the `SendBlock` hook on enable and
/// uninstalls it on disable; also clears any half-armed placement when
/// turning off.
pub fn set_enabled(on: bool) {
    EDITOR_STATE.with_borrow_mut(|s| {
        s.enabled = on;
        if !on {
            s.pending = None;
        }
    });
    if on {
        hook::install();
        chat_print("&aLiveSplit: edit mode ON");
        chat_print("&e  /client LiveSplit edit add, then click two blocks for a checkpoint");
    } else {
        hook::uninstall();
        chat_print("&aLiveSplit: edit mode OFF");
    }
}

/// `edit add [i]`. Arm a placement: the next two block clicks become a
/// checkpoint's corners. `target` is `None` to append to the player's
/// current map section (before its terminating `MapLoaded`, or before
/// `End` on the last/only map) or `Some(i)` to insert at index `i`.
pub fn arm_add(target: Option<usize>) {
    let armed = EDITOR_STATE.with_borrow_mut(|s| {
        if !s.enabled {
            return false;
        }
        s.pending = Some(Pending {
            op: PendingOp::Add(target),
            corner_a: None,
        });
        true
    });
    if !armed {
        chat_print("&cLiveSplit: enable edit mode first (/client LiveSplit edit on)");
        return;
    }
    match target {
        None => chat_print("&aLiveSplit: armed - click corner A (appends to current map)"),
        Some(i) => chat_print(&format!(
            "&aLiveSplit: armed - click corner A (inserts at #{i})"
        )),
    }
}

/// `edit redraw <i>`. Arm a two-click capture that replaces the AABB of
/// the existing checkpoint at `i` (keeping its index, kind, and label)
/// instead of inserting a new one. No index pre-check here -- consistent
/// with [`arm_add`]; the authoritative range / `MapLoaded` check lives
/// in `splits::editor_relocate` and surfaces via chat at commit. The two
/// clicks revert locally, so a wasted arm leaves the map untouched.
pub fn arm_redraw(i: usize) {
    let armed = EDITOR_STATE.with_borrow_mut(|s| {
        if !s.enabled {
            return false;
        }
        s.pending = Some(Pending {
            op: PendingOp::Redraw(i),
            corner_a: None,
        });
        true
    });
    if !armed {
        chat_print("&cLiveSplit: enable edit mode first (/client LiveSplit edit on)");
        return;
    }
    chat_print(&format!(
        "&aLiveSplit: armed - click corner A (redraws checkpoint #{i})"
    ));
}

/// `edit cancel`. Discard a half-armed placement.
pub fn cancel() {
    let had = EDITOR_STATE.with_borrow_mut(|s| s.pending.take().is_some());
    if had {
        chat_print("&aLiveSplit: placement cancelled");
    } else {
        chat_print("&eLiveSplit: nothing to cancel");
    }
}

/// `edit remove <i>`. Remove the checkpoint at `i`.
pub fn remove(i: usize) {
    splits::editor_remove(i);
}

/// `edit label <i> <text>`. Relabel a checkpoint.
pub fn set_label(i: usize, text: String) {
    splits::editor_set_label(i, text);
}

/// `edit move <from> <to>`. Reorder a checkpoint within the route: the
/// one at `from` lands at index `to`, shifting the rest. No arming /
/// block clicks (purely index-based), so -- like `remove` / `label` --
/// it doesn't gate on edit mode.
pub fn reindex(from: usize, to: usize) {
    splits::editor_reindex(from, to);
}

/// `edit kind <i> split|pause|resume`. Retype an existing AABB
/// checkpoint, keeping its zone. No arming / block clicks (purely
/// index-based), so -- like `remove` / `label` / `move` -- it doesn't
/// gate on edit mode.
pub fn set_kind(i: usize, kind: CheckpointKind) {
    splits::editor_set_kind(i, RetypeTarget::Aabb(kind));
}

/// `edit kind <i> map [name]`. Convert checkpoint #i into a zoneless map
/// transition. `name` defaults to the live world (`splits::current_map`);
/// errors in chat if a name is neither given nor resolvable.
pub fn set_kind_map(i: usize, name: Option<String>) {
    match name.or_else(splits::current_map) {
        Some(n) => {
            splits::editor_set_kind(i, RetypeTarget::Map(n));
        }
        None => chat_print("&cLiveSplit: no map name given and current map is unknown"),
    }
}

/// `edit clear`. Drop the loaded track and reset editor state so the
/// player can author a fresh one from scratch.
pub fn clear() {
    splits::clear_track();
    EDITOR_STATE.with_borrow_mut(|s| {
        s.pending = None;
    });
}

/// What a `SendBlock` click resolved to after consulting editor state.
enum ClickOutcome {
    /// Not in edit mode / not armed: let the block change flow through.
    Ignore,
    /// First corner recorded; revert the block and wait for corner B.
    CornerA,
    /// Second corner recorded; revert the block and commit the AABB.
    CornerB { a: IVec3, b: IVec3, op: PendingOp },
}

/// Called from the [`hook`]'s `Server.SendBlock` override for every block
/// the player places/breaks. Returns `true` when the editor consumed the
/// click as a checkpoint corner (the caller then suppresses the
/// server-notify), `false` to pass the block change through unchanged.
///
/// When a placement is armed, the click is always consumed: the local
/// world block the engine already mutated is reverted via
/// `Game_UpdateBlock` (which does NOT re-enter `SendBlock`, so no
/// recursion), then recorded as corner A or combined with A into a
/// committed checkpoint.
pub(super) fn consume_click(x: c_int, y: c_int, z: c_int, old: BlockID) -> bool {
    // Scope the EDITOR_STATE borrow to the decision; the revert +
    // splits mutation happen after it's released so the SendBlock hook
    // can't double-borrow editor state, and `editor_add`'s own
    // splits borrow stays independent.
    let outcome = EDITOR_STATE.with_borrow_mut(|s| {
        if !s.enabled {
            return ClickOutcome::Ignore;
        }
        let Some(pending) = s.pending.as_mut() else {
            return ClickOutcome::Ignore;
        };
        match pending.corner_a {
            None => {
                pending.corner_a = Some(IVec3 { x, y, z });
                ClickOutcome::CornerA
            }
            Some(a) => {
                let op = pending.op;
                s.pending = None;
                ClickOutcome::CornerB {
                    a,
                    b: IVec3 { x, y, z },
                    op,
                }
            }
        }
    });

    match outcome {
        ClickOutcome::Ignore => false,
        ClickOutcome::CornerA => {
            revert_block(x, y, z, old);
            chat_print("&aLiveSplit: corner A set; click corner B");
            true
        }
        ClickOutcome::CornerB { a, b, op } => {
            revert_block(x, y, z, old);
            let aabb = geometry::aabb_from_block_corners(a, b);
            match op {
                PendingOp::Add(target) => {
                    splits::editor_add(aabb, PLACEHOLDER_LABEL.to_owned(), target);
                }
                PendingOp::Redraw(i) => {
                    splits::editor_relocate(i, aabb);
                }
            }
            true
        }
    }
}

/// Restore the block the engine already changed locally. `Game_UpdateBlock`
/// updates only the local world (unlike `Game_ChangeBlock` it does not
/// call back into `Server.SendBlock`), so the edit click leaves the map
/// untouched and there's no hook recursion.
fn revert_block(x: c_int, y: c_int, z: c_int, old: BlockID) {
    // SAFETY: `Game_UpdateBlock` is a `CC_API` engine fn; we're on the
    // main thread (input dispatch), with in-range world coords supplied
    // by the engine's own `SendBlock` call.
    unsafe {
        Game_UpdateBlock(x, y, z, old);
    }
}

pub struct EditorModule {
    _tick: TickEventHandler,
}

impl EditorModule {
    pub fn init() -> Self {
        // Sweep the preview id in case a prior Init leaked it (crash /
        // abnormal teardown). Removing a non-installed id is a harmless no-op.
        preview::clear();
        // The SendBlock hook is installed lazily on `edit on`, not here:
        // `Server.SendBlock` is set by `SPConnection_Init` /
        // `MPConnection_Init` on world load / connect, so it may still be
        // unpopulated at plugin construction.
        let mut tick = TickEventHandler::new();
        tick.on(|_| preview::reconcile());
        Self { _tick: tick }
    }
}

impl Module for EditorModule {
    fn free(&mut self) {
        preview::clear();
        hook::uninstall();
        EDITOR_STATE.with_borrow_mut(|s| {
            s.enabled = false;
            s.pending = None;
        });
        debug!("EditorModule freed; SendBlock hook uninstalled, editor state cleared");
        // `_tick` unregisters via its own Drop after `free` returns; no
        // render or tick event fires during synchronous teardown.
    }

    fn on_new_map_loaded(&mut self) {
        // A half-armed placement holds corner coords from the old map;
        // they're meaningless after a map change. Invalidate the preview
        // cache -- the engine already wiped its selection list on map load.
        EDITOR_STATE.with_borrow_mut(|s| s.pending = None);
        preview::invalidate();
        // Reconnect / world reload re-points `Server.SendBlock` (e.g.
        // `MPConnection_Init`), dropping our hook. Re-assert it while
        // edit mode is on.
        if EDITOR_STATE.with_borrow(|s| s.enabled) {
            hook::install();
        }
    }
}
