#[cfg(test)]
mod tests;

use classicube_sys::Vec3;

use crate::plugin::splits::geometry::{Aabb, Checkpoint, CheckpointKind, Track, Trigger};

/// A small fixed track used by the `/client LiveSplit loadtest` chat
/// subcommand for development. AABB rows along the +X axis with a
/// `MapLoaded` Split in the middle and a `MapLoaded` End on the far
/// side. The encoded wire form exercises both inline and `LS label`
/// follow-up forms for both `cp` and `map` triggers: `Split B` (AABB)
/// and the middle map carry overflow-length labels that force the
/// follow-up fallback; the End map and every other AABB fit inline.
///
/// The first three AABBs have no `MapLoaded` ahead of them, so their
/// scope falls back to `SplitsState.starting_map` (captured at load
/// time) — they fire only on whatever world the player loaded the
/// track on. After the `MapLoaded("spiralp+livesplit2")` Split, the
/// next three AABBs are scoped to that map by the running scope walk
/// in `step()`.
///
/// Pause/Resume checkpoints sit on either side of the
/// `spiralp+livesplit2` MapLoaded so the test track demonstrates
/// cross-map pause: the Pause AABB is the last checkpoint on the
/// starting map (right before the transit) and the Resume AABB is the
/// first checkpoint on `spiralp+livesplit2` (right after).
#[must_use]
pub fn loadtest() -> Track {
    Track {
        name: "Load Test".into(),
        checkpoints: vec![
            aabb_checkpoint(CheckpointKind::Start, (0, 0, 0), (2, 4, 2), "Start"),
            aabb_checkpoint(CheckpointKind::Split, (10, 0, 0), (2, 4, 2), "1 Split A"),
            aabb_checkpoint(
                CheckpointKind::Split,
                (20, 0, 0),
                (2, 4, 2),
                "1 Split B with a really long descriptive label",
            ),
            aabb_checkpoint(CheckpointKind::Split, (30, 0, 0), (2, 4, 2), "1 Split C"),
            aabb_checkpoint(
                CheckpointKind::Pause,
                (34, 0, 5),
                (1, 2, 1),
                "Pause before transit",
            ),
            map_checkpoint(
                CheckpointKind::Split,
                "spiralp+livesplit2",
                "Map Name with a really really long descriptive label",
            ),
            aabb_checkpoint(
                CheckpointKind::Resume,
                (9, 0, 18),
                (1, 2, 1),
                "Resume after transit",
            ),
            aabb_checkpoint(CheckpointKind::Split, (0, 0, 0), (2, 4, 2), "2 Split A"),
            aabb_checkpoint(CheckpointKind::Split, (10, 0, 0), (2, 4, 2), "2 Split B"),
            aabb_checkpoint(CheckpointKind::Split, (20, 0, 0), (2, 4, 2), "2 Split C"),
            aabb_checkpoint(CheckpointKind::Split, (30, 0, 0), (2, 4, 2), "2 Split D"),
            aabb_checkpoint(
                CheckpointKind::Pause,
                (34, 0, 5),
                (1, 2, 1),
                "Pause before transit",
            ),
            map_checkpoint(CheckpointKind::Split, "novacity", "Nova City"),
            aabb_checkpoint(
                CheckpointKind::Resume,
                (1915, 45, 844),
                (1, 2, 1),
                "Resume after transit",
            ),
            aabb_checkpoint(
                CheckpointKind::Split,
                (1906, 40, 843),
                (1, 2, 1),
                "Nova City Split A",
            ),
            map_checkpoint(CheckpointKind::End, "main6", "Main Map"),
        ],
    }
}

fn aabb_checkpoint(
    kind: CheckpointKind,
    min: (u16, u16, u16),
    size: (u8, u8, u8),
    label: &str,
) -> Checkpoint {
    let min = Vec3::new(f32::from(min.0), f32::from(min.1), f32::from(min.2));
    let max = Vec3::new(
        min.x + f32::from(size.0),
        min.y + f32::from(size.1),
        min.z + f32::from(size.2),
    );
    Checkpoint {
        kind,
        trigger: Trigger::Aabb(Aabb { min, max }),
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
