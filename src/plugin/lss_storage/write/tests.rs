use classicube_sys::Vec3;
use livesplit_core::run::parser::livesplit::parse;

use super::*;
use crate::plugin::splits::geometry::{Aabb, Checkpoint, CheckpointKind, Trigger};

fn aabb(min: (u16, u16, u16), max: (u16, u16, u16)) -> Aabb {
    Aabb {
        min: Vec3::new(f32::from(min.0), f32::from(min.1), f32::from(min.2)),
        max: Vec3::new(f32::from(max.0), f32::from(max.1), f32::from(max.2)),
    }
}

fn sample_track() -> Track {
    Track {
        name: "any%".to_owned(),
        checkpoints: vec![
            Checkpoint {
                kind: CheckpointKind::Start,
                trigger: Trigger::Aabb(aabb((64, 40, 128), (66, 43, 130))),
                label: "start".to_owned(),
            },
            Checkpoint {
                kind: CheckpointKind::End,
                trigger: Trigger::Aabb(aabb((200, 50, 400), (204, 52, 404))),
                label: "end".to_owned(),
            },
        ],
    }
}

fn unique_tmp_dir(prefix: &str) -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("lss-{prefix}-{}-{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn build_lss_xml_stores_canonical_text() {
    let t = sample_track();
    let canonical = payload::serialize_canonical(&t).unwrap();

    let xml = build_lss_xml(&t, "Srv", "Map", &canonical).unwrap();
    let run = parse(&xml).expect("re-parse");

    let stored = run
        .metadata()
        .custom_variable_value(CUSTOM_VAR_NAME)
        .expect("ClassiCubeTrack present");
    // The stored value is the readable `LS …` geometry text verbatim.
    assert_eq!(stored, canonical);

    // The Start checkpoint is the timer-side run-start action, not a
    // named split, so it's omitted from the segment list -- only "end"
    // remains.
    let names: Vec<&str> = run.segments().iter().map(|s| s.name()).collect();
    assert_eq!(names, vec!["end"]);

    assert_eq!(run.game_name(), "ClassiCube");
    // The category name is the bare track name -- no server prefix.
    assert_eq!(run.category_name(), "any%");

    // Server + starting map are recorded as informational custom vars
    // (write-only; the plugin never reads them back).
    assert_eq!(
        run.metadata().custom_variable_value("ClassiCubeServer"),
        Some("Srv")
    );
    assert_eq!(
        run.metadata().custom_variable_value("ClassiCubeMap"),
        Some("Map")
    );
}

#[test]
fn build_lss_xml_round_trips_through_parse() {
    let t = sample_track(); // [Start "start", End "end"]
    let canonical = payload::serialize_canonical(&t).unwrap();

    let xml = build_lss_xml(&t, "Srv", "Map", &canonical).unwrap();
    let run = parse(&xml).expect("re-parse");

    let stored = run
        .metadata()
        .custom_variable_value(CUSTOM_VAR_NAME)
        .expect("ClassiCubeTrack present");
    let title = run.category_name().to_owned();
    let labels: Vec<String> = run.segments().iter().map(|s| s.name().to_owned()).collect();
    let back = payload::parse(stored, title, labels).unwrap();

    assert_eq!(back.checkpoints.len(), 2);
    // Start: kind + geometry survive; label defaults (no segment).
    assert_eq!(back.checkpoints[0].kind, CheckpointKind::Start);
    assert_eq!(back.checkpoints[0].trigger, t.checkpoints[0].trigger);
    assert_eq!(back.checkpoints[0].label, "Start");
    // End: its segment carries the label through.
    assert_eq!(back.checkpoints[1], t.checkpoints[1]);
}

#[test]
fn build_lss_xml_strips_color_codes_from_display() {
    let mut t = sample_track();
    t.name = "&aany%".to_owned();
    let canonical = payload::serialize_canonical(&t).unwrap();

    let xml = build_lss_xml(&t, "Srv", "Map", &canonical).unwrap();
    let run = parse(&xml).unwrap();
    let cat = run.category_name();
    assert!(
        !cat.contains('&'),
        "category name still has color code: {cat}"
    );
    // The category name is the bare, color-stripped track name.
    assert_eq!(cat, "any%");
}

#[test]
fn build_lss_xml_strips_color_codes_from_server_and_map() {
    let t = sample_track();
    let canonical = payload::serialize_canonical(&t).unwrap();

    // Server + map carry color codes on the wire; they're stripped for a
    // clean UI value, the same treatment `<CategoryName>` gets.
    let xml = build_lss_xml(&t, "&cMy&eServer", "&amymap", &canonical).unwrap();
    let run = parse(&xml).unwrap();

    let server = run
        .metadata()
        .custom_variable_value("ClassiCubeServer")
        .expect("ClassiCubeServer present");
    let map = run
        .metadata()
        .custom_variable_value("ClassiCubeMap")
        .expect("ClassiCubeMap present");
    assert!(
        !server.contains('&'),
        "server still has color code: {server}"
    );
    assert!(!map.contains('&'), "map still has color code: {map}");
    assert_eq!(server, "MyServer");
    assert_eq!(map, "mymap");
}

