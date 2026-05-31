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

    /// Half-open containment: `min <= p < max` per axis. Adjacent boxes that
    /// share a face will not both fire when the player straddles them.
    #[must_use]
    pub fn contains(&self, p: Vec3) -> bool {
        p.x >= self.min.x
            && p.x < self.max.x
            && p.y >= self.min.y
            && p.y < self.max.y
            && p.z >= self.min.z
            && p.z < self.max.z
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckpointKind {
    Start,
    Split,
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
    /// `/client LiveSplit reset` and on `on_new_map_loaded`.
    pub fn rearm(&mut self) {
        self.next_index = 0;
        self.fired.fill(false);
        self.last_inside.fill(false);
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
    }
}

/// Pure decision function: given the current state, the player position
/// for this frame, and the engine's current `World.Name`, advance the
/// state and emit LiveSplit commands via `send`.
///
/// Rules:
/// - Edge-triggered: a checkpoint only fires on the outside-to-inside
///   transition, not while the player stands still inside it.
/// - Sequential: only `track.checkpoints[next_index]` is eligible to fire as
///   a Split / End. A Start box is always eligible (entering one re-arms
///   the run latch).
/// - One-shot: each box's `fired[i]` latches true until `rearm`.
/// - Map-scoped: an `Aabb` cp's scope is the world named by the most
///   recent preceding `Trigger::MapLoaded` in the checkpoint list, or
///   `state.starting_map` if none precedes it. The box only fires
///   while `world == Some(scope)`; if either side is `None` the cp is
///   skipped.
pub fn step<F: FnMut(Command)>(
    state: &mut SplitsState,
    pos: Vec3,
    world: Option<&str>,
    mut send: F,
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
                (Some(t), Some(w)) if t == w => aabb.contains(pos),
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
                state.fired.iter_mut().for_each(|b| *b = false);
                state.fired[i] = true;
                state.next_index = i + 1;
                send(Command::SetCurrentTimingMethod {
                    timing_method: TimingMethod::GameTime,
                });
                send(Command::Start);
            }
            CheckpointKind::Split | CheckpointKind::End
                if i == state.next_index && !state.fired[i] =>
            {
                state.fired[i] = true;
                state.next_index = i + 1;
                send(Command::Split);
            }
            _ => {}
        }
    }

    state.last_inside = inside_now;
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
/// `i == next_index`.
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
                state.fired.iter_mut().for_each(|b| *b = false);
                state.fired[i] = true;
                state.next_index = i + 1;
                send(Command::SetCurrentTimingMethod {
                    timing_method: TimingMethod::GameTime,
                });
                send(Command::Start);
            }
            CheckpointKind::Split | CheckpointKind::End
                if i == state.next_index && !state.fired[i] =>
            {
                state.fired[i] = true;
                state.next_index = i + 1;
                send(Command::Split);
            }
            _ => {}
        }
    }
}
