use std::sync::Mutex;

use classicube_sys::Vec3;

use super::*;
use crate::plugin::{
    splits::geometry::{Aabb, Checkpoint, CheckpointKind},
    track_source::encode::encode_for_chat,
};

// The decoder owns a thread-local state. Cargo nextest runs each
// test in its own process so isolation is free there, but `cargo
// test` shares threads — guard with a mutex and reset state at the
// top of each test.
static SERIALIZE: Mutex<()> = Mutex::new(());

fn reset_state() {
    STATE.with(|s| *s.borrow_mut() = State::Idle);
}

fn fresh() -> std::sync::MutexGuard<'static, ()> {
    let g = SERIALIZE.lock().unwrap_or_else(|p| p.into_inner());
    reset_state();
    g
}

fn aabb(min: (f32, f32, f32), max: (f32, f32, f32)) -> Aabb {
    Aabb {
        min: Vec3::new(min.0, min.1, min.2),
        max: Vec3::new(max.0, max.1, max.2),
    }
}

fn cp(kind: CheckpointKind, min: (f32, f32, f32), max: (f32, f32, f32), label: &str) -> Checkpoint {
    Checkpoint {
        kind,
        trigger: Trigger::Aabb(aabb(min, max)),
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

fn assert_buffered(o: FrameOutcome) {
    match o {
        FrameOutcome::Buffered => {}
        FrameOutcome::NotOurs => panic!("expected Buffered, got NotOurs"),
        FrameOutcome::ParseError(m) => panic!("expected Buffered, got ParseError({m})"),
        FrameOutcome::Loaded(_) => panic!("expected Buffered, got Loaded"),
    }
}

fn assert_not_ours(o: FrameOutcome) {
    match o {
        FrameOutcome::NotOurs => {}
        FrameOutcome::Buffered => panic!("expected NotOurs, got Buffered"),
        FrameOutcome::ParseError(m) => panic!("expected NotOurs, got ParseError({m})"),
        FrameOutcome::Loaded(_) => panic!("expected NotOurs, got Loaded"),
    }
}

fn assert_parse_error(o: FrameOutcome) -> String {
    match o {
        FrameOutcome::ParseError(m) => m,
        FrameOutcome::NotOurs => panic!("expected ParseError, got NotOurs"),
        FrameOutcome::Buffered => panic!("expected ParseError, got Buffered"),
        FrameOutcome::Loaded(_) => panic!("expected ParseError, got Loaded"),
    }
}

fn assert_loaded(o: FrameOutcome) -> Track {
    match o {
        FrameOutcome::Loaded(t) => t,
        FrameOutcome::NotOurs => panic!("expected Loaded, got NotOurs"),
        FrameOutcome::Buffered => panic!("expected Loaded, got Buffered"),
        FrameOutcome::ParseError(m) => panic!("expected Loaded, got ParseError({m})"),
    }
}

// ---- NotOurs ----

#[test]
fn empty_string_is_not_ours() {
    let _g = fresh();
    assert_not_ours(feed_chat_line(""));
}

#[test]
fn plain_text_is_not_ours() {
    let _g = fresh();
    assert_not_ours(feed_chat_line("hello world"));
}

#[test]
fn colored_server_chat_is_not_ours() {
    let _g = fresh();
    // `&e` stripped, then `hello` has no `LS ` prefix.
    assert_not_ours(feed_chat_line("&ehello"));
}

#[test]
fn ls_without_trailing_space_is_not_ours() {
    let _g = fresh();
    assert_not_ours(feed_chat_line("LSfoo"));
}

#[test]
fn lone_ampersand_is_not_ours() {
    let _g = fresh();
    // `&` with no following char: nothing to strip, no `LS ` prefix.
    assert_not_ours(feed_chat_line("&"));
    // `&L` strips as a color code (custom CPE codes can be any ASCII
    // alphanumeric); remaining `S title foo` lacks `LS `.
    assert_not_ours(feed_chat_line("&LS title foo"));
}

#[test]
fn ampersand_then_non_code_is_not_ours() {
    let _g = fresh();
    // `&-` is not a valid color code (non-alphanumeric second char);
    // strip leaves the text intact, no `LS ` prefix.
    assert_not_ours(feed_chat_line("&-LS title foo"));
}

#[test]
fn colored_title_is_accepted() {
    let _g = fresh();
    // The observed wire form on ClassiCube official servers: a
    // single `&7` (or whatever the server's default color) preceding
    // the frame. MCGalaxy collapses runs of codes to one before
    // broadcast, so we only strip one.
    assert_buffered(feed_chat_line("&7LS title loadtest"));
}

#[test]
fn colored_round_trip_matches_uncolored() {
    let _g = fresh();
    // Full round-trip with a `&7` prefix on every emitted line — the
    // exact pattern seen on real servers when /msgme echoes each
    // chained line.
    let track = Track {
        name: "rt".into(),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "s"),
            cp(CheckpointKind::End, (10.0, 0.0, 0.0), (2.0, 4.0, 2.0), "e"),
        ],
    };
    let lines = encode_for_chat(&track).expect("encode");
    let last = lines.len() - 1;
    for line in &lines[..last] {
        assert_buffered(feed_chat_line(&format!("&7{line}")));
    }
    let loaded = assert_loaded(feed_chat_line(&format!("&7{}", lines[last])));
    assert_eq!(loaded.name, track.name);
    assert_eq!(loaded.checkpoints.len(), track.checkpoints.len());
}

