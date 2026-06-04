#[cfg(test)]
mod tests;

use anyhow::{Result, bail};
use classicube_sys::{IVec3, Vec3};

use crate::plugin::livesplit::{Command, protocol::TimingMethod};

/// Quantize an `f32` world coord to block precision (`u16`). CC world
/// coords are non-negative and a "big" CC map is roughly 700 × 300 × 1000
/// blocks, an order of magnitude under `u16::MAX`, so `u16` covers any
/// `min` with margin. Out-of-range values clamp.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "value is clamp()'d to the u16 range before the cast"
)]
pub(crate) fn quantize_axis(v: f32) -> u16 {
    v.round().clamp(0.0, f32::from(u16::MAX)) as u16
}

pub(crate) fn vec3_to_u16(v: Vec3) -> [u16; 3] {
    [quantize_axis(v.x), quantize_axis(v.y), quantize_axis(v.z)]
}

pub(crate) fn u16_to_vec3([x, y, z]: [u16; 3]) -> Vec3 {
    Vec3::new(f32::from(x), f32::from(y), f32::from(z))
}

/// Quantized wire form of an [`Aabb`]: `min` as `[u16; 3]` block coords,
/// extents as `[u8; 3]` (typical checkpoint volumes are a handful of
/// blocks per axis). Shared between the plaintext encoder (which writes
/// these as comma-separated decimals on the wire) and the decoder
/// (which reads them back). Errors if any axis extent exceeds 255 blocks.
pub(crate) fn aabb_to_min_size(aabb: Aabb) -> Result<([u16; 3], [u8; 3])> {
    let min = vec3_to_u16(aabb.min);
    let max = vec3_to_u16(aabb.max);
    let mut size = [0u8; 3];
    for axis in 0..3 {
        let extent = max[axis].saturating_sub(min[axis]);
        let Ok(byte) = u8::try_from(extent) else {
            bail!("AABB extent {extent} exceeds 255 blocks on one axis");
        };
        size[axis] = byte;
    }
    Ok((min, size))
}

pub(crate) fn aabb_from_min_size(min: [u16; 3], size: [u8; 3]) -> Aabb {
    let max = [
        min[0].saturating_add(u16::from(size[0])),
        min[1].saturating_add(u16::from(size[1])),
        min[2].saturating_add(u16::from(size[2])),
    ];
    Aabb {
        min: u16_to_vec3(min),
        max: u16_to_vec3(max),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    #[must_use]
    pub fn new(a: Vec3, b: Vec3) -> Self {
        Self {
            min: Vec3::new(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z)),
            max: Vec3::new(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z)),
        }
    }

    /// Closed AABB-vs-AABB overlap, matching the server's `AABB.Intersects`
    /// (a.Max >= b.Min && a.Min <= b.Max per axis). Used by [`step`] to fire a
    /// checkpoint when the player's model collision box overlaps it (message-block
    /// "walkthrough" parity). Closed, so face-touching counts -- sequential firing
    /// (`next_index`) keeps adjacent boxes from both firing.
    #[must_use]
    pub fn intersects(&self, other: &Aabb) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
            && self.min.z <= other.max.z
            && self.max.z >= other.min.z
    }
}

/// Build an [`Aabb`] from two clicked block corners. Each axis spans
/// `min(a, b) .. max(a, b) + 1`: the `+1` turns each clicked block coord
/// into its half-open `[block, block + 1)` world interval, so a single
/// block (both corners equal) yields a 1x1x1 box. Worked example:
/// `(10,4,20)` + `(12,7,22)` -> min `(10,4,20)`, max `(13,8,23)`.
/// Corner order doesn't matter (per-axis min/max canonicalizes).
#[expect(
    clippy::cast_precision_loss,
    reason = "block coords are small non-negative ints, exact in f32"
)]
pub(crate) fn aabb_from_block_corners(a: IVec3, b: IVec3) -> Aabb {
    let lo = |p: i32, q: i32| p.min(q) as f32;
    let hi = |p: i32, q: i32| (p.max(q) + 1) as f32;
    Aabb {
        min: Vec3::new(lo(a.x, b.x), lo(a.y, b.y), lo(a.z, b.z)),
        max: Vec3::new(hi(a.x, b.x), hi(a.y, b.y), hi(a.z, b.z)),
    }
}

/// Default human-model collision size, mirroring `HumanModel_GetSize`'s
/// `Model_RetSize(8.6, 28.1, 8.6)` (`Model.c`): the per-axis size in
/// sixteenths of a block, at the default `ModelScale` of 1.0. This is the
/// *collision* size the engine writes to `Entity.Size` (via
/// `Entity_UpdateModelBounds`), not the taller picking AABB
/// (`HumanModel_GetBounds`, 32/16 high). Used as the feet-box extent
/// fallback when `Entity.Size` is still zero (model not yet loaded), so
/// detection has a sane box before the engine populates the real scaled
/// size. Written as `n / 16.0` rather than pre-divided decimals so it stays
/// bit-faithful to the source (dividing by a power of two is exact) and the
/// provenance is unmistakable.
pub(crate) const DEFAULT_PLAYER_SIZE: Vec3 = Vec3 {
    x: 8.6 / 16.0,
    y: 28.1 / 16.0,
    z: 8.6 / 16.0,
};

