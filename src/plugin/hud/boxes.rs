//! Translucent colored cuboids for the loaded track's AABB checkpoints.
//!
//! Each `Trigger::Aabb` checkpoint is drawn via ClassiCube's built-in
//! `Selections_Add` / `Selections_Remove` `CC_API`, color-coded by
//! `CheckpointKind`. The engine re-renders its selection list every frame,
//! so this layer never touches the GPU -- it's pure state management: each
//! tick [`reconcile`] diffs the desired `(kind, aabb, is_next)` set against
//! the installed one ([`LAST_APPLIED`]) and only calls the engine on a
//! change. The next-target box is drawn at a higher alpha (same kind hue).

use std::cell::RefCell;

use classicube_sys::{IVec3, PackedCol, PackedCol_Make, Selections_Add, Selections_Remove, Vec3};
use tracing::{debug, warn};

use super::{HUD_ID_BASE, HUD_ID_COUNT, HUD_ID_LAST};
use crate::plugin::{
    module::Module,
    splits::geometry::{Aabb, CheckpointKind},
};

thread_local! {
    /// The scoped AABB set the currently-installed selections reflect:
    /// exactly the `(kind, aabb, is_next)` triples we last drew, in draw
    /// order. The tick reconcile short-circuits while this equals the
    /// want-state, so steady-state cost is one walk + compare per tick.
    /// Caching the *filtered* set (not the whole `Track`) means a map
    /// crossing -- which changes which AABBs are in scope -- naturally
    /// invalidates the cache; carrying `is_next` means the run cursor
    /// advancing (same kind+aabb set, different highlight) likewise
    /// forces a re-add. Reset to empty on map change / reset, because the
    /// engine wipes its own selection list on `OnNewMap` / `Reset` and
    /// our cache would otherwise wrongly suppress the re-add.
    static LAST_APPLIED: RefCell<Vec<(CheckpointKind, Aabb, bool)>> =
        const { RefCell::new(Vec::new()) };

    /// Selection ids we currently have installed, so the next reconcile
    /// removes exactly what it added.
    static ACTIVE_IDS: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

pub(super) struct BoxesModule;

impl BoxesModule {
    pub(super) fn init() -> Self {
        // Sweep our private id range in case a prior Init leaked
        // selections (crash / abnormal teardown). Removing an id that
        // isn't installed is a harmless no-op engine-side.
        clear_all_selections();
        Self
    }
}

impl Module for BoxesModule {
    fn free(&mut self) {
        clear_all_selections();
        LAST_APPLIED.with_borrow_mut(Vec::clear);
        debug!("checkpoint boxes cleared");
    }

    // The engine wipes its selection list on every `OnNewMap` / `Reset`
    // (`SelectionBox.c`'s OnReset zeroes `selections_count`). Our cached
    // "already applied" snapshot would then wrongly suppress the re-add,
    // so invalidate it here -- the next tick reconcile reinstalls the
    // boxes. These hooks fire after the built-in Selections component
    // has already cleared the list (plugins dispatch after core
    // components), so we never re-add ahead of the clear.
    fn reset(&mut self) {
        invalidate();
    }

    fn on_new_map_loaded(&mut self) {
        invalidate();
    }
}

/// Recompute and apply the desired selection set from the map-scoped
/// `visible` set (the boxes ignore the per-checkpoint label but keep the
/// `is_next` highlight flag). Cheap no-op while the want-state is
/// unchanged.
pub(super) fn reconcile(visible: &[(CheckpointKind, Aabb, String, bool)]) {
    let want: Vec<(CheckpointKind, Aabb, bool)> =
        visible.iter().map(|(k, a, _, n)| (*k, *a, *n)).collect();

    if LAST_APPLIED.with_borrow(|last| *last == want) {
        return;
    }

    // Remove whatever the previous reconcile installed.
    ACTIVE_IDS.with_borrow_mut(|ids| {
        for id in ids.drain(..) {
            unsafe { Selections_Remove(id) };
        }
    });

    if want.len() > HUD_ID_COUNT {
        warn!(
            total = want.len(),
            cap = HUD_ID_COUNT,
            "more in-scope AABB checkpoints than the HUD id range; showing the first \
             {HUD_ID_COUNT}"
        );
    }

    ACTIVE_IDS.with_borrow_mut(|ids| {
        // zip stops at the shorter side, capping installs at HUD_ID_COUNT.
        for (id, (kind, aabb, is_next)) in (HUD_ID_BASE..=HUD_ID_LAST).zip(&want) {
            let p1 = ivec3(aabb.min);
            let p2 = ivec3(aabb.max);
            unsafe { Selections_Add(id, &p1, &p2, color_for_kind(*kind, *is_next)) };
            ids.push(id);
        }
    });

    LAST_APPLIED.with_borrow_mut(|last| *last = want);
}

/// Drop the cached snapshot without removing selections (the caller
/// relies on the engine having already cleared its own list). Forces the
/// next reconcile to re-add: an empty cache never equals a non-empty want,
/// and if the want is also empty there's nothing to draw anyway.
fn invalidate() {
    LAST_APPLIED.with_borrow_mut(Vec::clear);
    ACTIVE_IDS.with_borrow_mut(Vec::clear);
}

/// Remove every selection in our private id range and forget them.
fn clear_all_selections() {
    for id in HUD_ID_BASE..=HUD_ID_LAST {
        unsafe { Selections_Remove(id) };
    }
    ACTIVE_IDS.with_borrow_mut(Vec::clear);
}

/// Convert a runtime float world coord to the block-grid `IVec3` the
/// selection API takes. The runtime AABB uses the half-open convention
/// `min <= p < max`, so the natural cast (e.g. min `(10,4,20)`, max
/// `(11,5,21)` for a 1x1x1 trigger) lands on the block-exclusive high
/// corner the engine wants to wrap the far block fully.
#[expect(
    clippy::cast_possible_truncation,
    reason = "world coords are non-negative and well within i32 after rounding"
)]
fn ivec3(v: Vec3) -> IVec3 {
    IVec3 {
        x: v.x.round() as i32,
        y: v.y.round() as i32,
        z: v.z.round() as i32,
    }
}