// ---- ParseError ----

#[test]
fn unknown_keyword_errors() {
    let _g = fresh();
    let m = assert_parse_error(feed_chat_line("LS frobnicate stuff"));
    assert!(m.contains("frobnicate"), "{m}");
}

#[test]
fn cp_before_title_errors() {
    let _g = fresh();
    let m = assert_parse_error(feed_chat_line("LS cp 0,0,0 1,1,1 label"));
    assert!(m.contains("no `LS title`"), "{m}");
}

#[test]
fn end_before_title_errors() {
    let _g = fresh();
    let m = assert_parse_error(feed_chat_line("LS end"));
    assert!(m.contains("no `LS title`"), "{m}");
}

#[test]
fn cp_in_need_label_errors() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    assert_buffered(feed_chat_line("LS cp 0,0,0 1,1,1")); // no inline label
    let m = assert_parse_error(feed_chat_line("LS cp 10,0,0 1,1,1 mid"));
    assert!(m.contains("not yet labeled"), "{m}");
}

#[test]
fn end_in_need_label_errors() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    assert_buffered(feed_chat_line("LS cp 0,0,0 1,1,1"));
    let m = assert_parse_error(feed_chat_line("LS end"));
    assert!(m.contains("not yet labeled"), "{m}");
}

#[test]
fn label_in_idle_errors() {
    let _g = fresh();
    let m = assert_parse_error(feed_chat_line("LS label some text"));
    assert!(m.contains("no `LS title`"), "{m}");
}

#[test]
fn label_in_need_first_errors() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    let m = assert_parse_error(feed_chat_line("LS label premature"));
    assert!(m.contains("no checkpoint to label"), "{m}");
}

#[test]
fn label_after_inline_label_errors() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    assert_buffered(feed_chat_line("LS cp 0,0,0 1,1,1 inline"));
    let m = assert_parse_error(feed_chat_line("LS label extra"));
    assert!(m.contains("already has a label"), "{m}");
}

#[test]
fn empty_title_name_errors() {
    let _g = fresh();
    let m = assert_parse_error(feed_chat_line("LS title "));
    assert!(m.contains("title name is empty"), "{m}");
}

#[test]
fn empty_title_name_no_space_errors() {
    let _g = fresh();
    let m = assert_parse_error(feed_chat_line("LS title"));
    assert!(m.contains("title name is empty"), "{m}");
}

#[test]
fn whitespace_inline_label_errors() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    // Trailing space after size triple, then empty (all-whitespace) label.
    let m = assert_parse_error(feed_chat_line("LS cp 100,64,200 1,1,1 "));
    assert!(m.contains("inline label is empty"), "{m}");
}

#[test]
fn whitespace_only_label_text_errors() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    assert_buffered(feed_chat_line("LS cp 0,0,0 1,1,1"));
    let m = assert_parse_error(feed_chat_line("LS label    "));
    assert!(m.contains("label text is empty"), "{m}");
}

#[test]
fn non_numeric_coord_errors() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    let m = assert_parse_error(feed_chat_line("LS cp abc,def,ghi 1,1,1"));
    assert!(m.contains("min"), "{m}");
}

#[test]
fn wrong_size_arity_errors() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    let m = assert_parse_error(feed_chat_line("LS cp 100,64,200 4,4"));
    assert!(m.contains("3 comma-separated size"), "{m}");
}