/// Feet-anchored world-space collision box from the feet position and the
/// model's already-scaled collision size (`Entity.Size`): X/Z centered on the
/// feet, Y from feet to feet+height. Matches the server's
/// `ModelBB.OffsetPosition` for walkthrough collision (engine `AABB_Make`).
pub(crate) fn player_bounds(feet: Vec3, size: Vec3) -> Aabb {
    Aabb {
        min: Vec3::new(feet.x - size.x * 0.5, feet.y, feet.z - size.z * 0.5),
        max: Vec3::new(
            feet.x + size.x * 0.5,
            feet.y + size.y,
            feet.z + size.z * 0.5,
        ),
    }
}

/// What semantic role a checkpoint plays in the run.
///
/// `Start` and `End` are positional: index 0 must be `Start`, the last
/// index must be `End`. Every middle checkpoint is one of `Split`,
/// `Pause`, or `Resume`. All middle kinds advance the split cursor
/// (`Command::Split`) on entry so the LiveSplit UI shows them as
/// segments; `Pause` additionally bumps the pause counter via
/// `pause_triggers::pause_add` and `Resume` drops it via `pause_sub`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckpointKind {
    Start,
    Split,
    Pause,
    Resume,
    End,
}

/// What causes a checkpoint to fire. `Aabb` is the position-driven
/// trigger polled by [`step`] each tick; `MapLoaded` matches the
/// engine's `World.Name` on `on_new_map_loaded` via
/// [`step_on_map_loaded`].
///
/// AABB scope is implicit: an `Aabb` checkpoint at index `i` belongs
/// to the section opened by the most recent `Trigger::MapLoaded` in
/// `track.checkpoints[..i]`, falling back to
/// `SplitsState.starting_map` when no `MapLoaded` precedes it. The
/// box only fires while the player's world matches that scope.
///
/// Pause/Resume checkpoints are AABB-only; the cross-map case is
/// expressed by placing a `Trigger::MapLoaded` checkpoint between
/// the `Pause` and `Resume` AABBs (the scope walk then derives that
/// the `Resume` AABB belongs to the post-transition map).
#[derive(Clone, Debug, PartialEq)]
pub enum Trigger {
    Aabb(Aabb),
    MapLoaded(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Checkpoint {
    pub kind: CheckpointKind,
    pub trigger: Trigger,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub name: String,
    pub checkpoints: Vec<Checkpoint>,
}

/// Target for `edit kind <i> ...` ([`SplitsState::set_kind`]): either an
/// AABB-trigger kind swap (`Split` / `Pause` / `Resume`, keeping the
/// existing zone) or a conversion to a zoneless `MapLoaded` transition
/// (dropping the zone; kind forced to `Split`).
#[derive(Clone, Debug)]
pub enum RetypeTarget {
    /// `edit kind <i> split|pause|resume`: keep the zone, swap the kind.
    /// Only `Split` / `Pause` / `Resume` are constructed by the caller;
    /// the boundary rejection in `set_kind` backstops a stray Start/End.
    Aabb(CheckpointKind),
    /// `edit kind <i> map [name]`: drop the zone, become a `MapLoaded`
    /// transition to `name`.
    Map(String),
}

/// Which boundary slot `edit kind <i> start|end` moves a checkpoint into.
/// Unlike [`RetypeTarget`], these are not in-place kind swaps: `Start` /
/// `End` are position-derived, so they map to a reorder via
/// [`SplitsState::move_to_boundary`] rather than [`SplitsState::set_kind`].
#[derive(Clone, Copy, Debug)]
pub enum Boundary {
    Start,
    End,
}

/// Short English name for a checkpoint kind, used in chat listings.
pub(crate) fn kind_name(kind: CheckpointKind) -> &'static str {
    match kind {
        CheckpointKind::Start => "Start",
        CheckpointKind::Split => "Split",
        CheckpointKind::Pause => "Pause",
        CheckpointKind::Resume => "Resume",
        CheckpointKind::End => "End",
    }
}

/// ClassiCube `&`-code whose hue matches `hud::boxes::color_for_kind`'s
/// `PackedCol` for this kind. The two hue tables are deliberately separate
/// (`PackedCol` vs `&`-code, different types); keep them in sync if a hue
/// ever changes. Used by [`format_splits`] and `hud::labels::kind_color_code`.
pub(crate) fn kind_color_code(kind: CheckpointKind) -> &'static str {
    match kind {
        CheckpointKind::Start => "&a",  // green  (0,255,0)
        CheckpointKind::Split => "&e",  // yellow (255,255,0)
        CheckpointKind::Pause => "&b",  // cyan   (0,200,255)
        CheckpointKind::Resume => "&6", // orange (255,140,0)
        CheckpointKind::End => "&c",    // red    (255,0,0)
    }
}

/// ClassiCube `&`-code for a `MapLoaded` map-transition row in the chat
/// listing. A `MapLoaded` checkpoint's `kind` is always `Split`, so without
/// this override the row would render the same yellow as a real `Split`.
/// Light purple -- distinct from every [`kind_color_code`] hue. HUD color
/// tables don't need a matching entry: map-transition checkpoints have no
/// AABB, so they never draw a box or label.
const MAP_COLOR_CODE: &str = "&d";

