use classicube_sys::Vec3;

use super::*;
use crate::plugin::splits::geometry::{Aabb, Checkpoint, CheckpointKind};

fn cp(kind: CheckpointKind, min: (f32, f32, f32), max: (f32, f32, f32), label: &str) -> Checkpoint {
    Checkpoint {
        kind,
        trigger: Trigger::Aabb(Aabb {
            min: Vec3::new(min.0, min.1, min.2),
            max: Vec3::new(max.0, max.1, max.2),
        }),
        label: label.into(),
    }
}

fn cp_map(kind: CheckpointKind, name: &str, label: &str) -> Checkpoint {
    Checkpoint {
        kind,
        trigger: Trigger::MapLoaded(name.into()),
        label: label.into(),
    }
}

fn loadtest_track() -> Track {
    Track {
        name: "loadtest".into(),
        checkpoints: vec![
            cp(
                CheckpointKind::Start,
                (0.0, 0.0, 0.0),
                (2.0, 4.0, 2.0),
                "start",
            ),
            cp(
                CheckpointKind::Split,
                (10.0, 0.0, 0.0),
                (12.0, 4.0, 2.0),
                "split 1",
            ),
            cp(
                CheckpointKind::Split,
                (20.0, 0.0, 0.0),
                (22.0, 4.0, 2.0),
                "split 2",
            ),
            cp(
                CheckpointKind::End,
                (30.0, 0.0, 0.0),
                (32.0, 4.0, 2.0),
                "end",
            ),
        ],
    }
}

fn assert_lines_within_cap(lines: &[String]) {
    for line in lines {
        let cp_len = line.chars().count();
        assert!(
            cp_len <= MAX_LINE_CP,
            "line `{line}` is {cp_len} cp (cap {MAX_LINE_CP})"
        );
    }
}

#[test]
fn loadtest_round_trip_all_inline() {
    let track = loadtest_track();
    let lines = encode_for_chat(&track).unwrap();
    // title + 4 cps (each inline) + end
    assert_eq!(lines.len(), 1 + 4 + 1);
    assert_lines_within_cap(&lines);
    assert!(lines[0].starts_with("LS title "));
    assert!(lines[1].starts_with("LS cp "));
    assert!(lines[2].starts_with("LS cp "));
    assert!(lines[3].starts_with("LS cp "));
    assert!(lines[4].starts_with("LS cp "));
    assert_eq!(lines[5], "LS end");
}

#[test]
fn two_checkpoint_round_trip_has_four_lines() {
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "s"),
            cp(CheckpointKind::End, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0), "e"),
        ],
    };
    let lines = encode_for_chat(&track).unwrap();
    // title + 2 cps (inline) + end
    assert_eq!(lines.len(), 4);
    assert_lines_within_cap(&lines);
    assert_eq!(lines.last().unwrap(), "LS end");
}

#[test]
fn rejects_single_checkpoint_track() {
    let track = Track {
        name: "T".into(),
        checkpoints: vec![cp(
            CheckpointKind::Start,
            (0.0, 0.0, 0.0),
            (2.0, 4.0, 2.0),
            "only",
        )],
    };
    assert!(encode_for_chat(&track).is_err());
}

#[test]
fn rejects_empty_track_name() {
    let track = Track {
        name: "   ".into(),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "s"),
            cp(CheckpointKind::End, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0), "e"),
        ],
    };
    assert!(encode_for_chat(&track).is_err());
}

#[test]
fn rejects_empty_checkpoint_label() {
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), ""),
            cp(CheckpointKind::End, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0), "e"),
        ],
    };
    assert!(encode_for_chat(&track).is_err());
}

#[test]
fn rejects_overlong_track_name() {
    // "LS title " is 9 cp; pad to push past 61 cp.
    let track = Track {
        name: "x".repeat(60),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "s"),
            cp(CheckpointKind::End, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0), "e"),
        ],
    };
    assert!(encode_for_chat(&track).is_err());
}

#[test]
fn rejects_label_too_long_even_standalone() {
    // "LS label " is 9 cp; >52 cp label overflows the standalone line.
    let label = "x".repeat(60);
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "s"),
            cp(
                CheckpointKind::End,
                (10.0, 0.0, 0.0),
                (12.0, 4.0, 2.0),
                &label,
            ),
        ],
    };
    assert!(encode_for_chat(&track).is_err());
}