#[test]
fn wrong_min_arity_errors() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    let m = assert_parse_error(feed_chat_line("LS cp 100,64,200,extra 4,4,4"));
    assert!(m.contains("3 comma-separated min"), "{m}");
}

#[test]
fn missing_comma_wrong_shape_errors() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    // splitn(3, ' ') sees ["100", "64", "200 4 4 4"] → min parse fails arity.
    let m = assert_parse_error(feed_chat_line("LS cp 100 64 200 4 4 4"));
    assert!(m.contains("min") || m.contains("comma"), "{m}");
}

#[test]
fn size_exceeds_u8_errors() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    let m = assert_parse_error(feed_chat_line("LS cp 100,64,200 4,4,300"));
    assert!(m.contains("size"), "{m}");
}

#[test]
fn end_in_need_first_errors() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    let m = assert_parse_error(feed_chat_line("LS end"));
    assert!(m.contains("no checkpoints before `LS end`"), "{m}");
}

#[test]
fn end_with_one_checkpoint_errors() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    assert_buffered(feed_chat_line("LS cp 0,0,0 1,1,1 only"));
    let m = assert_parse_error(feed_chat_line("LS end"));
    assert!(m.contains("at least 2 checkpoints"), "{m}");
}

#[test]
fn end_with_trailing_args_errors() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    let m = assert_parse_error(feed_chat_line("LS end stuff"));
    assert!(m.contains("takes no arguments"), "{m}");
}

// ---- Buffered ----

#[test]
fn title_alone_buffers() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title my track"));
}

#[test]
fn title_then_cp_inline_buffers() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    assert_buffered(feed_chat_line("LS cp 0,0,0 2,4,2 start"));
}

#[test]
fn title_then_cp_no_label_buffers() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    assert_buffered(feed_chat_line("LS cp 0,0,0 2,4,2"));
}

#[test]
fn title_then_cp_then_label_buffers() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    assert_buffered(feed_chat_line("LS cp 0,0,0 2,4,2"));
    assert_buffered(feed_chat_line("LS label start"));
}

#[test]
fn title_then_cp_chain_mixed_label_forms_buffer() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    assert_buffered(feed_chat_line("LS cp 0,0,0 2,4,2 start"));
    assert_buffered(feed_chat_line("LS cp 10,0,0 2,4,2"));
    assert_buffered(feed_chat_line("LS label split 1"));
    assert_buffered(feed_chat_line("LS cp 20,0,0 2,4,2 split 2"));
}

// ---- Loaded ----

fn linear_track() -> Track {
    Track {
        name: "linear".into(),
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

fn feed_all_but_last(lines: &[String]) {
    for line in &lines[..lines.len() - 1] {
        assert_buffered(feed_chat_line(line));
    }
}

#[test]
fn round_trip_all_inline() {
    let _g = fresh();
    let original = linear_track();
    let lines = encode_for_chat(&original).unwrap();
    feed_all_but_last(&lines);
    let decoded = assert_loaded(feed_chat_line(lines.last().unwrap()));
    assert_eq!(decoded, original);
}

#[test]
fn round_trip_with_overflow_label_on_end() {
    let _g = fresh();
    // Force the End checkpoint's label past the inline cap so the
    // payload line is `LS cp <coords>` followed by `LS label <text>`,
    // then `LS end`. Inline cp line: "LS cp 30,0,0 2,4,2 <label>" =
    // 20 + label cp. Cap is 61, so label needs > 41 cp to overflow
    // inline; 45 cp lands above that and still fits standalone.
    let long_label = "x".repeat(45);
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(
                CheckpointKind::Start,
                (0.0, 0.0, 0.0),
                (2.0, 4.0, 2.0),
                "start",
            ),
            cp(
                CheckpointKind::End,
                (30.0, 0.0, 0.0),
                (32.0, 4.0, 2.0),
                &long_label,
            ),
        ],
    };
    let lines = encode_for_chat(&track).unwrap();
    // title + cp(start inline) + cp(end bare) + label + end
    assert_eq!(lines.len(), 1 + 1 + 1 + 1 + 1);
    assert_eq!(lines.last().unwrap(), "LS end");
    assert!(lines[lines.len() - 2].starts_with("LS label "));
    feed_all_but_last(&lines);
    let decoded = assert_loaded(feed_chat_line(lines.last().unwrap()));
    assert_eq!(decoded, track);
}