/// Render the loaded track as chat lines for `/client LiveSplit splits`: a
/// header line plus one line per checkpoint, in track order. `fired` comes
/// straight off [`SplitsState`]; `next_index` is `Some(i)` for the run's
/// next-target row (highlighted `&e> ... &e<`) or `None` to suppress the
/// marker entirely (edit mode -- there's no live run to target). `Aabb` rows
/// are colored by checkpoint kind (matching the HUD); `MapLoaded` rows use
/// [`MAP_COLOR_CODE`] (light purple) so they're visually distinct from
/// `Split` rows. The marker char conveys run status: `x` (fired) or blank
/// (pending) for non-next rows. The kind column shows the kind name for an
/// `Aabb` trigger or `Map` for a `MapLoaded` map-transition. Each row shows a
/// dim-gray detail in parens before the quoted label: `Aabb` rows show the
/// box's min corner and size `(x,y,z w,h,d)` (size, not max corner -- shorter,
/// and matches the `<min> <size>` wire form); `MapLoaded` rows show the
/// destination map name.
#[must_use]
pub(crate) fn format_splits(
    track: &Track,
    fired: &[bool],
    next_index: Option<usize>,
) -> Vec<String> {
    let total = track.checkpoints.len();
    let fired_count = fired.iter().filter(|b| **b).count();
    let mut lines = Vec::with_capacity(total + 1);
    lines.push(format!(
        "&aLiveSplit: track \"{}\" ({total} checkpoints, {fired_count} fired)",
        track.name
    ));
    for (i, cp) in track.checkpoints.iter().enumerate() {
        // Dim-gray detail in parens before the label, then re-emit the kind
        // color so the label keeps it: Aabb rows show the box's min corner and
        // size `(x,y,z w,h,d)` (size, not max corner -- shorter, matches the
        // wire `<min> <size>` form), colored by checkpoint kind; Map rows show
        // the destination map name in MAP_COLOR_CODE (light purple), distinct
        // from a Split row.
        let (code, kind_col, paren) = match &cp.trigger {
            Trigger::Aabb(aabb) => {
                let code = kind_color_code(cp.kind);
                let lo = vec3_to_u16(aabb.min);
                let hi = vec3_to_u16(aabb.max);
                let sz = [
                    hi[0].saturating_sub(lo[0]),
                    hi[1].saturating_sub(lo[1]),
                    hi[2].saturating_sub(lo[2]),
                ];
                (
                    code,
                    kind_name(cp.kind),
                    format!(
                        "&7({},{},{} {},{},{}) {code}",
                        lo[0], lo[1], lo[2], sz[0], sz[1], sz[2]
                    ),
                )
            }
            Trigger::MapLoaded(name) => {
                (MAP_COLOR_CODE, "Map", format!("&7({name}) {MAP_COLOR_CODE}"))
            }
        };
        let label = &cp.label;
        lines.push(if Some(i) == next_index {
            // Wrap the next-target row like the HUD label: `&e> {body} &e<`.
            format!("&e> #{i} {code}{kind_col} {paren}\"{label}\" &e<")
        } else {
            // Next wins over fired; marker char conveys status (x = fired, blank = pending).
            let marker = if fired.get(i).copied().unwrap_or(false) {
                'x'
            } else {
                ' '
            };
            format!("{code} {marker} #{i} {code}{kind_col} {paren}\"{label}\"")
        });
    }
    lines
}

#[derive(Debug, Default)]
pub struct SplitsState {
    pub track: Option<Track>,
    /// The world name captured at the moment the track was loaded.
    /// AABB checkpoints with `map: None` resolve against this — they
    /// only fire while the player is on the same map they loaded the
    /// track on.
    pub starting_map: Option<String>,
    /// Last map name observed by [`observe_map`]. Drives the
    /// edge-trigger for `step_on_map_loaded`: a `MapLoaded`
    /// checkpoint fires when this value transitions to a different
    /// map. Seeded from `starting_map` at load so the very first
    /// observation on the same map is a no-op. Independent from
    /// `starting_map` (which never changes after load) so the
    /// AABB-scope walk in `step()` stays stable across map changes.
    pub last_seen_map: Option<String>,
    pub next_index: usize,
    pub fired: Vec<bool>,
    pub last_inside: Vec<bool>,
}

impl SplitsState {
    pub fn load(&mut self, track: Track, starting_map: Option<String>) {
        let n = track.checkpoints.len();
        self.track = Some(track);
        self.last_seen_map = starting_map.clone();
        self.starting_map = starting_map;
        self.next_index = 0;
        self.fired = vec![false; n];
        self.last_inside = vec![false; n];
    }

    /// Re-arm the run without unloading the track. Called on
    /// `/client LiveSplit reset` and on `on_new_map_loaded`. Also
    /// zeroes the shared pause counter — a fresh attempt can't
    /// inherit a stuck pause from a previous abandoned run.
    pub fn rearm(&mut self) {
        self.next_index = 0;
        self.fired.fill(false);
        self.last_inside.fill(false);
        crate::plugin::pause_triggers::pause_clear_all();
    }

    /// Walk the run cursor back one checkpoint in response to a timer-side
    /// undo. Un-fires the checkpoint at `next_index - 1` and reverses its
    /// pause side-effect (a Pause that bumped the counter on entry is
    /// `pause_sub`'d; a Resume is `pause_add`'d). `last_inside[]` is left
    /// untouched: if the player is still standing inside that box, edge
    /// triggering keeps it from instantly re-firing until they exit and
    /// re-enter (matching LiveSplit's "undo, then re-cross to re-split").
    /// No-op when the run hasn't started (`next_index == 0`).
    ///
    /// Reaches `pause_triggers` directly, mirroring `rearm()` / `unload()`
    /// (which call `pause_clear_all()` the same way). The timer can't undo
    /// its first split (`CantUndoFirstSplit`), so `SplitUndone` never
    /// arrives while `next_index == 1`; the `== 0` guard is the defensive
    /// floor.
    pub fn undo_one(&mut self) {
        if self.next_index == 0 {
            return;
        }
        let i = self.next_index - 1;
        self.next_index = i;
        if let Some(f) = self.fired.get_mut(i) {
            *f = false;
        }
        if let Some(cp) = self.track.as_ref().and_then(|t| t.checkpoints.get(i)) {
            match cp.kind {
                CheckpointKind::Pause => crate::plugin::pause_triggers::pause_sub(),
                CheckpointKind::Resume => crate::plugin::pause_triggers::pause_add(),
                _ => {}
            }
        }
    }

