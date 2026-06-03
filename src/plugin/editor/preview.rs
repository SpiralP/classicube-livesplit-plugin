//! Rubber-band preview for mid-placement checkpoint authoring.
//!
//! While a placement is armed (`edit add` / `edit redraw`) and the first
//! corner has been clicked, draw a live tentative box from corner A to
//! wherever the player is currently aiming, updating every frame. The
//! second click commits a permanent checkpoint whose AABB matches the
//! preview exactly -- both are built by `geometry::aabb_from_block_corners`.
//!
//! Rendered via `Selections_Add` on a reserved id (255) so no custom GPU
//! code is needed. White + reduced alpha reads as "tentative" and is
//! visually distinct from the committed kind-colored checkpoint boxes
//! (`hud/boxes.rs`, ids 200..=254).
//!
//! The live aimed-at block is read cross-platform through `Camera.Active
//! ->GetPickedBlock` -- the `Camera` global is `CC_VAR`-exported on all
//! platforms including Windows (unlike the plain `extern Game_SelectedPos`
//! which is not exported from `ClassiCube.exe`).

use std::{cell::Cell, mem};

use classicube_sys::{IVec3, PackedCol_Make, RayTracer, Selections_Add, Selections_Remove};

use crate::plugin::splits::geometry;

/// Selection id reserved for the rubber-band preview. `hud/boxes.rs` owns
/// `200..=254`; this sits one above that range.
pub(super) const PREVIEW_SELECTION_ID: u8 = 255;

/// Alpha for the tentative preview box. White at ~40% opacity reads as
/// "this is where the box will go" without obscuring the world.
const PREVIEW_ALPHA: u8 = 100;

thread_local! {
    /// The `(p1, p2)` pair currently installed as selection 255, or `None`
    /// when no preview is shown. Used to short-circuit `apply` so the engine
    /// is only touched on a change.
    static LAST_PREVIEW: Cell<Option<(IVec3, IVec3)>> = const { Cell::new(None) };
}

/// Called each frame (by `EditorModule`'s `TickEventHandler`) to update the
/// preview. Reads editor state and the live aimed-at block; installs or
/// removes selection 255 as needed.
pub(super) fn reconcile() {
    let corner_a = super::EDITOR_STATE.with_borrow(|s| {
        if s.enabled {
            s.pending.as_ref().and_then(|p| p.corner_a)
        } else {
            None
        }
    });

    let want = corner_a.and_then(|a| live_block().map(|b| selection_corners(a, b)));
    apply(want);
}

/// Read the block the local player is currently aiming at via the active
/// camera's raytrace. Returns `None` when not looking at any block.
///
/// Uses `Camera.Active->GetPickedBlock` rather than the global
/// `Game_SelectedPos` because `Camera` is `CC_VAR`-exported on Windows
/// while `Game_SelectedPos` is a plain `extern` (not exported from the
/// exe) and would fail to link in the Windows plugin DLL.
fn live_block() -> Option<IVec3> {
    // SAFETY: `Camera` is a CC_VAR global — always valid after plugin load.
    // `Camera.Active` is the engine-managed active camera pointer; non-null
    // whenever the game is running. All accesses are on the main thread.
    unsafe {
        let active = classicube_sys::Camera.Active;
        if active.is_null() {
            return None;
        }
        let get = (*active).GetPickedBlock?;
        // Zero-init is safe: RayTracer contains only ints, floats, and a
        // cc_bool — no pointers. `valid = 0` is the "no block" sentinel.
        let mut t: RayTracer = mem::zeroed();
        get(&mut t);
        // Use `pos` (the block looked at) so the preview matches the
        // engine's own white selection outline on the targeted block.
        (t.valid != 0).then_some(t.pos)
    }
}

/// Build the two `IVec3` selection corners for a preview box that
/// matches what `geometry::aabb_from_block_corners(a, b)` would produce.
/// Using the same source of truth ensures the preview's extent is identical
/// to the committed checkpoint before the second click lands.
///
/// `aabb_from_block_corners` always produces integer-valued floats, so
/// `Vec3::floor()` is equivalent to rounding and avoids a local helper.
fn selection_corners(a: IVec3, b: IVec3) -> (IVec3, IVec3) {
    let aabb = geometry::aabb_from_block_corners(a, b);
    (aabb.min.floor(), aabb.max.floor())
}

/// Install / remove / update selection 255 to reflect `want`, touching the
/// engine only when the state changed.
fn apply(want: Option<(IVec3, IVec3)>) {
    if LAST_PREVIEW.get() == want {
        return;
    }
    match want {
        Some((p1, p2)) => unsafe {
            // Re-adding an existing id replaces it. Remove first to be
            // consistent with `boxes.rs` and safe against engine variants.
            Selections_Remove(PREVIEW_SELECTION_ID);
            Selections_Add(
                PREVIEW_SELECTION_ID,
                &p1,
                &p2,
                PackedCol_Make(255, 255, 255, PREVIEW_ALPHA),
            );
        },
        None => unsafe {
            Selections_Remove(PREVIEW_SELECTION_ID);
        },
    }
    LAST_PREVIEW.set(want);
}

/// Remove the preview selection and reset the cache. Called on `init()`
/// (sweeps any hot-reload leak) and `free()`.
pub(super) fn clear() {
    unsafe { Selections_Remove(PREVIEW_SELECTION_ID) };
    LAST_PREVIEW.set(None);
}

/// Reset the cache without calling `Selections_Remove` — for use on map
/// change, where the engine has already wiped its own selection list.
pub(super) fn invalidate() {
    LAST_PREVIEW.set(None);
}
