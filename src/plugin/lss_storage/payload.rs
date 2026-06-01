#[cfg(test)]
mod tests;

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::plugin::splits::geometry::{
    Checkpoint, CheckpointKind, Track, Trigger, aabb_from_min_size, aabb_to_min_size,
};

const SCHEMA_VERSION: u32 = 1;

/// Name of the `<CustomVariable>` inside the `.lss` XML that holds
/// the canonical payload bytes. Shared by the reader (lookup) and
/// writer (build + dedup compare).
pub const CUSTOM_VAR_NAME: &str = "ClassiCubeTrack";

/// Canonical JSON form of a `Track`, stored as the
/// `ClassiCubeTrack` custom variable inside the `.lss` file.
/// Schema-versioned so future plugin builds detect unknown payloads
/// instead of misparsing.
///
/// Field declaration order is canonical: serde preserves it in
/// JSON output, and we serialize compactly (no pretty-printing) so
/// byte-equality is the comparison key for the writer's dedup gate.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Payload {
    pub v: u32,
    pub name: String,
    pub checkpoints: Vec<PayloadCheckpoint>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct PayloadCheckpoint {
    pub kind: PayloadKind,
    pub trigger: PayloadTrigger,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PayloadKind {
    Start,
    Split,
    Pause,
    Resume,
    End,
}

/// Wire form of a `Trigger`. Externally-tagged enum so the JSON
/// reads as `{"aabb": {...}}` or `{"map": "<name>"}`. AABBs use the
/// same quantized `[u16; 3]` min + `[u8; 3]` size encoding as the
/// chat protocol, so a track encoded for chat and one persisted to
/// disk round-trip identically.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PayloadTrigger {
    Aabb { min: [u16; 3], size: [u8; 3] },
    Map(String),
}

/// Convert a runtime `Track` to its canonical compact-JSON byte
/// representation. Errors if any AABB extent exceeds 255 blocks or
/// the position-implicit `CheckpointKind` sequence is violated
/// (index 0 must be `Start`, last must be `End`, middle must be
/// `Split` / `Pause` / `Resume`).
pub fn serialize_canonical(track: &Track) -> Result<Vec<u8>> {
    let payload = track_to_payload(track)?;
    let bytes = serde_json::to_vec(&payload)?;
    Ok(bytes)
}

fn track_to_payload(track: &Track) -> Result<Payload> {
    let n = track.checkpoints.len();
    ensure!(
        n >= 2,
        "track has {n} checkpoint(s); need at least 2 (Start + End)"
    );
    ensure!(!track.name.trim().is_empty(), "track name is empty");

    let mut checkpoints = Vec::with_capacity(n);
    for (i, cp) in track.checkpoints.iter().enumerate() {
        if !kind_valid_at(i, n, cp.kind) {
            bail!(
                "checkpoint[{i}] kind is {:?}; expected Start at index 0, End at last index, and \
                 Split/Pause/Resume in between",
                cp.kind
            );
        }
        if matches!(cp.kind, CheckpointKind::Pause | CheckpointKind::Resume)
            && !matches!(cp.trigger, Trigger::Aabb(_))
        {
            bail!(
                "checkpoint[{i}] is {:?} kind but trigger is not AABB; Pause/Resume kinds are \
                 AABB-only",
                cp.kind
            );
        }
        let trigger = match &cp.trigger {
            Trigger::Aabb(aabb) => {
                let (min, size) = aabb_to_min_size(*aabb)?;
                PayloadTrigger::Aabb { min, size }
            }
            Trigger::MapLoaded(name) => {
                ensure!(!name.trim().is_empty(), "checkpoint[{i}] map name is empty");
                PayloadTrigger::Map(name.clone())
            }
        };
        checkpoints.push(PayloadCheckpoint {
            kind: kind_to_payload(cp.kind),
            trigger,
        });
    }

    Ok(Payload {
        v: SCHEMA_VERSION,
        name: track.name.clone(),
        checkpoints,
    })
}

fn kind_valid_at(i: usize, n: usize, k: CheckpointKind) -> bool {
    if i == 0 {
        k == CheckpointKind::Start
    } else if i + 1 == n {
        k == CheckpointKind::End
    } else {
        matches!(
            k,
            CheckpointKind::Split | CheckpointKind::Pause | CheckpointKind::Resume
        )
    }
}

fn kind_to_payload(k: CheckpointKind) -> PayloadKind {
    match k {
        CheckpointKind::Start => PayloadKind::Start,
        CheckpointKind::Split => PayloadKind::Split,
        CheckpointKind::Pause => PayloadKind::Pause,
        CheckpointKind::Resume => PayloadKind::Resume,
        CheckpointKind::End => PayloadKind::End,
    }
}

fn kind_from_payload(k: &PayloadKind) -> CheckpointKind {
    match k {
        PayloadKind::Start => CheckpointKind::Start,
        PayloadKind::Split => CheckpointKind::Split,
        PayloadKind::Pause => CheckpointKind::Pause,
        PayloadKind::Resume => CheckpointKind::Resume,
        PayloadKind::End => CheckpointKind::End,
    }
}

/// Parse a canonical payload. Rejects unknown schema versions so
/// older plugin builds notice future format bumps instead of
/// silently misreading them.
pub fn parse(bytes: &[u8]) -> Result<Payload> {
    let payload: Payload = serde_json::from_slice(bytes)?;
    if payload.v != SCHEMA_VERSION {
        bail!(
            "unknown payload schema version {} (expected {})",
            payload.v,
            SCHEMA_VERSION
        );
    }
    Ok(payload)
}

/// Build a runtime `Track` from a parsed payload + a list of
/// per-checkpoint labels (read from the `.lss`'s `<Segments>` list
/// in order). Empty labels get a generated `"split <i>"` placeholder
/// so the runtime invariant "label is non-empty" holds.
pub fn into_track(payload: Payload, labels: Vec<String>) -> Result<Track> {
    let n = payload.checkpoints.len();
    ensure!(
        n >= 2,
        "payload has {n} checkpoint(s); need at least 2 (Start + End)"
    );
    ensure!(
        labels.len() == n,
        "label count {} doesn't match checkpoint count {n}",
        labels.len()
    );

    let mut checkpoints = Vec::with_capacity(n);
    for (i, (pcp, label)) in payload.checkpoints.into_iter().zip(labels).enumerate() {
        let parsed_kind = kind_from_payload(&pcp.kind);
        if !kind_valid_at(i, n, parsed_kind) {
            bail!(
                "checkpoint[{i}] kind is {parsed_kind:?}; expected Start at index 0, End at last \
                 index, and Split/Pause/Resume in between"
            );
        }
        let trigger = match pcp.trigger {
            PayloadTrigger::Aabb { min, size } => Trigger::Aabb(aabb_from_min_size(min, size)),
            PayloadTrigger::Map(name) => {
                ensure!(!name.trim().is_empty(), "checkpoint[{i}] map name is empty");
                Trigger::MapLoaded(name)
            }
        };
        if matches!(parsed_kind, CheckpointKind::Pause | CheckpointKind::Resume)
            && !matches!(trigger, Trigger::Aabb(_))
        {
            bail!(
                "checkpoint[{i}] is {parsed_kind:?} kind but trigger is not AABB; Pause/Resume \
                 kinds are AABB-only"
            );
        }
        let label = if label.trim().is_empty() {
            format!("split {i}")
        } else {
            label
        };
        checkpoints.push(Checkpoint {
            kind: parsed_kind,
            trigger,
            label,
        });
    }

    Ok(Track {
        name: payload.name,
        checkpoints,
    })
}