    /// Whether `map_name` is part of the loaded track's map set --
    /// its `starting_map` or the target of any `Trigger::MapLoaded`
    /// checkpoint. Returns `false` when no track is loaded.
    pub fn includes_map(&self, map_name: &str) -> bool {
        if self.starting_map.as_deref() == Some(map_name) {
            return true;
        }
        self.track.as_ref().is_some_and(|t| {
            t.checkpoints
                .iter()
                .any(|cp| matches!(&cp.trigger, Trigger::MapLoaded(n) if n == map_name))
        })
    }

    /// Drop the loaded track and its per-checkpoint latches. After
    /// this, `step()` short-circuits (no `track`) until a new
    /// `load()`.
    pub fn unload(&mut self) {
        self.track = None;
        self.starting_map = None;
        self.last_seen_map = None;
        self.next_index = 0;
        self.fired.clear();
        self.last_inside.clear();
        // Mirror `rearm()`: dropping the track mid-pause shouldn't
        // leak counter state into whatever replaces it. Without this,
        // `/client LiveSplit clear` while a Pause AABB has fired
        // leaves the counter stuck non-zero.
        crate::plugin::pause_triggers::pause_clear_all();
    }

    /// Add a new `Split`/AABB checkpoint and return the index it
    /// landed at. `target` is `None` to append (just before `End`) or
    /// `Some(i)` to insert at a specific slot. On a populated track
    /// (>= 2 checkpoints) the insert position is clamped to
    /// `[0, n - 1]`: the upper bound keeps `End` as the last
    /// checkpoint; `Some(0)` inserts before the current `Start`,
    /// which `re_derive_kinds` then promotes to the new `Start` (the
    /// displaced former `Start` becomes a middle `Split`). Bootstrapping
    /// an empty/one-checkpoint track ignores `target` -- the first
    /// placement becomes index 0 (later re-derived to `Start`), the
    /// second appends as index 1 (re-derived to `End`). After the
    /// structural change the boundary kinds are re-derived and the
    /// per-checkpoint latches reallocated (the run re-arms to index 0).
    /// `Err` if no track is loaded.
    pub fn add_checkpoint(
        &mut self,
        aabb: Aabb,
        label: String,
        target: Option<usize>,
    ) -> Result<usize> {
        let idx = {
            let Some(track) = self.track.as_mut() else {
                bail!("no track loaded");
            };
            let n = track.checkpoints.len();
            let idx = if n < 2 {
                // Bootstrap: first placement -> 0, second -> 1.
                n
            } else {
                match target {
                    None => n - 1,
                    // Upper bound: End must stay last. Lower bound removed so
                    // Some(0) inserts before Start; re_derive_kinds promotes it.
                    Some(i) => i.min(n - 1),
                }
            };
            track.checkpoints.insert(
                idx,
                Checkpoint {
                    kind: CheckpointKind::Split,
                    trigger: Trigger::Aabb(aabb),
                    label,
                },
            );
            idx
        };
        self.re_derive_kinds();
        self.reset_after_structural_change();
        Ok(idx)
    }

    /// Remove the checkpoint at index `i`. Refuses if the track would
    /// drop below 2 checkpoints (Start + End minimum, mirroring the
    /// `n >= 2` floor the payload serializer enforces). Re-derives the
    /// boundary kinds and reallocates the latches afterward. `Err` if
    /// no track is loaded or `i` is out of range.
    pub fn remove_checkpoint(&mut self, i: usize) -> Result<()> {
        {
            let Some(track) = self.track.as_mut() else {
                bail!("no track loaded");
            };
            let n = track.checkpoints.len();
            if i >= n {
                bail!("checkpoint index {i} out of range (track has {n})");
            }
            if n <= 2 {
                bail!("cannot remove: a track needs at least 2 checkpoints (Start + End)");
            }
            track.checkpoints.remove(i);
        }
        self.re_derive_kinds();
        self.reset_after_structural_change();
        Ok(())
    }

    /// Move the checkpoint at `from` to index `to`, shifting the
    /// checkpoints between them. Equivalent to `remove(from)` then
    /// `insert(to, cp)`: the moved checkpoint ends at final index `to`
    /// and everything from `to` onward shifts up by one. Boundary slots
    /// are **not** locked -- after the shift the kinds are re-derived
    /// (`re_derive_kinds`: idx 0 -> `Start`, last -> `End`, a former
    /// boundary stranded in the middle demoted to `Split`), so a middle
    /// checkpoint moved to a boundary adopts the boundary kind and a
    /// former `Start`/`End` moved inward becomes a `Split`. Reordering can
    /// invert a `Pause`/`Resume` pair, so the post-move track is validated
    /// against [`validate_pause_resume_pairing`]; on failure the move is
    /// rolled back and `Err` returned (unlike `insert`/`remove`, which
    /// defer pairing checks to the load-entry gates -- reordering is the
    /// operation most likely to invert a pair). On success the latches are
    /// reallocated and the run re-armed to index 0. `Err` if no track is
    /// loaded or either index is out of range.
    pub fn move_checkpoint(&mut self, from: usize, to: usize) -> Result<()> {
        let snapshot = {
            let Some(track) = self.track.as_mut() else {
                bail!("no track loaded");
            };
            let n = track.checkpoints.len();
            if from >= n {
                bail!("from index {from} out of range (track has {n})");
            }
            if to >= n {
                bail!("to index {to} out of range (track has {n})");
            }
            let snapshot = track.checkpoints.clone();
            let cp = track.checkpoints.remove(from);
            track.checkpoints.insert(to, cp);
            snapshot
        };
        self.re_derive_kinds();
        // Validate the reordered track before committing the run reset.
        // `validate_pause_resume_pairing` returns an owned error (no
        // borrow of `track`), so the immutable borrow ends before the
        // mutable rollback borrow below.
        let pairing = self.track.as_ref().map(validate_pause_resume_pairing);
        if let Some(Err(e)) = pairing {
            // Roll back to the pre-move order + kinds. The cursor and
            // latches are untouched -- the reset happens only on success.
            if let Some(track) = self.track.as_mut() {
                track.checkpoints = snapshot;
            }
            bail!("{e}");
        }
        self.reset_after_structural_change();
        Ok(())
    }