/// Alpha applied to a non-next checkpoint box. The engine alpha-blends
/// both the translucent face fill and the (RGB-inverted) wireframe edges
/// off this same byte (`SelectionBox.c` BuildFaces / BuildEdges), so it's
/// a uniform fade -- kept low enough to read as "mostly transparent" but
/// high enough that the outline stays legible against the world.
const BOX_ALPHA: u8 = 64;

/// Alpha for the run's next-target box. Same kind hue as every other box
/// (so it still reads as "yellow = split" etc.), just far more opaque so
/// it pops as the one to head for. Stays legible against all five kind
/// colors since only the alpha changes.
const NEXT_BOX_ALPHA: u8 = 170;

fn color_for_kind(kind: CheckpointKind, is_next: bool) -> PackedCol {
    // `PackedCol_Make` is the classicube-sys const-fn wrapper around the
    // engine's `PackedCol_Make` macro -- it builds the word from the
    // platform-correct channel shifts, so we don't hand-roll the packing.
    let a = if is_next { NEXT_BOX_ALPHA } else { BOX_ALPHA };
    match kind {
        CheckpointKind::Start => PackedCol_Make(0, 255, 0, a), // green
        CheckpointKind::Split => PackedCol_Make(255, 255, 0, a), // yellow
        CheckpointKind::Pause => PackedCol_Make(0, 200, 255, a), // cyan
        CheckpointKind::Resume => PackedCol_Make(255, 140, 0, a), // orange
        CheckpointKind::End => PackedCol_Make(255, 0, 0, a),   // red
    }
}
