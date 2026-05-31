use super::*;

const TEST_MAP: &str = "test_map";

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

fn aabb(min: (f32, f32, f32), max: (f32, f32, f32)) -> Aabb {
    Aabb {
        min: v(min.0, min.1, min.2),
        max: v(max.0, max.1, max.2),
    }
}

fn cp(kind: CheckpointKind, min: (f32, f32, f32), max: (f32, f32, f32)) -> Checkpoint {
    cp_on(kind, min, max, None)
}

fn cp_on(
    kind: CheckpointKind,
    min: (f32, f32, f32),
    max: (f32, f32, f32),
    map: Option<&str>,
) -> Checkpoint {
    Checkpoint {
        kind,
        trigger: Trigger::Aabb {
            aabb: aabb(min, max),
            map: map.map(str::to_string),
        },
        label: String::new(),
    }
}

fn cp_map(kind: CheckpointKind, name: &str) -> Checkpoint {
    Checkpoint {
        kind,
        trigger: Trigger::MapLoaded(name.to_string()),
        label: String::new(),
    }
}

fn linear_track() -> Track {
    Track {
        name: "linear".into(),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
            cp(CheckpointKind::Split, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0)),
            cp(CheckpointKind::Split, (20.0, 0.0, 0.0), (22.0, 4.0, 2.0)),
            cp(CheckpointKind::End, (30.0, 0.0, 0.0), (32.0, 4.0, 2.0)),
        ],
    }
}

fn run(positions: &[Vec3]) -> Vec<Command> {
    let mut state = SplitsState::default();
    state.load(linear_track(), Some(TEST_MAP.to_string()));
    let mut out = Vec::new();
    for p in positions {
        step(&mut state, *p, Some(TEST_MAP), |c| out.push(c));
    }
    out
}

fn variant_name(c: &Command) -> &'static str {
    match c {
        Command::Start => "Start",
        Command::Split => "Split",
        _ => "Other",
    }
}

fn names(cmds: &[Command]) -> Vec<&'static str> {
    cmds.iter().map(variant_name).collect()
}

#[test]
fn contains_point_inside() {
    assert!(aabb((0.0, 0.0, 0.0), (1.0, 1.0, 1.0)).contains(v(0.5, 0.5, 0.5)));
}

#[test]
fn contains_min_corner_is_inside() {
    // Half-open: the min corner is included.
    assert!(aabb((0.0, 0.0, 0.0), (1.0, 1.0, 1.0)).contains(v(0.0, 0.0, 0.0)));
}

#[test]
fn contains_max_corner_is_outside() {
    // Half-open: the max corner is excluded so adjacent boxes don't overlap.
    assert!(!aabb((0.0, 0.0, 0.0), (1.0, 1.0, 1.0)).contains(v(1.0, 1.0, 1.0)));
}

#[test]
fn contains_just_outside_each_axis() {
    let b = aabb((0.0, 0.0, 0.0), (1.0, 1.0, 1.0));
    assert!(!b.contains(v(-0.1, 0.5, 0.5)));
    assert!(!b.contains(v(0.5, -0.1, 0.5)));
    assert!(!b.contains(v(0.5, 0.5, -0.1)));
    assert!(!b.contains(v(1.1, 0.5, 0.5)));
    assert!(!b.contains(v(0.5, 1.1, 0.5)));
    assert!(!b.contains(v(0.5, 0.5, 1.1)));
}

#[test]
fn aabb_new_canonicalizes_swapped_corners() {
    let a = Aabb::new(v(5.0, 6.0, 7.0), v(1.0, 2.0, 3.0));
    let b = Aabb::new(v(1.0, 2.0, 3.0), v(5.0, 6.0, 7.0));
    assert_eq!(a, b);
    assert_eq!(a.min, v(1.0, 2.0, 3.0));
    assert_eq!(a.max, v(5.0, 6.0, 7.0));
}

#[test]
fn sequential_traversal_fires_each_box_once() {
    let cmds = run(&[
        v(-5.0, 0.0, 0.0),
        v(1.0, 1.0, 1.0),  // Start
        v(11.0, 1.0, 1.0), // Split 1
        v(21.0, 1.0, 1.0), // Split 2
        v(31.0, 1.0, 1.0), // End
    ]);
    assert_eq!(names(&cmds), vec!["Start", "Split", "Split", "Split"]);
}

#[test]
fn walking_back_through_earlier_split_does_not_refire() {
    let cmds = run(&[
        v(1.0, 1.0, 1.0),  // Start
        v(11.0, 1.0, 1.0), // Split 1
        v(-5.0, 0.0, 0.0), // leave
        v(11.0, 1.0, 1.0), // re-enter Split 1
    ]);
    assert_eq!(names(&cmds), vec!["Start", "Split"]);
}