#[test]
fn refeed_title_resets_buffer() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T1"));
    assert_buffered(feed_chat_line("LS cp 0,0,0 2,4,2 start"));
    assert_buffered(feed_chat_line("LS cp 10,0,0 2,4,2 mid"));
    // Re-fed title resets to NeedFirst.
    assert_buffered(feed_chat_line("LS title T2"));
    // No checkpoints buffered yet, so `LS end` must error.
    let m = assert_parse_error(feed_chat_line("LS end"));
    assert!(m.contains("no checkpoints before `LS end`"), "{m}");
}

#[test]
fn multi_space_label_survives_round_trip() {
    let _g = fresh();
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
    feed_all_but_last(&lines);
    let decoded = assert_loaded(feed_chat_line(lines.last().unwrap()));
    assert_eq!(decoded, track);
}

#[test]
fn mixed_inline_and_followup_labels_round_trip() {
    let _g = fresh();
    // First cp inline label, second cp's label via follow-up, third
    // (becomes End) inline label. Round-trips to runtime kinds Start,
    // Split, End.
    let lines = [
        "LS title mixed",
        "LS cp 0,0,0 2,4,2 start",
        "LS cp 10,0,0 2,4,2",
        "LS label split 1",
        "LS cp 30,0,0 2,4,2 end",
        "LS end",
    ];
    for line in &lines[..lines.len() - 1] {
        assert_buffered(feed_chat_line(line));
    }
    let decoded = assert_loaded(feed_chat_line(lines.last().unwrap()));
    let expected = Track {
        name: "mixed".into(),
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
                CheckpointKind::End,
                (30.0, 0.0, 0.0),
                (32.0, 4.0, 2.0),
                "end",
            ),
        ],
    };
    assert_eq!(decoded, expected);
}

#[test]
fn map_only_round_trip_via_encoder() {
    let _g = fresh();
    let track = Track {
        name: "M".into(),
        checkpoints: vec![
            cp_map(CheckpointKind::Start, "spawn", "start"),
            cp_map(CheckpointKind::End, "goal", "end"),
        ],
    };
    let lines = encode_for_chat(&track).unwrap();
    feed_all_but_last(&lines);
    let decoded = assert_loaded(feed_chat_line(lines.last().unwrap()));
    assert_eq!(decoded, track);
}

#[test]
fn mixed_aabb_and_map_round_trip_via_encoder() {
    let _g = fresh();
    let track = Track {
        name: "mix".into(),
        checkpoints: vec![
            cp(
                CheckpointKind::Start,
                (0.0, 0.0, 0.0),
                (2.0, 4.0, 2.0),
                "begin",
            ),
            cp_map(CheckpointKind::Split, "level2", "to level 2"),
            cp(
                CheckpointKind::Split,
                (20.0, 0.0, 0.0),
                (22.0, 4.0, 2.0),
                "post-load",
            ),
            cp_map(CheckpointKind::End, "goal", "fin"),
        ],
    };
    let lines = encode_for_chat(&track).unwrap();
    feed_all_but_last(&lines);
    let decoded = assert_loaded(feed_chat_line(lines.last().unwrap()));
    assert_eq!(decoded, track);
}

#[test]
fn map_in_idle_errors() {
    let _g = fresh();
    let m = assert_parse_error(feed_chat_line("LS map spawn"));
    assert!(m.contains("no `LS title`"), "{m}");
}

#[test]
fn map_in_need_label_errors() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    assert_buffered(feed_chat_line("LS map spawn"));
    let m = assert_parse_error(feed_chat_line("LS map other"));
    assert!(m.contains("not yet labeled"), "{m}");
}

#[test]
fn empty_map_name_errors() {
    let _g = fresh();
    assert_buffered(feed_chat_line("LS title T"));
    let m = assert_parse_error(feed_chat_line("LS map "));
    assert!(m.contains("map name is empty"), "{m}");
}

#[test]
fn map_name_preserves_spaces() {
    let _g = fresh();
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp_map(CheckpointKind::Start, "Castle Lobby", "spawn"),
            cp_map(CheckpointKind::End, "Final Room", "done"),
        ],
    };
    let lines = encode_for_chat(&track).unwrap();
    feed_all_but_last(&lines);
    let decoded = assert_loaded(feed_chat_line(lines.last().unwrap()));
    assert_eq!(decoded, track);
}
