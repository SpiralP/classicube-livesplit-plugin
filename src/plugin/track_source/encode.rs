#[cfg(test)]
mod tests;

use anyhow::{Result, bail, ensure};

use crate::plugin::splits::geometry::{
    CheckpointKind, Track, Trigger, aabb_to_min_size, validate_pause_resume_pairing,
};

/// Maximum per-line length, in codepoints. ClassiCube's
/// `INPUTWIDGET_LEN`/`STRING_SIZE` wrap point is 64; subtract 3 for the
/// default color prefix the server prepends on echo, leaving 61 cp for
/// our payload. Going over means `LineWrapper` re-splits the line and
/// inserts a `> &X` continuation marker, which we don't reassemble on
/// the receive side.
pub(crate) const MAX_LINE_CP: usize = 64 - 3;

/// Encode a `Track` into a series of `LS …` chat lines. The caller is
/// responsible for emitting them — one per `/mb sign` block, or chained
/// into a single block via the `mb` arm of the command module.
///
/// Layout:
///   line[0]    = `LS title <name>`
///   line[1..n] = per checkpoint, in order. Each checkpoint emits one
///                of four keyword lines:
///                  `LS cp <min> <size> [label]`      (Split, AABB)
///                  `LS map <name> [label]`           (Split, MapLoaded)
///                  `LS pause <min> <size> [label]`   (Pause, AABB)
///                  `LS unpause <min> <size> [label]` (Resume, AABB)
///                Start uses `LS cp` / `LS map` like a Split (position
///                implies kind); End is the last checkpoint and uses
///                whichever wire form (cp / map) its trigger demands.
///                Labels are inline when they fit, otherwise the kind
///                line is emitted bare and followed by
///                `LS label <text>`. Map names cannot contain a space
///                (the first space delimits name from label); the
///                encoder errors if the runtime `MapLoaded(name)`
///                contains one. Pause/Resume kinds are AABB-only; the
///                encoder rejects `Trigger::MapLoaded` paired with
///                them.
///   line[-1]   = `LS end` terminator. The receiver promotes the
///                last buffered checkpoint's kind from `Split` to
///                `End` on this line (Pause/Resume can't be the last
///                checkpoint; the encoder enforces this above).
pub fn encode_for_chat(track: &Track) -> Result<Vec<String>> {
    let n = track.checkpoints.len();
    ensure!(
        n >= 2,
        "track has {n} checkpoint(s); need at least 2 (Start + End)"
    );
    ensure!(!track.name.trim().is_empty(), "track name is empty");

    for (i, cp) in track.checkpoints.iter().enumerate() {
        let valid = if i == 0 {
            cp.kind == CheckpointKind::Start
        } else if i + 1 == n {
            cp.kind == CheckpointKind::End
        } else {
            matches!(
                cp.kind,
                CheckpointKind::Split | CheckpointKind::Pause | CheckpointKind::Resume
            )
        };
        if !valid {
            bail!(
                "checkpoint[{i}] kind is {:?}; expected Start at index 0, End at last index, and \
                 Split/Pause/Resume in between",
                cp.kind
            );
        }
        ensure!(
            !cp.label.trim().is_empty(),
            "checkpoint[{i}] label is empty (encoder requires non-empty labels)"
        );
        if matches!(cp.kind, CheckpointKind::Pause | CheckpointKind::Resume)
            && !matches!(cp.trigger, Trigger::Aabb(_))
        {
            bail!(
                "checkpoint[{i}] is {:?} kind but trigger is not AABB; Pause/Resume kinds are \
                 AABB-only",
                cp.kind
            );
        }
    }

    validate_pause_resume_pairing(track)?;

    let mut lines = Vec::with_capacity(2 + n);

    let title = format!("LS title {}", track.name);
    let title_cp = title.chars().count();
    ensure!(
        title_cp <= MAX_LINE_CP,
        "title line is {title_cp} cp; cap is {MAX_LINE_CP}"
    );
    lines.push(title);

    for (i, cp) in track.checkpoints.iter().enumerate() {
        let keyword = match cp.kind {
            CheckpointKind::Pause => "pause",
            CheckpointKind::Resume => "unpause",
            CheckpointKind::Start | CheckpointKind::Split | CheckpointKind::End => {
                match &cp.trigger {
                    Trigger::Aabb(_) => "cp",
                    Trigger::MapLoaded(_) => "map",
                }
            }
        };

        match &cp.trigger {
            Trigger::Aabb(aabb) => {
                let (min, size) = aabb_to_min_size(*aabb)?;
                let coords = format!(
                    "{},{},{} {},{},{}",
                    min[0], min[1], min[2], size[0], size[1], size[2]
                );

                let inline = format!("LS {keyword} {coords} {}", cp.label);
                if inline.chars().count() <= MAX_LINE_CP {
                    lines.push(inline);
                    continue;
                }

                let bare = format!("LS {keyword} {coords}");
                let bare_cp = bare.chars().count();
                ensure!(
                    bare_cp <= MAX_LINE_CP,
                    "checkpoint[{i}] `{keyword}` line without label is {bare_cp} cp; cap is \
                     {MAX_LINE_CP}"
                );
                lines.push(bare);

                let label_line = format!("LS label {}", cp.label);
                let label_cp = label_line.chars().count();
                ensure!(
                    label_cp <= MAX_LINE_CP,
                    "checkpoint[{i}] label too long: standalone `LS label` line is {label_cp} cp; \
                     cap is {MAX_LINE_CP}"
                );
                lines.push(label_line);
            }
            Trigger::MapLoaded(name) => {
                ensure!(!name.trim().is_empty(), "checkpoint[{i}] map name is empty");
                ensure!(
                    !name.contains(' '),
                    "checkpoint[{i}] map name `{name}` contains a space; map names on the wire \
                     cannot contain spaces (the space delimits name from label)"
                );

                let inline = format!("LS {keyword} {name} {}", cp.label);
                if inline.chars().count() <= MAX_LINE_CP {
                    lines.push(inline);
                    continue;
                }

                let bare = format!("LS {keyword} {name}");
                let bare_cp = bare.chars().count();
                ensure!(
                    bare_cp <= MAX_LINE_CP,
                    "checkpoint[{i}] `{keyword}` line without label is {bare_cp} cp; cap is \
                     {MAX_LINE_CP}"
                );
                lines.push(bare);

                let label_line = format!("LS label {}", cp.label);
                let label_cp = label_line.chars().count();
                ensure!(
                    label_cp <= MAX_LINE_CP,
                    "checkpoint[{i}] label too long: standalone `LS label` line is {label_cp} cp; \
                     cap is {MAX_LINE_CP}"
                );
                lines.push(label_line);
            }
        }
    }

    lines.push("LS end".to_string());

    for (i, line) in lines.iter().enumerate() {
        let cp_len = line.chars().count();
        debug_assert!(
            cp_len <= MAX_LINE_CP,
            "line[{i}] is {cp_len} cp, exceeds cap {MAX_LINE_CP}: {line}"
        );
    }

    Ok(lines)
}
