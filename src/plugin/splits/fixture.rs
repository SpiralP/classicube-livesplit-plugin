#[cfg(test)]
mod tests;

use classicube_sys::Vec3;

use crate::plugin::splits::geometry::{Aabb, Checkpoint, CheckpointKind, Track, Trigger};

/// A small fixed track used by the `/client LiveSplit loadtest` chat
/// subcommand for development. Eight checkpoints arranged as two AABB
/// rows along the +X axis with a `MapLoaded` Split in the middle and
/// a `MapLoaded` End on the far side. The encoded wire form exercises
/// both inline and `LS label` follow-up forms for both `cp` and `map`
/// triggers: `Split B` (AABB) and the middle map carry overflow-length
/// labels that force the follow-up fallback; the End map and every
/// other AABB fit inline.
///
/// The first three AABBs have no `MapLoaded` ahead of them, so their
/// scope falls back to `SplitsState.starting_map` (captured at load
/// time) — they fire only on whatever world the player loaded the
/// track on. After the `MapLoaded("spiralp+livesplit2")` Split, the
/// next three AABBs are scoped to that map by the running scope walk
/// in `step()`.
#[must_use]
pub fn loadtest() -> Track {
    Track {
        name: "Load Test".into(),
        checkpoints: vec![
            aabb_checkpoint(
                CheckpointKind::Start,
                (0.0, 0.0, 0.0),
                (2.0, 4.0, 2.0),
                "Start",
            ),
            aabb_checkpoint(
                CheckpointKind::Split,
                (10.0, 0.0, 0.0),
                (12.0, 4.0, 2.0),
                "1 Split A",
            ),
            aabb_checkpoint(
                CheckpointKind::Split,
                (20.0, 0.0, 0.0),
                (22.0, 4.0, 2.0),
                "1 Split B with a really long descriptive label",
            ),
            aabb_checkpoint(
                CheckpointKind::Split,
                (30.0, 0.0, 0.0),
                (32.0, 4.0, 2.0),
                "1 Split C",
            ),
            map_checkpoint(
                CheckpointKind::Split,
                "spiralp+livesplit2",
                "Map Name with a really really long descriptive label",
            ),
            aabb_checkpoint(
                CheckpointKind::Split,
                (0.0, 0.0, 0.0),
                (2.0, 4.0, 2.0),
                "2 Split A",
            ),
            aabb_checkpoint(
                CheckpointKind::Split,
                (10.0, 0.0, 0.0),
                (12.0, 4.0, 2.0),
                "2 Split B",
            ),
            aabb_checkpoint(
                CheckpointKind::Split,
                (20.0, 0.0, 0.0),
                (22.0, 4.0, 2.0),
                "2 Split C",
            ),
            aabb_checkpoint(
                CheckpointKind::Split,
                (30.0, 0.0, 0.0),
                (32.0, 4.0, 2.0),
                "2 Split D",
            ),
            map_checkpoint(CheckpointKind::Split, "novacity", "Nova City"),
            aabb_checkpoint(
                CheckpointKind::Split,
                (1905.0, 40.0, 844.0),
                (1906.0, 42.0, 845.0),
                "Nova City Target",
            ),
            map_checkpoint(CheckpointKind::End, "main6", "Main Map"),
        ],
    }
}

fn aabb_checkpoint(
    kind: CheckpointKind,
    min: (f32, f32, f32),
    max: (f32, f32, f32),
    label: &str,
) -> Checkpoint {
    Checkpoint {
        kind,
        trigger: Trigger::Aabb(Aabb {
            min: Vec3::new(min.0, min.1, min.2),
            max: Vec3::new(max.0, max.1, max.2),
        }),
        label: label.into(),
    }
}

fn map_checkpoint(kind: CheckpointKind, map_name: &str, label: &str) -> Checkpoint {
    Checkpoint {
        kind,
        trigger: Trigger::MapLoaded(map_name.into()),
        label: label.into(),
    }
}