#[test]
fn standing_still_inside_box_fires_exactly_once() {
    let cmds = run(&[v(1.0, 1.0, 1.0), v(1.0, 1.0, 1.0), v(1.0, 1.0, 1.0)]);
    assert_eq!(names(&cmds), vec!["Start"]);
}

#[test]
fn skipping_to_split_two_fires_nothing() {
    // Player skips Start and Split₁, jumps straight to Split₂.
    let cmds = run(&[v(-5.0, 0.0, 0.0), v(21.0, 1.0, 1.0)]);
    assert!(cmds.is_empty(), "expected no commands, got {cmds:?}");
}

#[test]
fn end_without_first_hitting_splits_does_not_fire() {
    let cmds = run(&[v(-5.0, 0.0, 0.0), v(31.0, 1.0, 1.0)]);
    assert!(cmds.is_empty(), "expected no commands, got {cmds:?}");
}

#[test]
fn reentering_start_rearms_the_run() {
    let mut state = SplitsState::default();
    state.load(linear_track(), Some(TEST_MAP.to_string()));
    let mut cmds = Vec::new();

    let positions = [
        v(1.0, 1.0, 1.0),  // Start
        v(11.0, 1.0, 1.0), // Split 1
        v(21.0, 1.0, 1.0), // Split 2
        v(-5.0, 0.0, 0.0), // leave
        v(1.0, 1.0, 1.0),  // back to Start → re-arm
    ];
    for p in &positions {
        step(&mut state, *p, Some(TEST_MAP), |c| cmds.push(c));
    }

    assert_eq!(names(&cmds), vec!["Start", "Split", "Split", "Start"]);
    assert_eq!(state.next_index, 1);
    assert_eq!(state.fired, vec![true, false, false, false]);
}

#[test]
fn no_commands_when_no_track_loaded() {
    let mut state = SplitsState::default();
    let mut cmds = Vec::new();
    step(&mut state, v(0.0, 0.0, 0.0), Some(TEST_MAP), |c| {
        cmds.push(c)
    });
    assert!(cmds.is_empty());
}

#[test]
fn rearm_clears_fired_and_resets_cursor() {
    let mut state = SplitsState::default();
    state.load(linear_track(), Some(TEST_MAP.to_string()));
    step(&mut state, v(1.0, 1.0, 1.0), Some(TEST_MAP), |_| {});
    step(&mut state, v(11.0, 1.0, 1.0), Some(TEST_MAP), |_| {});
    assert_eq!(state.next_index, 2);
    assert_eq!(state.fired, vec![true, true, false, false]);

    state.rearm();
    assert_eq!(state.next_index, 0);
    assert_eq!(state.fired, vec![false; 4]);
    assert_eq!(state.last_inside, vec![false; 4]);
}

#[test]
fn aabb_wire_round_trip() {
    let original = aabb((10.0, 20.0, 30.0), (15.0, 24.0, 32.0));
    let (min, size) = aabb_to_min_size(original).unwrap();
    assert_eq!(min, [10, 20, 30]);
    assert_eq!(size, [5, 4, 2]);
    let decoded = aabb_from_min_size(min, size);
    assert_eq!(decoded, original);
}

#[test]
fn aabb_wire_rejects_oversize_extent() {
    let bad = aabb((0.0, 0.0, 0.0), (300.0, 1.0, 1.0));
    assert!(aabb_to_min_size(bad).is_err());
}

#[test]
fn multi_map_route_progresses() {
    let track = Track {
        name: "multi".into(),
        checkpoints: vec![
            cp_map(CheckpointKind::Start, "a"),
            cp_on(
                CheckpointKind::Split,
                (10.0, 0.0, 0.0),
                (12.0, 4.0, 2.0),
                Some("a"),
            ),
            cp_map(CheckpointKind::End, "b"),
        ],
    };
    let mut state = SplitsState::default();
    state.load(track, Some("a".to_string()));
    let mut cmds = Vec::new();
    step_on_map_loaded(&mut state, "a", |c| cmds.push(c)); // Start
    step(&mut state, v(11.0, 1.0, 1.0), Some("a"), |c| cmds.push(c)); // middle Split (box)
    step_on_map_loaded(&mut state, "b", |c| cmds.push(c)); // End
    assert_eq!(names(&cmds), vec!["Start", "Split", "Split"]);
    assert_eq!(state.fired, vec![true, true, true]);
    assert_eq!(state.next_index, 3);
}