#[test]
fn track_name_color_code_stripped_through_category_name() {
    // The track name is no longer in the geometry payload (no `LS title`
    // line) -- it's carried by the `.lss` `<CategoryName>`, which is
    // color-stripped on write. So a `&`-prefixed name comes back
    // color-stripped after a round-trip; the color code is intentionally
    // lost (the accepted cosmetic loss of sourcing the title from the
    // stripped CategoryName).
    let mut t = sample_track();
    t.name = "&aany%".to_owned();
    let canonical = payload::serialize_canonical(&t).unwrap();
    assert!(canonical.starts_with("LS v1\n"));
    assert!(!canonical.contains("LS title"));

    let xml = build_lss_xml(&t, "Srv", "Map", &canonical).unwrap();
    let run = parse(&xml).expect("re-parse");
    let stored = run
        .metadata()
        .custom_variable_value(CUSTOM_VAR_NAME)
        .expect("ClassiCubeTrack present");
    assert_eq!(stored, canonical);

    let title = run.category_name().to_owned();
    let labels: Vec<String> = run.segments().iter().map(|s| s.name().to_owned()).collect();
    let back = payload::parse(stored, title, labels).unwrap();
    assert_eq!(back.name, "any%");
}

#[test]
fn save_track_to_writes_then_dedups() {
    let dir = unique_tmp_dir("write-dedup");
    let category = "anypct";

    let t = sample_track();
    match save_track_to(&t, "Srv", "Map", &dir, category).unwrap() {
        SaveOutcome::Wrote(p) => {
            assert_eq!(
                p.file_name().unwrap(),
                format!("{category}-v1.lss").as_str()
            );
        }
        SaveOutcome::AlreadyLatest => panic!("first call should have written"),
    }

    assert!(matches!(
        save_track_to(&t, "Srv", "Map", &dir, category).unwrap(),
        SaveOutcome::AlreadyLatest
    ));

    let mut t2 = sample_track();
    if let Trigger::Aabb(ref mut bb) = t2.checkpoints[0].trigger {
        bb.min.x += 1.0;
        bb.max.x += 1.0;
    }
    match save_track_to(&t2, "Srv", "Map", &dir, category).unwrap() {
        SaveOutcome::Wrote(p) => {
            assert_eq!(
                p.file_name().unwrap(),
                format!("{category}-v2.lss").as_str()
            );
        }
        SaveOutcome::AlreadyLatest => panic!("modified track should have written"),
    }

    // Label-only change: the geometry text is identical (labels live in
    // the `<Segment>` elements, not the payload), so the dedup gate still
    // reports no change -- the no-bump-on-relabel guarantee.
    let mut t3 = t2.clone();
    t3.checkpoints[0].label = "renamed".to_owned();
    assert!(matches!(
        save_track_to(&t3, "Srv", "Map", &dir, category).unwrap(),
        SaveOutcome::AlreadyLatest
    ));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn build_lss_xml_preserves_multiline_payload_through_real_save_load() {
    // The geometry payload is newline-joined `LS …` records stored in the
    // `ClassiCubeTrack` custom variable. livesplit-core writes literal
    // newlines (its XML writer escapes only `< > & ' "`) and reads internal
    // newlines back intact (the reader only trims the text node's
    // leading/trailing whitespace), so a multi-line payload -- including a
    // long single line -- round-trips byte-for-byte through the real
    // writer+reader. This pins that guarantee end-to-end, so no
    // wrap/continuation-join handling is needed on the decode side.
    //
    // The title is no longer in the payload, so the long line is now a
    // long map name (`LS map <name>`, under `MAX_LINE_CP`) rather than the
    // old `LS title` line.
    let long_map = "m".repeat(50);
    let t = Track {
        name: "any%".to_owned(),
        checkpoints: vec![
            Checkpoint {
                kind: CheckpointKind::Start,
                trigger: Trigger::Aabb(aabb((64, 40, 128), (66, 43, 130))),
                label: "start".to_owned(),
            },
            Checkpoint {
                kind: CheckpointKind::Split,
                trigger: Trigger::Aabb(aabb((100, 40, 200), (102, 43, 202))),
                label: "cp1".to_owned(),
            },
            Checkpoint {
                kind: CheckpointKind::Split,
                trigger: Trigger::MapLoaded(long_map.clone()),
                label: "map".to_owned(),
            },
            Checkpoint {
                kind: CheckpointKind::End,
                trigger: Trigger::Aabb(aabb((200, 50, 400), (204, 52, 404))),
                label: "end".to_owned(),
            },
        ],
    };

    let canonical = payload::serialize_canonical(&t).unwrap();
    // Multi-line payload with a long single line, exactly as on disk.
    assert!(canonical.matches('\n').count() >= 5);
    assert!(canonical.lines().any(|l| l.chars().count() > 50));

    let xml = build_lss_xml(&t, "Srv", "Map", &canonical).unwrap();
    let run = parse(&xml).expect("re-parse");
    let stored = run
        .metadata()
        .custom_variable_value(CUSTOM_VAR_NAME)
        .expect("ClassiCubeTrack present");
    // Byte-for-byte: every newline and the long line survive save + load.
    assert_eq!(stored, canonical);

    // Geometry decodes back to the original kinds + triggers.
    let title = run.category_name().to_owned();
    let labels: Vec<String> = run.segments().iter().map(|s| s.name().to_owned()).collect();
    let back = payload::parse(stored, title, labels).unwrap();
    assert_eq!(back.checkpoints.len(), t.checkpoints.len());
    for (b, o) in back.checkpoints.iter().zip(t.checkpoints.iter()) {
        assert_eq!(b.kind, o.kind);
        assert_eq!(b.trigger, o.trigger);
    }
}
