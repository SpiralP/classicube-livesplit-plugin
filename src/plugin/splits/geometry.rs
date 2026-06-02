#[cfg(test)]
mod tests;

use anyhow::{Result, bail};
use classicube_sys::Vec3;

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
/// with their kind and label, in checkpoint order. Mirrors the scope
/// walk in [`step`]: a running `current_map` seeded from
/// `starting_map`, advanced by each `Trigger::MapLoaded`; an `Aabb` is
/// in scope only while `current_map == Some(world)`. Used by the
/// in-world HUD to draw just the boxes (and their floating labels)
/// relevant to the player's current map. Empty when `world` is `None`,
/// or when no in-scope concrete map matches it.
#[must_use]
pub fn aabbs_on_map(
    track: &Track,
    starting_map: Option<&str>,
    world: Option<&str>,
) -> Vec<(CheckpointKind, Aabb, String)> {
    let mut current_map = starting_map;
    let mut out = Vec::new();
    for cp in &track.checkpoints {
        match &cp.trigger {
            Trigger::Aabb(aabb) => {
                if let (Some(t), Some(w)) = (current_map, world)
                    && t == w
                {
                    out.push((cp.kind, *aabb, cp.label.clone()));
                }
            }
            Trigger::MapLoaded(name) => current_map = Some(name.as_str()),
        }
    }
    out
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

    let on_route = state.starting_map.as_deref() == Some(map_name)
        || track
            .checkpoints
            .iter()
            .any(|cp| matches!(&cp.trigger, Trigger::MapLoaded(n) if n == map_name));
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
