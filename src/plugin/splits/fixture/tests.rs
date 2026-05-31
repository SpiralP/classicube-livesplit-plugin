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
fn save_loadtest_as_lss() {
    use std::{env, fs};

    use livesplit_core::{Run, Segment, run::saver::livesplit::save_run};

    let mut run = Run::new();
    run.set_game_name("ClassiCube");
    run.set_category_name("loadtest");

    // LiveSplit's segment list is everything after the implicit Start —
    // pressing Start is the timer-side action, not a named segment. So
    // the fixture's Start checkpoint doesn't get a Segment; the rest do.
    let segment_names = ["split 1", "split 2", "end"];
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
    assert!(buf.contains("<CategoryName>loadtest</CategoryName>"));
    for name in segment_names {
        assert!(
            buf.contains(&format!("<Name>{name}</Name>")),
            "missing segment <Name>{name}</Name> in:\n{buf}"
        );
    }
}