    /// `edit kind <i> start|end`: move checkpoint `from` to a boundary slot.
    /// `Start` targets index 0; `End` targets the last index. Delegates to
    /// [`Self::move_checkpoint`], which re-derives the moved checkpoint's
    /// kind to `Start` / `End`, demotes the displaced former boundary to
    /// `Split`, validates pause/resume pairing (rolling back on inversion),
    /// reallocates the latches, and re-arms the run. Returns `Ok(false)` as
    /// a no-op when the checkpoint is already at that boundary (so the
    /// caller can skip the timer reset); `Ok(true)` when a move happened.
    /// `Err` if no track is loaded or `from` is out of range.
    pub fn move_to_boundary(&mut self, from: usize, which: Boundary) -> Result<bool> {
        let n = {
            let Some(track) = self.track.as_ref() else {
                bail!("no track loaded");
            };
            track.checkpoints.len()
        };
        if from >= n {
            bail!("checkpoint index {from} out of range (track has {n})");
        }
        let to = match which {
            Boundary::Start => 0,
            Boundary::End => n - 1,
        };
        if from == to {
            return Ok(false);
        }
        self.move_checkpoint(from, to)?;
        Ok(true)
    }

    /// Rename the loaded track (`edit rename`). Non-structural: the
    /// checkpoint sequence, latch lengths, and run cursor are all untouched
    /// (no re-arm). `Err` if no track is loaded.
    pub fn set_name(&mut self, name: String) -> Result<()> {
        let Some(track) = self.track.as_mut() else {
            bail!("no track loaded");
        };
        track.name = name;
        Ok(())
    }

    /// Relabel the checkpoint at index `i`. Non-structural: the kind
    /// sequence, latch lengths, and run cursor are all untouched (no
    /// re-arm). `Err` if no track is loaded or `i` is out of range.
    pub fn set_label(&mut self, i: usize, text: String) -> Result<()> {
        let Some(track) = self.track.as_mut() else {
            bail!("no track loaded");
        };
        let n = track.checkpoints.len();
        let Some(cp) = track.checkpoints.get_mut(i) else {
            bail!("checkpoint index {i} out of range (track has {n})");
        };
        cp.label = text;
        Ok(())
    }

    /// Replace the AABB of the existing checkpoint at `i` (`edit redraw`),
    /// keeping its kind / label / position. Non-structural (no kind
    /// re-derive, no latch realloc), but the changed geometry invalidates a
    /// run in progress, so the cursor + latches re-arm to 0 via `rearm()`.
    /// `Err` if no track is loaded, `i` is out of range, or `i` is a
    /// `MapLoaded` (map-transition) checkpoint, which has no zone.
    pub fn set_trigger(&mut self, i: usize, aabb: Aabb) -> Result<()> {
        {
            let Some(track) = self.track.as_mut() else {
                bail!("no track loaded");
            };
            let n = track.checkpoints.len();
            let Some(cp) = track.checkpoints.get_mut(i) else {
                bail!("checkpoint index {i} out of range (track has {n})");
            };
            if matches!(cp.trigger, Trigger::MapLoaded(_)) {
                bail!("checkpoint #{i} is a map transition; only zone checkpoints can be redrawn");
            }
            cp.trigger = Trigger::Aabb(aabb);
        }
        self.rearm();
        Ok(())
    }