#[test]
fn incidental_map_load_does_not_clear_run() {
    let mut state = SplitsState::default();
    state.load(linear_track(), Some(TEST_MAP.to_string()));
    let mut cmds = Vec::new();
    step(&mut state, v(1.0, 1.0, 1.0), Some(TEST_MAP), |c| {
        cmds.push(c)
    }); // Start
    step(&mut state, v(11.0, 1.0, 1.0), Some(TEST_MAP), |c| {
        cmds.push(c)
    }); // Split₁
    assert_eq!(state.next_index, 2);
    assert_eq!(state.fired, vec![true, true, false, false]);

    step_on_map_loaded(&mut state, "unrelated", |c| cmds.push(c));
    assert_eq!(state.next_index, 2, "next_index must survive map load");
    assert_eq!(state.fired, vec![true, true, false, false]);
    assert_eq!(names(&cmds), vec!["Start", "Split"]);
}

#[test]
fn start_map_rearms_after_completion() {
    let track = Track {
        name: "loop".into(),
        checkpoints: vec![
            cp_map(CheckpointKind::Start, "spawn"),
            cp_map(CheckpointKind::End, "goal"),
        ],
    };
    let mut state = SplitsState::default();
    state.load(track, Some("spawn".to_string()));
    let mut cmds = Vec::new();
    step_on_map_loaded(&mut state, "spawn", |c| cmds.push(c));
    step_on_map_loaded(&mut state, "goal", |c| cmds.push(c));
    assert_eq!(state.fired, vec![true, true]);
    assert_eq!(state.next_index, 2);

    step_on_map_loaded(&mut state, "spawn", |c| cmds.push(c));
    assert_eq!(names(&cmds), vec!["Start", "Split", "Start"]);
    assert_eq!(state.fired, vec![true, false]);
    assert_eq!(state.next_index, 1);
}

#[test]
fn non_matching_map_name_is_noop() {
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp_map(CheckpointKind::Start, "a"),
            cp_map(CheckpointKind::End, "b"),
        ],
    };
    let mut state = SplitsState::default();
    state.load(track, Some("a".to_string()));
    let mut cmds = Vec::new();
    step_on_map_loaded(&mut state, "nowhere", |c| cmds.push(c));
    assert!(cmds.is_empty());
    assert_eq!(state.next_index, 0);
    assert_eq!(state.fired, vec![false, false]);
}

#[test]
fn map_loaded_split_only_fires_when_at_cursor() {
    // Track is [Start MapLoaded "a", Split MapLoaded "b"].
    // Loading "b" before "a" must not advance.
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp_map(CheckpointKind::Start, "a"),
            cp_map(CheckpointKind::End, "b"),
        ],
    };
    let mut state = SplitsState::default();
    state.load(track, Some("a".to_string()));
    let mut cmds = Vec::new();
    step_on_map_loaded(&mut state, "b", |c| cmds.push(c));
    assert!(cmds.is_empty());
    assert_eq!(state.next_index, 0);
}

// ---- AABB map scoping ----

#[test]
fn aabb_with_none_map_resolves_to_starting_map() {
    let mut state = SplitsState::default();
    state.load(linear_track(), Some("home".to_string()));
    let mut cmds = Vec::new();
    // World matches the captured starting map → fires.
    step(&mut state, v(1.0, 1.0, 1.0), Some("home"), |c| cmds.push(c));
    assert_eq!(names(&cmds), vec!["Start"]);
}

#[test]
fn aabb_with_none_map_does_not_fire_on_different_world() {
    let mut state = SplitsState::default();
    state.load(linear_track(), Some("home".to_string()));
    let mut cmds = Vec::new();
    step(&mut state, v(1.0, 1.0, 1.0), Some("away"), |c| cmds.push(c));
    assert!(cmds.is_empty());
    assert_eq!(state.next_index, 0);
    assert_eq!(state.fired, vec![false; 4]);
}

#[test]
fn aabb_with_some_map_only_fires_on_matching_world() {
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp_on(
                CheckpointKind::Start,
                (0.0, 0.0, 0.0),
                (2.0, 4.0, 2.0),
                Some("here"),
            ),
            cp_on(
                CheckpointKind::End,
                (10.0, 0.0, 0.0),
                (12.0, 4.0, 2.0),
                Some("here"),
            ),
        ],
    };
    let mut state = SplitsState::default();
    state.load(track, Some("starting".to_string()));
    let mut cmds = Vec::new();
    // Inside Start AABB but on "starting" — doesn't match Some("here").
    step(&mut state, v(1.0, 1.0, 1.0), Some("starting"), |c| {
        cmds.push(c)
    });
    assert!(cmds.is_empty());
    // Switch to the right world; same position now fires.
    step(&mut state, v(1.0, 1.0, 1.0), Some("here"), |c| cmds.push(c));
    assert_eq!(names(&cmds), vec!["Start"]);
}

#[test]
fn aabb_skipped_when_world_is_none() {
    let mut state = SplitsState::default();
    state.load(linear_track(), Some("home".to_string()));
    let mut cmds = Vec::new();
    // No world known yet (e.g. singleplayer pre-load); nothing fires.
    step(&mut state, v(1.0, 1.0, 1.0), None, |c| cmds.push(c));
    assert!(cmds.is_empty());
}