#[test]
fn falls_back_to_separate_label_line_when_inline_overflows() {
    // Inline `LS cp 0,0,0 2,4,2 <label>` = 18 + label cp; cap 61
    // → label needs > 43 cp to overflow inline but ≤ 52 cp to fit
    // standalone. 45 cp lands in that range.
    let label = "x".repeat(45);
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(
                CheckpointKind::Start,
                (0.0, 0.0, 0.0),
                (2.0, 4.0, 2.0),
                &label,
            ),
            cp(CheckpointKind::End, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0), "e"),
        ],
    };
    let lines = encode_for_chat(&track).unwrap();
    // title + (cp + label) + cp + end
    assert_eq!(lines.len(), 1 + 2 + 1 + 1);
    assert!(lines[1].starts_with("LS cp ") && !lines[1].ends_with(&label));
    assert_eq!(lines[2], format!("LS label {label}"));
    assert_eq!(lines.last().unwrap(), "LS end");
    assert_lines_within_cap(&lines);
}

#[test]
fn preserves_multi_space_label_inline() {
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(
                CheckpointKind::Start,
                (0.0, 0.0, 0.0),
                (2.0, 4.0, 2.0),
                "my  multi  word  label",
            ),
            cp(
                CheckpointKind::End,
                (10.0, 0.0, 0.0),
                (12.0, 4.0, 2.0),
                "end",
            ),
        ],
    };
    let lines = encode_for_chat(&track).unwrap();
    assert!(
        lines[1].ends_with(" my  multi  word  label"),
        "got: {}",
        lines[1]
    );
}

#[test]
fn rejects_aabb_extent_over_255() {
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(
                CheckpointKind::Start,
                (0.0, 0.0, 0.0),
                (300.0, 4.0, 2.0),
                "s",
            ),
            cp(CheckpointKind::End, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0), "e"),
        ],
    };
    assert!(encode_for_chat(&track).is_err());
}

#[test]
fn rejects_split_at_index_zero() {
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(CheckpointKind::Split, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "a"),
            cp(CheckpointKind::End, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0), "e"),
        ],
    };
    assert!(encode_for_chat(&track).is_err());
}

#[test]
fn rejects_start_at_middle_index() {
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "s"),
            cp(
                CheckpointKind::Start,
                (10.0, 0.0, 0.0),
                (12.0, 4.0, 2.0),
                "mid",
            ),
            cp(CheckpointKind::End, (20.0, 0.0, 0.0), (22.0, 4.0, 2.0), "e"),
        ],
    };
    assert!(encode_for_chat(&track).is_err());
}

#[test]
fn rejects_end_at_non_last_index() {
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "s"),
            cp(
                CheckpointKind::End,
                (10.0, 0.0, 0.0),
                (12.0, 4.0, 2.0),
                "mid",
            ),
            cp(CheckpointKind::End, (20.0, 0.0, 0.0), (22.0, 4.0, 2.0), "e"),
        ],
    };
    assert!(encode_for_chat(&track).is_err());
}

#[test]
fn map_only_two_checkpoint_emits_map_lines_and_end() {
    let track = Track {
        name: "M".into(),
        checkpoints: vec![
            cp_map(CheckpointKind::Start, "spawn", "start"),
            cp_map(CheckpointKind::End, "goal", "end"),
        ],
    };
    let lines = encode_for_chat(&track).unwrap();
    // title + 2 cps × (map + label) + end
    assert_eq!(lines.len(), 1 + 4 + 1);
    assert_lines_within_cap(&lines);
    assert_eq!(lines[0], "LS title M");
    assert_eq!(lines[1], "LS map spawn");
    assert_eq!(lines[2], "LS label start");
    assert_eq!(lines[3], "LS map goal");
    assert_eq!(lines[4], "LS label end");
    assert_eq!(lines[5], "LS end");
}

#[test]
fn mixed_aabb_and_map_interleave() {
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "s"),
            cp_map(CheckpointKind::Split, "mid_map", "midmap"),
            cp(
                CheckpointKind::Split,
                (20.0, 0.0, 0.0),
                (22.0, 4.0, 2.0),
                "s2",
            ),
            cp_map(CheckpointKind::End, "goal", "fin"),
        ],
    };
    let lines = encode_for_chat(&track).unwrap();
    assert_lines_within_cap(&lines);
    assert!(lines[0].starts_with("LS title "));
    assert!(lines[1].starts_with("LS cp "));
    assert_eq!(lines[2], "LS map mid_map");
    assert_eq!(lines[3], "LS label midmap");
    assert!(lines[4].starts_with("LS cp "));
    assert_eq!(lines[5], "LS map goal");
    assert_eq!(lines[6], "LS label fin");
    assert_eq!(lines[7], "LS end");
}

#[test]
fn rejects_empty_map_name() {
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp_map(CheckpointKind::Start, "  ", "start"),
            cp_map(CheckpointKind::End, "goal", "end"),
        ],
    };
    assert!(encode_for_chat(&track).is_err());
}

#[test]
fn rejects_overlong_map_name() {
    // "LS map " is 7 cp; cap 61 → name > 54 cp overflows.
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp_map(CheckpointKind::Start, &"x".repeat(55), "s"),
            cp_map(CheckpointKind::End, "goal", "e"),
        ],
    };
    assert!(encode_for_chat(&track).is_err());
}