    /// Retype the **middle** checkpoint at `i` (`edit kind <i> ...`).
    /// Non-structural (the list length is unchanged, so no latch realloc
    /// and no `re_derive_kinds`), but the kind / scope change invalidates
    /// an in-progress run, so it `rearm()`s like [`set_trigger`].
    ///
    /// Two target shapes:
    /// - [`RetypeTarget::Aabb`] swaps only the kind, keeping the existing
    ///   zone. Rejected when `i` is currently a `MapLoaded` checkpoint (no
    ///   zone to keep -- remove + re-add is the path).
    /// - [`RetypeTarget::Map`] converts to a zoneless `Trigger::MapLoaded`
    ///   (dropping any zone) and forces the kind to `Split` (a middle
    ///   MapLoaded is always a Split). On an already-`MapLoaded`
    ///   checkpoint this just renames the destination -- the only way to
    ///   edit a transition's map name short of remove + re-add.
    ///
    /// Pause/Resume pairing is **not** validated here: like
    /// `add_checkpoint` / `remove_checkpoint`, a retype may leave the
    /// track temporarily unbalanced (e.g. a lone Pause while the matching
    /// Resume is still being placed), so building a pause window
    /// one checkpoint at a time isn't blocked. The full
    /// [`validate_pause_resume_pairing`] runs at the save/load gates and
    /// rejects a track that's still unbalanced. (Unlike `move_checkpoint`,
    /// which validates eagerly because a reorder never changes Pause /
    /// Resume *counts*.)
    ///
    /// `Err` if no track is loaded, `i` is out of range, `i` is a boundary
    /// (Start / End), or an `Aabb` retype targets a `MapLoaded` checkpoint.
    pub fn set_kind(&mut self, i: usize, target: RetypeTarget) -> Result<()> {
        {
            let Some(track) = self.track.as_mut() else {
                bail!("no track loaded");
            };
            let n = track.checkpoints.len();
            if i >= n {
                bail!("checkpoint index {i} out of range (track has {n})");
            }
            if i == 0 || i + 1 == n {
                bail!(
                    "checkpoint #{i} is a boundary (Start/End); only middle checkpoints can be \
                     retyped"
                );
            }
            let cp = &mut track.checkpoints[i];
            match target {
                RetypeTarget::Aabb(kind) => {
                    if matches!(cp.trigger, Trigger::MapLoaded(_)) {
                        bail!(
                            "checkpoint #{i} is a map transition (has no zone); remove it and add \
                             a new zone checkpoint instead"
                        );
                    }
                    cp.kind = kind;
                }
                RetypeTarget::Map(name) => {
                    cp.trigger = Trigger::MapLoaded(name);
                    cp.kind = CheckpointKind::Split;
                }
            }
        }
        self.rearm();
        Ok(())
    }

    /// Force the boundary kinds after a structural mutation: index 0 ->
    /// `Start`, last -> `End`. Legitimately-middle `Split`/`Pause`/
    /// `Resume`/`MapLoaded` kinds are left untouched, so author-/load-
    /// defined checkpoints survive an edit elsewhere in the list. The one
    /// exception: a `Start`/`End` kind stranded at a middle index is
    /// demoted to `Split` -- only a reorder (`move_checkpoint`) can shift
    /// a former boundary inward; for `insert`/`remove` that demotion loop
    /// is a no-op (they never move a boundary into the middle).
    fn re_derive_kinds(&mut self) {
        let Some(track) = self.track.as_mut() else {
            return;
        };
        let n = track.checkpoints.len();
        if n == 0 {
            return;
        }
        track.checkpoints[0].kind = expected_kind(0, n);
        track.checkpoints[n - 1].kind = expected_kind(n - 1, n);
        for i in 1..n.saturating_sub(1) {
            if matches!(
                track.checkpoints[i].kind,
                CheckpointKind::Start | CheckpointKind::End
            ) {
                track.checkpoints[i].kind = CheckpointKind::Split;
            }
        }
    }

    /// `load()`-shaped reset for a structural mutation: a changed
    /// `checkpoints.len()` means `fired[]`/`last_inside[]` must be
    /// **reallocated** to the new length, not `fill`'d (which `rearm()`
    /// does, assuming the lengths already match). Re-arms the cursor to
    /// 0 and drains the pause counter so an edit mid-pause can't strand
    /// a stuck pause.
    fn reset_after_structural_change(&mut self) {
        let n = self.track.as_ref().map_or(0, |t| t.checkpoints.len());
        self.next_index = 0;
        self.fired = vec![false; n];
        self.last_inside = vec![false; n];
        crate::plugin::pause_triggers::pause_clear_all();
    }
}

/// Positional kind for the boundary slots only. Called solely for index
/// 0 (-> `Start`) and the last index (-> `End`) by
/// `SplitsState::re_derive_kinds`; middle kinds are author-/load-defined
/// and preserved across edits. NOT the same as the payload module's
/// `kind_valid_at` (a permissive validator that accepts Split/Pause/
/// Resume in the middle).
pub(crate) fn expected_kind(i: usize, n: usize) -> CheckpointKind {
    if i == 0 {
        CheckpointKind::Start
    } else if i + 1 == n {
        CheckpointKind::End
    } else {
        CheckpointKind::Split
    }
}

/// Walk the checkpoint list and assert that `Pause` / `Resume`
/// checkpoints form a well-balanced sequence: the running balance never
/// goes negative (no `Resume` without a preceding unmatched `Pause`),
/// and the balance hits zero at `End` (a run can't terminate mid-pause).
/// A track that violates either rule can strand the player game-time
/// paused with no in-game escape short of `/client LiveSplit resume` or
/// `reset`, so every track-entry gate (encoder, decoder finalization,
/// `splits::load_*`) calls this before adopting the track.
///
/// Nesting (`Pause`, `Pause`, `Resume`, `Resume`) is accepted: the
/// pause counter survives the inner pair as a no-op on `PauseGameTime`
/// / `ResumeGameTime` emission. Structural invariants other than
/// pairing (Pause/Resume must be AABB-only, position-implicit kind
/// sequence) stay in `encode_for_chat`.
pub fn validate_pause_resume_pairing(track: &Track) -> Result<()> {
    let mut balance: i32 = 0;
    for (i, cp) in track.checkpoints.iter().enumerate() {
        match cp.kind {
            CheckpointKind::Pause => balance += 1,
            CheckpointKind::Resume => {
                balance -= 1;
                if balance < 0 {
                    bail!(
                        "checkpoint[{i}] is Resume but no preceding unmatched Pause (balance \
                         would go negative)"
                    );
                }
            }
            CheckpointKind::End => {
                if balance != 0 {
                    bail!(
                        "track ends with {balance} unmatched Pause checkpoint(s); add a Resume \
                         before End"
                    );
                }
            }
            CheckpointKind::Start | CheckpointKind::Split => {}
        }
    }
    Ok(())
}

