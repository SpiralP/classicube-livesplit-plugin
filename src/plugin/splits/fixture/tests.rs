use super::*;

#[test]
fn loadtest_has_expected_kind_sequence() {
    let t = loadtest();
    let kinds: Vec<_> = t.checkpoints.iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        vec![
            CheckpointKind::Start,
            CheckpointKind::Split,
            CheckpointKind::Split,
            CheckpointKind::Split,
            CheckpointKind::Split,
            CheckpointKind::Split,
            CheckpointKind::End,
        ]
    );
}

#[test]
fn loadtest_labels_are_populated() {
    let t = loadtest();
    for cp in &t.checkpoints {
        assert!(!cp.label.is_empty());
    }
}

#[test]
fn loadtest_has_map_loaded_trigger() {
    let t = loadtest();
    let map_triggers: Vec<&str> = t
        .checkpoints
        .iter()
        .filter_map(|c| match &c.trigger {
            Trigger::MapLoaded(name) => Some(name.as_str()),
            Trigger::Aabb(_) => None,
        })
        .collect();
    assert_eq!(map_triggers, vec!["mapname"]);
}

#[test]
fn loadtest_encodes_to_expected_wire_form() {
    use crate::plugin::track_source::encode::encode_for_chat;
    let lines = encode_for_chat(&loadtest()).unwrap();
    assert_eq!(
        lines,
        vec![
            "LS title Load Test",
            "LS cp 0,0,0 2,4,2 Start CheckPoint",
            "LS cp 10,0,0 2,4,2 Split A",
            "LS cp 20,0,0 2,4,2 Split B",
            "LS map mapname Map Name",
            "LS cp 0,0,0 2,4,2 Split C",
            "LS cp 10,0,0 2,4,2 Split D",
            "LS cp 20,0,0 2,4,2 Split E",
            "LS end",
        ]
    );
}

#[test]
fn save_loadtest_as_lss() {
    use std::{env, fs};

    use livesplit_core::{Run, Segment, run::saver::livesplit::save_run};

    let mut run = Run::new();
    run.set_game_name("ClassiCube");
    run.set_category_name("Load Test");

    // LiveSplit's segment list is everything after the implicit Start —
    // pressing Start is the timer-side action, not a named segment. So
    // the fixture's Start checkpoint doesn't get a Segment; the rest do.
    let segment_names = [
        "Split A", "Split B", "Map Name", "Split C", "Split D", "Split E",
    ];
    for name in segment_names {
        run.push_segment(Segment::new(name));
    }

    let mut buf = String::new();
    save_run(&run, &mut buf).unwrap();

    let path = env::temp_dir().join("loadtest.lss");
    fs::write(&path, &buf).unwrap();
    eprintln!("wrote {} bytes to {}", buf.len(), path.display());

    assert!(buf.starts_with(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
    assert!(buf.contains(r#"<Run version="1.8.0">"#));
    assert!(buf.contains("<GameName>ClassiCube</GameName>"));
    assert!(buf.contains("<CategoryName>Load Test</CategoryName>"));
    for name in segment_names {
        assert!(
            buf.contains(&format!("<Name>{name}</Name>")),
            "missing segment <Name>{name}</Name> in:\n{buf}"
        );
    }
}