#[test]
fn aabb_skipped_when_starting_map_unset_and_cp_map_none() {
    let mut state = SplitsState::default();
    state.load(linear_track(), None);
    let mut cmds = Vec::new();
    step(&mut state, v(1.0, 1.0, 1.0), Some("anywhere"), |c| {
        cmds.push(c)
    });
    assert!(cmds.is_empty());
}

#[test]
fn cross_map_aabb_route_only_fires_on_correct_sections() {
    // Mirrors the user's example: 3 AABBs on starting map, MapLoaded
    // split, 3 AABBs on "mapname" (last is End). Coords are identical
    // across sections to prove the map scope discriminates.
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp_on(
                CheckpointKind::Start,
                (0.0, 0.0, 0.0),
                (2.0, 4.0, 2.0),
                None,
            ),
            cp_on(
                CheckpointKind::Split,
                (10.0, 0.0, 0.0),
                (12.0, 4.0, 2.0),
                None,
            ),
            cp_on(
                CheckpointKind::Split,
                (20.0, 0.0, 0.0),
                (22.0, 4.0, 2.0),
                None,
            ),
            cp_map(CheckpointKind::Split, "mapname"),
            cp_on(
                CheckpointKind::Split,
                (0.0, 0.0, 0.0),
                (2.0, 4.0, 2.0),
                Some("mapname"),
            ),
            cp_on(
                CheckpointKind::Split,
                (10.0, 0.0, 0.0),
                (12.0, 4.0, 2.0),
                Some("mapname"),
            ),
            cp_on(
                CheckpointKind::End,
                (20.0, 0.0, 0.0),
                (22.0, 4.0, 2.0),
                Some("mapname"),
            ),
        ],
    };
    let mut state = SplitsState::default();
    state.load(track, Some("starting".to_string()));
    let mut cmds = Vec::new();

    // Walk all three section-0 AABBs while on starting map.
    step(&mut state, v(1.0, 1.0, 1.0), Some("starting"), |c| {
        cmds.push(c)
    });
    step(&mut state, v(11.0, 1.0, 1.0), Some("starting"), |c| {
        cmds.push(c)
    });
    step(&mut state, v(21.0, 1.0, 1.0), Some("starting"), |c| {
        cmds.push(c)
    });
    assert_eq!(names(&cmds), vec!["Start", "Split", "Split"]);

    // Crossing into mapname fires the MapLoaded Split.
    step_on_map_loaded(&mut state, "mapname", |c| cmds.push(c));
    assert_eq!(
        names(&cmds),
        vec!["Start", "Split", "Split", "Split"],
        "MapLoaded split should fire on map change",
    );

    // Now walking the section-1 AABBs (same coords as section 0) fires
    // the section-1 cps, not the section-0 ones.
    step(&mut state, v(1.0, 1.0, 1.0), Some("mapname"), |c| {
        cmds.push(c)
    });
    step(&mut state, v(11.0, 1.0, 1.0), Some("mapname"), |c| {
        cmds.push(c)
    });
    step(&mut state, v(21.0, 1.0, 1.0), Some("mapname"), |c| {
        cmds.push(c)
    });
    assert_eq!(
        names(&cmds),
        vec![
            "Start", "Split", "Split", "Split", "Split", "Split", "Split",
        ],
    );
    assert_eq!(state.next_index, 7);
}

#[test]
fn section1_aabb_does_not_fire_while_still_on_starting_map() {
    // Track has a section-1 AABB at coords identical to a section-0
    // AABB; while on the starting map, walking the coords must only
    // fire section 0.
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp_on(
                CheckpointKind::Start,
                (0.0, 0.0, 0.0),
                (2.0, 4.0, 2.0),
                None,
            ),
            cp_map(CheckpointKind::Split, "next"),
            cp_on(
                CheckpointKind::End,
                (0.0, 0.0, 0.0),
                (2.0, 4.0, 2.0),
                Some("next"),
            ),
        ],
    };
    let mut state = SplitsState::default();
    state.load(track, Some("home".to_string()));
    let mut cmds = Vec::new();
    // On "home": Start fires; End is scoped to "next", doesn't fire.
    step(&mut state, v(1.0, 1.0, 1.0), Some("home"), |c| cmds.push(c));
    assert_eq!(names(&cmds), vec!["Start"]);
    assert_eq!(state.next_index, 1);
}

#[test]
fn unload_clears_starting_map() {
    let mut state = SplitsState::default();
    state.load(linear_track(), Some("home".to_string()));
    assert_eq!(state.starting_map.as_deref(), Some("home"));
    state.unload();
    assert!(state.starting_map.is_none());
    assert!(state.track.is_none());
}