/// Pure decision function: given the current state, the player's
/// feet-anchored model collision box for this frame, and the engine's
/// current `World.Name`, advance the state and emit LiveSplit commands
/// via `send`. Pause/Resume kinds additionally invoke `on_pause` /
/// `on_resume` (wired to the `pause_triggers` counter at call sites).
///
/// Rules:
/// - Message-block collision parity: a checkpoint fires when the player's
///   model collision box (`player_box`, built by [`player_bounds`] from the
///   live feet position + `Entity.Size`) overlaps the checkpoint AABB
///   ([`Aabb::intersects`], closed), exactly like the server's
///   `PlayerPhysics.Walkthrough`.
/// - Edge-triggered: a checkpoint only fires on the outside-to-inside
///   transition, not while the player stands still inside it.
/// - Sequential: only `track.checkpoints[next_index]` is eligible to fire as
///   a Split / Pause / Resume / End. A Start box is always eligible
///   (entering one re-arms the run latch).
/// - One-shot: each box's `fired[i]` latches true until `rearm`.
/// - Map-scoped: an `Aabb` cp's scope is the world named by the most
///   recent preceding `Trigger::MapLoaded` in the checkpoint list, or
///   `state.starting_map` if none precedes it. The box only fires
///   while `world == Some(scope)`; if either side is `None` the cp is
///   skipped.
pub fn step<F: FnMut(Command), P: FnMut(), R: FnMut()>(
    state: &mut SplitsState,
    player_box: Aabb,
    world: Option<&str>,
    mut send: F,
    mut on_pause: P,
    mut on_resume: R,
) {
    let Some(track) = state.track.as_ref() else {
        return;
    };

    let mut current_map: Option<&str> = state.starting_map.as_deref();
    let inside_now: Vec<bool> = track
        .checkpoints
        .iter()
        .map(|cp| match &cp.trigger {
            Trigger::Aabb(aabb) => match (current_map, world) {
                (Some(t), Some(w)) if t == w => aabb.intersects(&player_box),
                _ => false,
            },
            Trigger::MapLoaded(name) => {
                current_map = Some(name.as_str());
                false
            }
        })
        .collect();

    for (i, cp) in track.checkpoints.iter().enumerate() {
        if !matches!(cp.trigger, Trigger::Aabb(_)) {
            continue;
        }
        let entered = inside_now[i] && !state.last_inside[i];
        if !entered {
            continue;
        }

        match cp.kind {
            CheckpointKind::Start => {
                // Inline re-arm of all per-run state. Can't call
                // `state.rearm()` here because `track` is held as an
                // immutable borrow into `state`; field-level
                // assignments work via split borrows but methods
                // don't. The post-loop write of `inside_now` to
                // `last_inside` overrides the zeroed edge state for
                // this same tick (so a player standing inside Start
                // at fire time doesn't re-trigger on the next tick).
                state.fired.iter_mut().for_each(|b| *b = false);
                state.last_inside.iter_mut().for_each(|b| *b = false);
                crate::plugin::pause_triggers::pause_clear_all();
                state.fired[i] = true;
                state.next_index = i + 1;
                send(Command::SetCurrentTimingMethod {
                    timing_method: TimingMethod::GameTime,
                });
                send(Command::Start);
                send(Command::InitializeGameTime);
            }
            CheckpointKind::Split | CheckpointKind::End
                if i == state.next_index && !state.fired[i] =>
            {
                state.fired[i] = true;
                state.next_index = i + 1;
                send(Command::Split);
            }
            CheckpointKind::Pause if i == state.next_index && !state.fired[i] => {
                state.fired[i] = true;
                state.next_index = i + 1;
                send(Command::Split);
                on_pause();
            }
            CheckpointKind::Resume if i == state.next_index && !state.fired[i] => {
                state.fired[i] = true;
                state.next_index = i + 1;
                send(Command::Split);
                on_resume();
            }
            _ => {}
        }
    }

    state.last_inside = inside_now;
}

/// AABB checkpoints whose derived map scope matches `world`, paired
/// with their kind, label, and an "is the next eligible checkpoint"
/// flag, in checkpoint order. Mirrors the scope walk in [`step`]: a
/// running `current_map` seeded from `starting_map`, advanced by each
/// `Trigger::MapLoaded`; an `Aabb` is in scope only while
/// `current_map == Some(world)`. Used by the in-world HUD to draw just
/// the boxes (and their floating labels) relevant to the player's
/// current map. Empty when `world` is `None`, or when no in-scope
/// concrete map matches it.
///
/// The final `bool` is `next_index == Some(i)` for the checkpoint's
/// **track-wide** index `i` (not its position in the filtered output),
/// so the HUD can highlight the one checkpoint the run cursor points
/// at. Because the comparison is on the source index inside the same
/// scope walk, a duplicate-geometry AABB on a different map section is
/// never mis-flagged. Pass `None` to flag nothing (e.g. no run cursor
/// to highlight); a `Some(i)` that lands on a `Trigger::MapLoaded`
/// checkpoint or an off-map AABB simply matches no returned entry.
#[must_use]
pub fn aabbs_on_map(
    track: &Track,
    starting_map: Option<&str>,
    world: Option<&str>,
    next_index: Option<usize>,
) -> Vec<(usize, CheckpointKind, Aabb, String, bool)> {
    let mut current_map = starting_map;
    let mut out = Vec::new();
    for (i, cp) in track.checkpoints.iter().enumerate() {
        match &cp.trigger {
            Trigger::Aabb(aabb) => {
                if let (Some(t), Some(w)) = (current_map, world)
                    && t == w
                {
                    out.push((i, cp.kind, *aabb, cp.label.clone(), next_index == Some(i)));
                }
            }
            Trigger::MapLoaded(name) => current_map = Some(name.as_str()),
        }
    }
    out
}

/// Index at which a bare `edit add` (no explicit target) should insert:
/// the end of the section whose map name matches `world` -- just before
/// that section's terminating `Trigger::MapLoaded` -- so a new checkpoint
/// lands at the end of the map the player is currently standing on. Falls
/// back to `n - 1` (append before `End`, the prior behavior) when the
/// matched section is the last one (runs straight to `End`) or `world`
/// matches no section. Mirrors the implicit-scope walk in [`aabbs_on_map`]:
/// seed `current_map` from `starting_map`, advance on each `MapLoaded`.
///
/// First-match on a route that revisits a map name (`A -> B -> A`): the
/// author uses an explicit `add <i>` to target a later instance, since
/// there's no reliable in-world signal to pick between same-named sections
/// during authoring (the run cursor is `0` whenever no run is in progress).
#[must_use]
pub(crate) fn append_index_for_section(
    checkpoints: &[Checkpoint],
    starting_map: Option<&str>,
    world: Option<&str>,
) -> usize {
    let n = checkpoints.len();
    // `in_target` tracks whether the section currently being walked matches
    // `world`: seeded from `starting_map`, re-evaluated against each
    // `MapLoaded`'s name as the walk advances.
    let mut in_target = matches!((starting_map, world), (Some(t), Some(w)) if t == w);
    for (i, cp) in checkpoints.iter().enumerate() {
        if let Trigger::MapLoaded(name) = &cp.trigger {
            if in_target {
                // This MapLoaded closes the matched section: insert before it.
                return i;
            }
            in_target = world == Some(name.as_str());
        }
    }
    // Matched section is last, or no match: append before End.
    n.saturating_sub(1)
}

/// Edge-trigger wrapper over [`step_on_map_loaded`] for tick-driven
/// map-change detection. Compares the freshly-observed map name
/// against `state.last_seen_map`; on a transition to a different
/// `Some(name)` fires `step_on_map_loaded` and latches
/// `last_seen_map`. `None` observations are ignored so transient
/// gaps (e.g. between `World.Name = ""` and the tab-list group
/// catching up after a server-driven map change) don't reset the
/// edge.
///
/// Driven from the per-tick poll in `SplitsModule` rather than from
/// `on_new_map_loaded` because on multiplayer the engine raises
/// `MapLoaded` before the server's `ExtAddPlayerName` updates the
/// local player's tab-list group, so the map name at the event is
/// stale. Polling defers the observation until both signals (engine
/// + protocol) have settled.
pub fn observe_map<F: FnMut(Command)>(state: &mut SplitsState, world: Option<&str>, send: F) {
    let Some(name) = world else {
        return;
    };
    if state.last_seen_map.as_deref() == Some(name) {
        return;
    }
    state.last_seen_map = Some(name.to_owned());
    step_on_map_loaded(state, name, send);
}

/// Map-load counterpart of [`step`]. Sequential and one-shot rules
/// match [`step`]: a Start always re-arms; Split/End only fire when
/// `i == next_index`. Pause/Resume kinds are AABB-only and are
/// ignored by this function (a `MapLoaded` trigger never carries a
/// Pause/Resume kind in a valid track).
///
/// Off-route guard: if `map_name` is neither `state.starting_map` nor
/// the target of any `Trigger::MapLoaded` in the track, the player has
/// warped outside the track's map set. When a run is in progress
/// (`0 < next_index < n`) we emit `Command::Reset` and re-arm the run
/// locally; otherwise (pre-Start or post-End) we leave state alone so
/// the player can come back and start fresh.
pub fn step_on_map_loaded<F: FnMut(Command)>(state: &mut SplitsState, map_name: &str, mut send: F) {
    let Some(track) = state.track.as_ref() else {
        return;
    };

    let on_route = state.includes_map(map_name);
    if !on_route {
        let n = state.fired.len();
        let in_progress = state.next_index > 0 && state.next_index < n;
        if in_progress {
            send(Command::Reset { save_attempt: None });
            state.rearm();
        }
        return;
    }

    for (i, cp) in track.checkpoints.iter().enumerate() {
        let Trigger::MapLoaded(name) = &cp.trigger else {
            continue;
        };
        if name != map_name {
            continue;
        }

        match cp.kind {
            CheckpointKind::Start => {
                // Inline re-arm (see same comment in `step`). Borrow
                // checker rejects `state.rearm()` while `track` is
                // held immutably.
                state.fired.iter_mut().for_each(|b| *b = false);
                state.last_inside.iter_mut().for_each(|b| *b = false);
                crate::plugin::pause_triggers::pause_clear_all();
                state.fired[i] = true;
                state.next_index = i + 1;
                send(Command::SetCurrentTimingMethod {
                    timing_method: TimingMethod::GameTime,
                });
                send(Command::Start);
                send(Command::InitializeGameTime);
            }
            CheckpointKind::Split | CheckpointKind::End
                if i == state.next_index && !state.fired[i] =>
            {
                state.fired[i] = true;
                state.next_index = i + 1;
                send(Command::Split);
            }
            // Pause/Resume kinds are AABB-only; a MapLoaded trigger
            // never carries them in a valid track. Skip rather than
            // panic so an out-of-spec track loaded from chat or disk
            // just no-ops instead of crashing the tick.
            CheckpointKind::Pause | CheckpointKind::Resume => {}
            _ => {}
        }
    }
}
