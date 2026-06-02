use super::*;

const TEST_MAP: &str = "test_map";

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

/// Fixed model collision size for the position-driven tests: the default
/// human box (`DEFAULT_PLAYER_SIZE`, ~0.54 x 1.76 x 0.54 blocks). Small
/// enough that a feet position deep inside one checkpoint AABB never
/// spills into a neighbor -- the suite's checkpoints are >= 8 blocks
/// apart.
const TEST_SIZE: Vec3 = DEFAULT_PLAYER_SIZE;

/// Feet-anchored model collision box at `pos` -- the shape `step()` now
/// consumes. Lets the position-driven tests keep expressing the player
/// as a single feet `Vec3` while exercising the real `player_bounds` +
/// `intersects` collision path.
fn pbox(pos: Vec3) -> Aabb {
    player_bounds(pos, TEST_SIZE)
}

/// Test wrapper around `step()` for the bulk of the suite that doesn't
/// exercise Pause/Resume kinds. Builds the feet-box via [`pbox`] and
/// passes no-op `on_pause` / `on_resume` callbacks; tests that care about
/// the pause-counter side effects call `step()` directly with real
/// callbacks.
fn tstep<F: FnMut(Command)>(state: &mut SplitsState, pos: Vec3, world: Option<&str>, send: F) {
    step(state, pbox(pos), world, send, || {}, || {});
}

fn aabb(min: (f32, f32, f32), max: (f32, f32, f32)) -> Aabb {
    Aabb {
        min: v(min.0, min.1, min.2),
        max: v(max.0, max.1, max.2),
    }
}

fn cp(kind: CheckpointKind, min: (f32, f32, f32), max: (f32, f32, f32)) -> Checkpoint {
    Checkpoint {
        kind,
        trigger: Trigger::Aabb(aabb(min, max)),
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
        tstep(&mut state, *p, Some(TEST_MAP), |c| out.push(c));
    }
    out
}

fn variant_name(c: &Command) -> Option<&'static str> {
    match c {
        Command::Start => Some("Start"),
        Command::Split => Some("Split"),
        // SetCurrentTimingMethod + InitializeGameTime are bundled with
        // `Start` and asserted separately in
        // `start_is_bundled_with_set_timing_method_and_initialize_game_time`;
        // filter them out of the sequence helper so geometry tests can
        // continue to assert against fire-event names alone.
        Command::SetCurrentTimingMethod { .. } | Command::InitializeGameTime => None,
        _ => Some("Other"),
    }
}

fn names(cmds: &[Command]) -> Vec<&'static str> {
    cmds.iter().filter_map(variant_name).collect()
}

#[test]
fn intersects_overlapping_boxes() {
    let a = aabb((0.0, 0.0, 0.0), (2.0, 2.0, 2.0));
    let b = aabb((1.0, 1.0, 1.0), (3.0, 3.0, 3.0));
    assert!(a.intersects(&b));
    assert!(b.intersects(&a), "intersects is symmetric");
}

#[test]
fn intersects_disjoint_boxes() {
    let a = aabb((0.0, 0.0, 0.0), (1.0, 1.0, 1.0));
    // Separated on X only.
    assert!(!a.intersects(&aabb((5.0, 0.0, 0.0), (6.0, 1.0, 1.0))));
    // Separated on Y only.
    assert!(!a.intersects(&aabb((0.0, 5.0, 0.0), (1.0, 6.0, 1.0))));
    // Separated on Z only.
    assert!(!a.intersects(&aabb((0.0, 0.0, 5.0), (1.0, 1.0, 6.0))));
}

#[test]
fn intersects_face_touching_is_true() {
    // Closed overlap (server's `AABB.Intersects` uses `>=`/`<=`): boxes
    // sharing exactly the x = 1 face count as intersecting. Sequential
    // firing (next_index) keeps adjacent checkpoints from both firing.
    let a = aabb((0.0, 0.0, 0.0), (1.0, 1.0, 1.0));
    let b = aabb((1.0, 0.0, 0.0), (2.0, 1.0, 1.0));
    assert!(a.intersects(&b));
    assert!(b.intersects(&a));
}

#[test]
fn player_bounds_centers_xz_on_feet() {
    let b = player_bounds(v(10.0, 5.0, 20.0), v(0.6, 1.8, 0.6));
    assert_eq!(b.min.x, 10.0 - 0.3);
    assert_eq!(b.max.x, 10.0 + 0.3);
    assert_eq!(b.min.z, 20.0 - 0.3);
    assert_eq!(b.max.z, 20.0 + 0.3);
}

#[test]
fn player_bounds_anchors_y_from_feet_to_feet_plus_height() {
    // Y is NOT centered: it runs from the feet up to feet + height, like
    // the engine's `AABB_Make` (the feet are the model's anchor).
    let b = player_bounds(v(10.0, 5.0, 20.0), v(0.6, 1.8, 0.6));
    assert_eq!(b.min.y, 5.0);
    assert_eq!(b.max.y, 5.0 + 1.8);
}

#[test]
fn aabb_new_canonicalizes_swapped_corners() {
    let a = Aabb::new(v(5.0, 6.0, 7.0), v(1.0, 2.0, 3.0));
    let b = Aabb::new(v(1.0, 2.0, 3.0), v(5.0, 6.0, 7.0));
    assert_eq!(a, b);
    assert_eq!(a.min, v(1.0, 2.0, 3.0));
    assert_eq!(a.max, v(5.0, 6.0, 7.0));
}

// ---- message-block collision parity (model-box overlap) ----

#[test]
fn step_fires_head_height_checkpoint_via_model_height() {
    // A checkpoint floating at head height (Y in [2, 3)) like a message
    // block the player walks into. The feet stay on the ground, but the
    // model box reaches up to feet + height, so the checkpoint fires when
    // (and only when) that height reaches into it.
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 2.0, 0.0), (2.0, 3.0, 2.0)),
            cp(CheckpointKind::End, (10.0, 2.0, 0.0), (12.0, 3.0, 2.0)),
        ],
    };

    // Feet at 0.5 -> box top 0.5 + 1.75625 = 2.25625, into [2, 3) -> fires.
    let mut state = SplitsState::default();
    state.load(track.clone(), Some(TEST_MAP.to_string()));
    let mut cmds = Vec::new();
    tstep(&mut state, v(1.0, 0.5, 1.0), Some(TEST_MAP), |c| {
        cmds.push(c)
    });
    assert_eq!(names(&cmds), vec!["Start"]);

    // Feet at 0.0 -> box top 1.75625, short of Y = 2 -> nothing fires even
    // though the feet are directly below the checkpoint.
    let mut state = SplitsState::default();
    state.load(track, Some(TEST_MAP.to_string()));
    let mut cmds = Vec::new();
    tstep(&mut state, v(1.0, 0.0, 1.0), Some(TEST_MAP), |c| {
        cmds.push(c)
    });
    assert!(
        cmds.is_empty(),
        "model box top falls short of the checkpoint: {cmds:?}"
    );
}

#[test]
fn wide_model_overlaps_checkpoint_a_narrow_model_misses() {
    // Pure player_bounds + intersects: a checkpoint sits beside the feet
    // (x in [3, 4)). A narrow model's box stops short; a wide model (large
    // size.x) reaches into it -- detection is model-scale dependent, which
    // is the whole point of reading the live `Entity.Size`.
    let cp_box = aabb((3.0, 0.0, 0.0), (4.0, 4.0, 1.0));
    let feet = v(2.0, 1.0, 0.5);

    // Narrow: half-width 0.3 -> box x max 2.3, short of 3.0.
    let narrow = player_bounds(feet, v(0.6, 1.8, 0.6));
    assert!(!cp_box.intersects(&narrow));

    // Wide: half-width 1.5 -> box x max 3.5, into [3, 4).
    let wide = player_bounds(feet, v(3.0, 1.8, 0.6));
    assert!(cp_box.intersects(&wide));
}

#[test]
fn start_is_bundled_with_set_timing_method_and_initialize_game_time() {
    // Bundled with `Command::Start`: every Start fired by the geometry
    // layer emits `SetCurrentTimingMethod { GameTime }` first (so the
    // timer is in game-time mode before the run begins) and
    // `InitializeGameTime` immediately after (so LSO's game-time field
    // becomes `Some(0)` instead of staying blank until a later
    // pause/resume-game-time accidentally initializes it as a side
    // effect via `set_loading_times`). Order matters: `Start` must come
    // before `InitializeGameTime` because livesplit-core's
    // `initialize_game_time()` errors with `NoRunInProgress` otherwise.
    use crate::plugin::livesplit::protocol::TimingMethod;

    // AABB Start.
    let mut state = SplitsState::default();
    state.load(linear_track(), Some(TEST_MAP.to_string()));
    let mut cmds = Vec::new();
    tstep(&mut state, v(1.0, 1.0, 1.0), Some(TEST_MAP), |c| {
        cmds.push(c)
    });
    assert!(
        matches!(
            cmds.as_slice(),
            [
                Command::SetCurrentTimingMethod {
                    timing_method: TimingMethod::GameTime,
                },
                Command::Start,
                Command::InitializeGameTime,
            ]
        ),
        "AABB Start bundling: {cmds:?}",
    );

    // MapLoaded Start.
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
    step_on_map_loaded(&mut state, "a", |c| cmds.push(c));
    assert!(
        matches!(
            cmds.as_slice(),
            [
                Command::SetCurrentTimingMethod {
                    timing_method: TimingMethod::GameTime,
                },
                Command::Start,
                Command::InitializeGameTime,
            ]
        ),
        "MapLoaded Start bundling: {cmds:?}",
    );
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
        tstep(&mut state, *p, Some(TEST_MAP), |c| cmds.push(c));
    }

    assert_eq!(names(&cmds), vec!["Start", "Split", "Split", "Start"]);
    assert_eq!(state.next_index, 1);
    assert_eq!(state.fired, vec![true, false, false, false]);
}

#[test]
fn no_commands_when_no_track_loaded() {
    let mut state = SplitsState::default();
    let mut cmds = Vec::new();
    tstep(&mut state, v(0.0, 0.0, 0.0), Some(TEST_MAP), |c| {
        cmds.push(c)
    });
    assert!(cmds.is_empty());
}

#[test]
fn rearm_clears_fired_and_resets_cursor() {
    let mut state = SplitsState::default();
    state.load(linear_track(), Some(TEST_MAP.to_string()));
    tstep(&mut state, v(1.0, 1.0, 1.0), Some(TEST_MAP), |_| {});
    tstep(&mut state, v(11.0, 1.0, 1.0), Some(TEST_MAP), |_| {});
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
            cp(CheckpointKind::Split, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0)),
            cp_map(CheckpointKind::End, "b"),
        ],
    };
    let mut state = SplitsState::default();
    state.load(track, Some("a".to_string()));
    let mut cmds = Vec::new();
    step_on_map_loaded(&mut state, "a", |c| cmds.push(c)); // Start
    tstep(&mut state, v(11.0, 1.0, 1.0), Some("a"), |c| cmds.push(c)); // middle Split (box)
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
    tstep(&mut state, v(1.0, 1.0, 1.0), Some(TEST_MAP), |c| {
        cmds.push(c)
    }); // Start
    tstep(&mut state, v(11.0, 1.0, 1.0), Some(TEST_MAP), |c| {
        cmds.push(c)
    }); // Split₁
    assert_eq!(state.next_index, 2);
    assert_eq!(state.fired, vec![true, true, false, false]);

    // Reloading the same starting map (e.g. the player gets respawned)
    // is on-route and must not clear the run.
    step_on_map_loaded(&mut state, TEST_MAP, |c| cmds.push(c));
    assert_eq!(state.next_index, 2, "next_index must survive map load");
    assert_eq!(state.fired, vec![true, true, false, false]);
    assert_eq!(names(&cmds), vec!["Start", "Split"]);
}

#[test]
fn off_route_map_load_resets_in_progress_run() {
    // Track has no MapLoaded checkpoints, so the valid map set is just
    // `starting_map`. Loading any other map mid-run is off-route → the
    // run is aborted with a Reset.
    let mut state = SplitsState::default();
    state.load(linear_track(), Some(TEST_MAP.to_string()));
    let mut cmds = Vec::new();
    tstep(&mut state, v(1.0, 1.0, 1.0), Some(TEST_MAP), |c| {
        cmds.push(c)
    }); // Start
    tstep(&mut state, v(11.0, 1.0, 1.0), Some(TEST_MAP), |c| {
        cmds.push(c)
    }); // Split₁
    assert_eq!(state.next_index, 2);

    step_on_map_loaded(&mut state, "unrelated", |c| cmds.push(c));
    assert!(matches!(cmds.last(), Some(Command::Reset { .. })));
    assert_eq!(state.next_index, 0, "off-route warp re-arms the run");
    assert_eq!(state.fired, vec![false; 4]);
}

#[test]
fn off_route_map_load_before_run_starts_is_silent() {
    // `next_index == 0` and no fired flags → no run in progress; warping
    // off-route just leaves the track ready for a fresh attempt.
    let mut state = SplitsState::default();
    state.load(linear_track(), Some(TEST_MAP.to_string()));
    let mut cmds = Vec::new();
    step_on_map_loaded(&mut state, "unrelated", |c| cmds.push(c));
    assert!(cmds.is_empty());
    assert_eq!(state.next_index, 0);
    assert_eq!(state.fired, vec![false; 4]);
}

#[test]
fn off_route_map_load_after_completion_is_silent() {
    // Run completed (`next_index == n`); warping off-route mustn't undo
    // the celebration by sending a spurious Reset.
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
    assert_eq!(state.next_index, 2);
    let before = cmds.len();

    step_on_map_loaded(&mut state, "lobby", |c| cmds.push(c));
    assert_eq!(cmds.len(), before, "no Reset sent after completion");
    assert_eq!(state.next_index, 2);
    assert_eq!(state.fired, vec![true, true]);
}

#[test]
fn mapload_checkpoint_map_is_on_route_even_if_not_yet_eligible() {
    // Loading a future MapLoaded checkpoint's map while still earlier
    // in the run is on-route (no Reset) — sequential firing handles
    // the "wrong step" case by silently not firing.
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
            cp_map(CheckpointKind::Split, "mid"),
            cp_map(CheckpointKind::End, "goal"),
        ],
    };
    let mut state = SplitsState::default();
    state.load(track, Some("home".to_string()));
    let mut cmds = Vec::new();
    tstep(&mut state, v(1.0, 1.0, 1.0), Some("home"), |c| cmds.push(c)); // Start
    assert_eq!(state.next_index, 1);

    // Skip past "mid" straight to "goal" — on-route (in MapLoaded set)
    // but not the expected next step, so it's a no-op rather than a
    // reset.
    step_on_map_loaded(&mut state, "goal", |c| cmds.push(c));
    assert_eq!(names(&cmds), vec!["Start"]);
    assert_eq!(state.next_index, 1, "next_index must not advance");
    assert_eq!(state.fired, vec![true, false, false]);
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
fn aabb_with_no_preceding_mapload_resolves_to_starting_map() {
    let mut state = SplitsState::default();
    state.load(linear_track(), Some("home".to_string()));
    let mut cmds = Vec::new();
    // World matches the captured starting map → fires.
    tstep(&mut state, v(1.0, 1.0, 1.0), Some("home"), |c| cmds.push(c));
    assert_eq!(names(&cmds), vec!["Start"]);
}

#[test]
fn aabb_does_not_fire_on_different_world_than_starting_map() {
    let mut state = SplitsState::default();
    state.load(linear_track(), Some("home".to_string()));
    let mut cmds = Vec::new();
    tstep(&mut state, v(1.0, 1.0, 1.0), Some("away"), |c| cmds.push(c));
    assert!(cmds.is_empty());
    assert_eq!(state.next_index, 0);
    assert_eq!(state.fired, vec![false; 4]);
}

#[test]
fn aabb_skipped_when_world_is_none() {
    let mut state = SplitsState::default();
    state.load(linear_track(), Some("home".to_string()));
    let mut cmds = Vec::new();
    // No world known yet (e.g. singleplayer pre-load); nothing fires.
    tstep(&mut state, v(1.0, 1.0, 1.0), None, |c| cmds.push(c));
    assert!(cmds.is_empty());
}

#[test]
fn aabb_skipped_when_starting_map_unset_and_no_preceding_mapload() {
    let mut state = SplitsState::default();
    state.load(linear_track(), None);
    let mut cmds = Vec::new();
    tstep(&mut state, v(1.0, 1.0, 1.0), Some("anywhere"), |c| {
        cmds.push(c)
    });
    assert!(cmds.is_empty());
}

#[test]
fn cross_map_aabb_route_only_fires_on_correct_sections() {
    // Mirrors the user's example: 3 AABBs on starting map, MapLoaded
    // split, 3 AABBs on "mapname" (last is End). Coords are identical
    // across sections to prove the derived-scope walk discriminates.
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
            cp(CheckpointKind::Split, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0)),
            cp(CheckpointKind::Split, (20.0, 0.0, 0.0), (22.0, 4.0, 2.0)),
            cp_map(CheckpointKind::Split, "mapname"),
            cp(CheckpointKind::Split, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
            cp(CheckpointKind::Split, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0)),
            cp(CheckpointKind::End, (20.0, 0.0, 0.0), (22.0, 4.0, 2.0)),
        ],
    };
    let mut state = SplitsState::default();
    state.load(track, Some("starting".to_string()));
    let mut cmds = Vec::new();

    // Walk all three section-0 AABBs while on starting map.
    tstep(&mut state, v(1.0, 1.0, 1.0), Some("starting"), |c| {
        cmds.push(c)
    });
    tstep(&mut state, v(11.0, 1.0, 1.0), Some("starting"), |c| {
        cmds.push(c)
    });
    tstep(&mut state, v(21.0, 1.0, 1.0), Some("starting"), |c| {
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
    tstep(&mut state, v(1.0, 1.0, 1.0), Some("mapname"), |c| {
        cmds.push(c)
    });
    tstep(&mut state, v(11.0, 1.0, 1.0), Some("mapname"), |c| {
        cmds.push(c)
    });
    tstep(&mut state, v(21.0, 1.0, 1.0), Some("mapname"), |c| {
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
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
            cp_map(CheckpointKind::Split, "next"),
            cp(CheckpointKind::End, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
        ],
    };
    let mut state = SplitsState::default();
    state.load(track, Some("home".to_string()));
    let mut cmds = Vec::new();
    // On "home": Start fires; End is scoped to "next", doesn't fire.
    tstep(&mut state, v(1.0, 1.0, 1.0), Some("home"), |c| cmds.push(c));
    assert_eq!(names(&cmds), vec!["Start"]);
    assert_eq!(state.next_index, 1);
}

#[test]
fn unload_clears_starting_map() {
    let mut state = SplitsState::default();
    state.load(linear_track(), Some("home".to_string()));
    assert_eq!(state.starting_map.as_deref(), Some("home"));
    assert_eq!(state.last_seen_map.as_deref(), Some("home"));
    state.unload();
    assert!(state.starting_map.is_none());
    assert!(state.last_seen_map.is_none());
    assert!(state.track.is_none());
}

// ---- aabbs_on_map (HUD map-scope filter) ----

#[test]
fn aabbs_on_map_single_map_shows_all_on_load_map_none_off_it() {
    let track = linear_track();
    // `linear_track`'s checkpoints all carry an empty label. `next_index`
    // is `None` here, so every `is_next` flag is `false` (the next-flag
    // behavior is exercised in `aabbs_on_map_marks_next_index_respecting_scope`).
    let expected = vec![
        (
            CheckpointKind::Start,
            aabb((0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
            String::new(),
            false,
        ),
        (
            CheckpointKind::Split,
            aabb((10.0, 0.0, 0.0), (12.0, 4.0, 2.0)),
            String::new(),
            false,
        ),
        (
            CheckpointKind::Split,
            aabb((20.0, 0.0, 0.0), (22.0, 4.0, 2.0)),
            String::new(),
            false,
        ),
        (
            CheckpointKind::End,
            aabb((30.0, 0.0, 0.0), (32.0, 4.0, 2.0)),
            String::new(),
            false,
        ),
    ];
    // On the load map: every AABB is in scope.
    assert_eq!(
        aabbs_on_map(&track, Some("home"), Some("home"), None),
        expected,
        "all AABBs visible on the starting map"
    );
    // On a different map: a single-map track's AABBs are scoped to the
    // starting map, so none show.
    assert!(aabbs_on_map(&track, Some("home"), Some("away"), None).is_empty());
    // World unknown: nothing to anchor scope against.
    assert!(aabbs_on_map(&track, Some("home"), None, None).is_empty());
    // Starting map unknown and no preceding MapLoaded: nothing in scope.
    assert!(aabbs_on_map(&track, None, Some("home"), None).is_empty());
}

#[test]
fn aabbs_on_map_multi_map_partitions_by_scope() {
    // [Start, Split] on the starting map, then MapLoaded("mapname"),
    // then [Split, End] scoped to "mapname". Identical coords across
    // sections to prove the walk discriminates by derived scope, not
    // geometry.
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
            cp(CheckpointKind::Split, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0)),
            cp_map(CheckpointKind::Split, "mapname"),
            cp(CheckpointKind::Split, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0)),
            cp(CheckpointKind::End, (20.0, 0.0, 0.0), (22.0, 4.0, 2.0)),
        ],
    };

    assert_eq!(
        aabbs_on_map(&track, Some("starting"), Some("starting"), None),
        vec![
            (
                CheckpointKind::Start,
                aabb((0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
                String::new(),
                false,
            ),
            (
                CheckpointKind::Split,
                aabb((10.0, 0.0, 0.0), (12.0, 4.0, 2.0)),
                String::new(),
                false,
            ),
        ],
        "only the pre-transition AABBs show on the starting map"
    );

    assert_eq!(
        aabbs_on_map(&track, Some("starting"), Some("mapname"), None),
        vec![
            (
                CheckpointKind::Split,
                aabb((10.0, 0.0, 0.0), (12.0, 4.0, 2.0)),
                String::new(),
                false,
            ),
            (
                CheckpointKind::End,
                aabb((20.0, 0.0, 0.0), (22.0, 4.0, 2.0)),
                String::new(),
                false,
            ),
        ],
        "only the post-transition AABBs show on the second map"
    );

    // A map that's in neither section: nothing in scope.
    assert!(aabbs_on_map(&track, Some("starting"), Some("elsewhere"), None).is_empty());
}

#[test]
fn aabbs_on_map_marks_next_index_respecting_scope() {
    // Two map sections with a duplicate-geometry AABB across them: index 1
    // on the starting map and index 3 on "mapname" share the same box. The
    // next-flag must key off the track-wide source index, not geometry, so
    // the duplicate box is never mis-flagged.
    let start_box = aabb((0.0, 0.0, 0.0), (2.0, 4.0, 2.0));
    let mid_box = aabb((10.0, 0.0, 0.0), (12.0, 4.0, 2.0));
    let end_box = aabb((20.0, 0.0, 0.0), (22.0, 4.0, 2.0));
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)), // idx 0, starting
            cp(CheckpointKind::Split, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0)), // idx 1, starting
            cp_map(CheckpointKind::Split, "mapname"),                    // idx 2
            cp(CheckpointKind::Split, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0)), // idx 3, mapname (dup of 1)
            cp(CheckpointKind::End, (20.0, 0.0, 0.0), (22.0, 4.0, 2.0)),   // idx 4, mapname
        ],
    };

    // next = Start (idx 0) on the starting map: Start flagged, the
    // starting Split not.
    assert_eq!(
        aabbs_on_map(&track, Some("starting"), Some("starting"), Some(0)),
        vec![
            (CheckpointKind::Start, start_box, String::new(), true),
            (CheckpointKind::Split, mid_box, String::new(), false),
        ],
    );

    // next = idx 1 (the starting Split): it's the one flagged.
    assert_eq!(
        aabbs_on_map(&track, Some("starting"), Some("starting"), Some(1)),
        vec![
            (CheckpointKind::Start, start_box, String::new(), false),
            (CheckpointKind::Split, mid_box, String::new(), true),
        ],
    );

    // next = idx 1 but we're on "mapname": idx 1 is out of scope, so
    // nothing is flagged -- and the duplicate-geometry idx 3 (same box)
    // is NOT mis-flagged.
    assert_eq!(
        aabbs_on_map(&track, Some("starting"), Some("mapname"), Some(1)),
        vec![
            (CheckpointKind::Split, mid_box, String::new(), false),
            (CheckpointKind::End, end_box, String::new(), false),
        ],
        "out-of-scope next index must not mark a same-geometry box on another map",
    );

    // next = idx 3 (the mapname Split): flagged on "mapname".
    assert_eq!(
        aabbs_on_map(&track, Some("starting"), Some("mapname"), Some(3)),
        vec![
            (CheckpointKind::Split, mid_box, String::new(), true),
            (CheckpointKind::End, end_box, String::new(), false),
        ],
    );

    // next = idx 2, the MapLoaded checkpoint (no AABB): nothing flagged.
    assert!(
        aabbs_on_map(&track, Some("starting"), Some("starting"), Some(2))
            .iter()
            .all(|(.., is_next)| !is_next),
        "a MapLoaded next index matches no AABB entry",
    );

    // None: nothing flagged.
    assert!(
        aabbs_on_map(&track, Some("starting"), Some("starting"), None)
            .iter()
            .all(|(.., is_next)| !is_next),
    );
}

// ---- observe_map (tick-driven map-change detection) ----

#[test]
fn observe_map_seeds_last_seen_from_starting_map() {
    let mut state = SplitsState::default();
    state.load(linear_track(), Some("home".to_string()));
    assert_eq!(state.last_seen_map.as_deref(), Some("home"));
    // First observation matches starting_map → no fire.
    let mut cmds = Vec::new();
    observe_map(&mut state, Some("home"), |c| cmds.push(c));
    assert!(cmds.is_empty());
    assert_eq!(state.last_seen_map.as_deref(), Some("home"));
}

#[test]
fn observe_map_rearm_keeps_last_seen() {
    // rearm() must not touch last_seen_map — the physical map didn't
    // change, just the run state.
    let mut state = SplitsState::default();
    state.load(linear_track(), Some("home".to_string()));
    state.rearm();
    assert_eq!(state.last_seen_map.as_deref(), Some("home"));
}

#[test]
fn observe_map_none_world_does_not_reset_edge() {
    // World briefly becomes None (e.g. between engine reset and
    // tab-list catch-up). Must not clobber last_seen_map, so a later
    // re-observation of the same map is still a no-op.
    let mut state = SplitsState::default();
    state.load(linear_track(), Some("home".to_string()));
    let mut cmds = Vec::new();
    observe_map(&mut state, None, |c| cmds.push(c));
    assert_eq!(state.last_seen_map.as_deref(), Some("home"));
    observe_map(&mut state, Some("home"), |c| cmds.push(c));
    assert!(cmds.is_empty());
}

#[test]
fn observe_map_fires_split_on_transition_to_mapload_target() {
    // Reproduces the original bug: AABB Start fired (next_index = 1),
    // next checkpoint is `MapLoaded("next")`. When the tick observes
    // `world` flipping from "home" to "next", observe_map drives the
    // MapLoaded Split.
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
            cp_map(CheckpointKind::Split, "next"),
            cp(CheckpointKind::End, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
        ],
    };
    let mut state = SplitsState::default();
    state.load(track, Some("home".to_string()));
    let mut cmds = Vec::new();
    tstep(&mut state, v(1.0, 1.0, 1.0), Some("home"), |c| cmds.push(c)); // Start
    assert_eq!(state.next_index, 1);

    // A few ticks pass while tab-list still reports the old map — no
    // change observed, no fire.
    observe_map(&mut state, Some("home"), |c| cmds.push(c));
    observe_map(&mut state, Some("home"), |c| cmds.push(c));
    assert_eq!(names(&cmds), vec!["Start"]);

    // Tab-list catches up. observe_map detects the transition and
    // drives the MapLoaded Split.
    observe_map(&mut state, Some("next"), |c| cmds.push(c));
    assert_eq!(names(&cmds), vec!["Start", "Split"]);
    assert_eq!(state.next_index, 2);
    assert_eq!(state.last_seen_map.as_deref(), Some("next"));
}

#[test]
fn observe_map_repeated_observations_fire_once() {
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
            cp_map(CheckpointKind::End, "goal"),
        ],
    };
    let mut state = SplitsState::default();
    state.load(track, Some("home".to_string()));
    let mut cmds = Vec::new();
    tstep(&mut state, v(1.0, 1.0, 1.0), Some("home"), |c| cmds.push(c)); // Start

    // First observation transitions home→goal: End fires once.
    observe_map(&mut state, Some("goal"), |c| cmds.push(c));
    // Subsequent ticks observe the same map: must not re-fire.
    observe_map(&mut state, Some("goal"), |c| cmds.push(c));
    observe_map(&mut state, Some("goal"), |c| cmds.push(c));
    assert_eq!(names(&cmds), vec!["Start", "Split"]);
    assert_eq!(state.next_index, 2);
    assert_eq!(state.fired, vec![true, true]);
}

#[test]
fn observe_map_off_route_warp_resets_in_progress_run() {
    // Tick-side equivalent of the off-route abort case.
    let mut state = SplitsState::default();
    state.load(linear_track(), Some("home".to_string()));
    let mut cmds = Vec::new();
    tstep(&mut state, v(1.0, 1.0, 1.0), Some("home"), |c| cmds.push(c)); // Start
    tstep(&mut state, v(11.0, 1.0, 1.0), Some("home"), |c| {
        cmds.push(c);
    }); // Split₁
    assert_eq!(state.next_index, 2);

    observe_map(&mut state, Some("unrelated"), |c| cmds.push(c));
    assert!(matches!(cmds.last(), Some(Command::Reset { .. })));
    assert_eq!(state.next_index, 0);
    assert_eq!(state.fired, vec![false; 4]);
    assert_eq!(state.last_seen_map.as_deref(), Some("unrelated"));
}

// --- validate_pause_resume_pairing ---

fn track_with_kinds(kinds: &[CheckpointKind]) -> Track {
    // Geometry doesn't matter for the validator — only the `kind`
    // sequence does — so reuse one AABB for every checkpoint.
    let checkpoints = kinds
        .iter()
        .map(|k| cp(*k, (0.0, 0.0, 0.0), (1.0, 1.0, 1.0)))
        .collect();
    Track {
        name: "validator-fixture".into(),
        checkpoints,
    }
}

#[test]
fn validate_pairing_accepts_no_pause_resume() {
    use CheckpointKind::{End, Split, Start};
    let t = track_with_kinds(&[Start, Split, Split, End]);
    validate_pause_resume_pairing(&t).unwrap();
}

#[test]
fn validate_pairing_accepts_balanced_pair() {
    use CheckpointKind::{End, Pause, Resume, Split, Start};
    let t = track_with_kinds(&[Start, Pause, Split, Resume, End]);
    validate_pause_resume_pairing(&t).unwrap();
}

#[test]
fn validate_pairing_accepts_back_to_back_pairs() {
    use CheckpointKind::{End, Pause, Resume, Start};
    let t = track_with_kinds(&[Start, Pause, Resume, Pause, Resume, End]);
    validate_pause_resume_pairing(&t).unwrap();
}

#[test]
fn validate_pairing_accepts_nested_pairs() {
    // Nesting (Pause, Pause, Resume, Resume) is allowed; the inner
    // pair is a no-op on the counter's edge emissions.
    use CheckpointKind::{End, Pause, Resume, Start};
    let t = track_with_kinds(&[Start, Pause, Pause, Resume, Resume, End]);
    validate_pause_resume_pairing(&t).unwrap();
}

#[test]
fn validate_pairing_rejects_lone_pause() {
    use CheckpointKind::{End, Pause, Start};
    let t = track_with_kinds(&[Start, Pause, End]);
    let err = validate_pause_resume_pairing(&t).unwrap_err().to_string();
    assert!(
        err.contains("unmatched Pause"),
        "unexpected error message: {err}"
    );
}

#[test]
fn validate_pairing_rejects_lone_resume() {
    use CheckpointKind::{End, Resume, Start};
    let t = track_with_kinds(&[Start, Resume, End]);
    let err = validate_pause_resume_pairing(&t).unwrap_err().to_string();
    assert!(
        err.contains("checkpoint[1]") && err.contains("Resume"),
        "unexpected error message: {err}"
    );
}

#[test]
fn validate_pairing_rejects_resume_before_pause() {
    use CheckpointKind::{End, Pause, Resume, Start};
    let t = track_with_kinds(&[Start, Resume, Pause, End]);
    let err = validate_pause_resume_pairing(&t).unwrap_err().to_string();
    // The Resume at index 1 trips the balance-negative check before the
    // (otherwise also broken) Pause at index 2 gets a chance to drift
    // the counter.
    assert!(
        err.contains("checkpoint[1]"),
        "expected first violation at index 1, got: {err}"
    );
}

#[test]
fn validate_pairing_error_names_first_violation() {
    // Two unmatched Pauses; the message must blame the missing
    // Resume on the End checkpoint with the count, not silently
    // report only the last one.
    use CheckpointKind::{End, Pause, Start};
    let t = track_with_kinds(&[Start, Pause, Pause, End]);
    let err = validate_pause_resume_pairing(&t).unwrap_err().to_string();
    assert!(
        err.contains("2 unmatched"),
        "expected count in error, got: {err}"
    );
}

#[test]
fn unload_clears_pause_counter() {
    use crate::plugin::pause_triggers;

    pause_triggers::reset_counter();
    pause_triggers::pause_add();
    pause_triggers::pause_add();
    assert_eq!(pause_triggers::current_counter(), 2);

    let mut state = SplitsState::default();
    state.load(linear_track(), Some(TEST_MAP.to_string()));
    state.unload();

    assert_eq!(pause_triggers::current_counter(), 0);
}

// ---- editor mutation API ----

fn iv(x: i32, y: i32, z: i32) -> IVec3 {
    IVec3 { x, y, z }
}

#[test]
fn aabb_from_block_corners_single_block_is_unit_cube() {
    // Clicking the same block twice yields a 1x1x1 box: each axis spans
    // [block, block + 1).
    let b = aabb_from_block_corners(iv(10, 4, 20), iv(10, 4, 20));
    assert_eq!(b.min, v(10.0, 4.0, 20.0));
    assert_eq!(b.max, v(11.0, 5.0, 21.0));
}

#[test]
fn aabb_from_block_corners_worked_example() {
    // The plan's worked example: (10,4,20) + (12,7,22) -> min (10,4,20),
    // max (13,8,23). The +1 lands on the per-axis max.
    let b = aabb_from_block_corners(iv(10, 4, 20), iv(12, 7, 22));
    assert_eq!(b.min, v(10.0, 4.0, 20.0));
    assert_eq!(b.max, v(13.0, 8.0, 23.0));
}

#[test]
fn aabb_from_block_corners_order_independent() {
    // Per-axis min/max canonicalizes, so corner order doesn't matter.
    let forward = aabb_from_block_corners(iv(10, 4, 20), iv(12, 7, 22));
    let swapped = aabb_from_block_corners(iv(12, 7, 22), iv(10, 4, 20));
    assert_eq!(forward, swapped);
    // Mixed per-axis ordering canonicalizes too.
    let mixed = aabb_from_block_corners(iv(12, 4, 22), iv(10, 7, 20));
    assert_eq!(mixed, forward);
}

fn kinds(state: &SplitsState) -> Vec<CheckpointKind> {
    state
        .track
        .as_ref()
        .unwrap()
        .checkpoints
        .iter()
        .map(|c| c.kind)
        .collect()
}

#[test]
fn insert_checkpoint_appends_before_end_and_rederives_boundaries() {
    use CheckpointKind::{End, Split, Start};
    let mut state = SplitsState::default();
    state.load(linear_track(), Some(TEST_MAP.to_string())); // Start, Split, Split, End
    let idx = state
        .insert_checkpoint(aabb((5.0, 0.0, 0.0), (6.0, 1.0, 1.0)), "new".into(), None)
        .unwrap();
    assert_eq!(idx, 3, "append lands just before End at index n - 1");
    let t = state.track.as_ref().unwrap();
    assert_eq!(t.checkpoints.len(), 5);
    assert_eq!(kinds(&state), vec![Start, Split, Split, Split, End]);
    assert_eq!(t.checkpoints[3].label, "new");
    // Latches reallocated to the new length, run re-armed.
    assert_eq!(state.next_index, 0);
    assert_eq!(state.fired, vec![false; 5]);
    assert_eq!(state.last_inside, vec![false; 5]);
}

#[test]
fn insert_checkpoint_target_clamps_inside_boundaries() {
    use CheckpointKind::{End, Split, Start};
    let mut state = SplitsState::default();
    state.load(linear_track(), Some(TEST_MAP.to_string())); // n = 4
    // target 0 would displace Start -> clamped to 1.
    let idx = state
        .insert_checkpoint(aabb((5.0, 0.0, 0.0), (6.0, 1.0, 1.0)), "x".into(), Some(0))
        .unwrap();
    assert_eq!(idx, 1);
    assert_eq!(kinds(&state), vec![Start, Split, Split, Split, End]);

    // A large target clamps to n - 1 (just before End), never past it.
    let mut state = SplitsState::default();
    state.load(linear_track(), Some(TEST_MAP.to_string())); // n = 4
    let idx = state
        .insert_checkpoint(aabb((5.0, 0.0, 0.0), (6.0, 1.0, 1.0)), "x".into(), Some(99))
        .unwrap();
    assert_eq!(idx, 3);
    let t = state.track.as_ref().unwrap();
    assert_eq!(t.checkpoints[4].kind, CheckpointKind::End);
}

#[test]
fn insert_checkpoint_bootstraps_empty_then_single_track() {
    // First placement on an empty track is index 0 (-> Start after
    // re-derive); the second appends as index 1 (-> End).
    let mut state = SplitsState::default();
    state.load(
        Track {
            name: "empty".into(),
            checkpoints: vec![],
        },
        Some(TEST_MAP.to_string()),
    );
    let idx0 = state
        .insert_checkpoint(aabb((0.0, 0.0, 0.0), (1.0, 1.0, 1.0)), "a".into(), None)
        .unwrap();
    assert_eq!(idx0, 0);
    assert_eq!(kinds(&state), vec![CheckpointKind::Start]);

    let idx1 = state
        .insert_checkpoint(aabb((5.0, 0.0, 0.0), (6.0, 1.0, 1.0)), "b".into(), None)
        .unwrap();
    assert_eq!(idx1, 1);
    assert_eq!(
        kinds(&state),
        vec![CheckpointKind::Start, CheckpointKind::End]
    );
}

#[test]
fn insert_checkpoint_preserves_middle_pause_resume() {
    use CheckpointKind::{End, Pause, Resume, Split, Start};
    let mut state = SplitsState::default();
    state.load(
        track_with_kinds(&[Start, Pause, Resume, End]),
        Some(TEST_MAP.to_string()),
    );
    // Insert at index 2 (between Pause and Resume).
    state
        .insert_checkpoint(aabb((5.0, 0.0, 0.0), (6.0, 1.0, 1.0)), "x".into(), Some(2))
        .unwrap();
    assert_eq!(kinds(&state), vec![Start, Pause, Split, Resume, End]);
}

#[test]
fn delete_checkpoint_preserves_middle_maploaded() {
    use CheckpointKind::{End, Split, Start};
    let track = Track {
        name: "T".into(),
        checkpoints: vec![
            cp(Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
            cp(Split, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0)), // idx 1, deleted
            cp_map(Split, "mid"),                          // idx 2, MapLoaded
            cp(End, (20.0, 0.0, 0.0), (22.0, 4.0, 2.0)),
        ],
    };
    let mut state = SplitsState::default();
    state.load(track, Some(TEST_MAP.to_string()));
    state.delete_checkpoint(1).unwrap();
    let t = state.track.as_ref().unwrap();
    assert_eq!(t.checkpoints.len(), 3);
    assert_eq!(kinds(&state), vec![Start, Split, End]);
    // The surviving MapLoaded checkpoint kept its trigger and middle kind.
    assert!(matches!(t.checkpoints[1].trigger, Trigger::MapLoaded(ref n) if n == "mid"));
    // Latches reallocated, run re-armed.
    assert_eq!(state.next_index, 0);
    assert_eq!(state.fired, vec![false; 3]);
}

#[test]
fn delete_checkpoint_refuses_below_two() {
    let mut state = SplitsState::default();
    state.load(
        track_with_kinds(&[CheckpointKind::Start, CheckpointKind::End]),
        Some(TEST_MAP.to_string()),
    );
    assert!(state.delete_checkpoint(0).is_err());
    assert_eq!(state.track.as_ref().unwrap().checkpoints.len(), 2);
}

#[test]
fn delete_checkpoint_out_of_range_errors() {
    let mut state = SplitsState::default();
    state.load(linear_track(), Some(TEST_MAP.to_string()));
    assert!(state.delete_checkpoint(99).is_err());
}

#[test]
fn set_label_does_not_resize_or_rearm() {
    let mut state = SplitsState::default();
    state.load(linear_track(), Some(TEST_MAP.to_string()));
    // Pretend a run is mid-flight.
    state.next_index = 2;
    state.fired = vec![true, true, false, false];
    state.set_label(1, "renamed".into()).unwrap();
    assert_eq!(
        state.track.as_ref().unwrap().checkpoints[1].label,
        "renamed"
    );
    // Non-structural: cursor and latches untouched.
    assert_eq!(state.next_index, 2);
    assert_eq!(state.fired, vec![true, true, false, false]);
}

#[test]
fn mutation_methods_error_without_track() {
    let mut state = SplitsState::default();
    assert!(
        state
            .insert_checkpoint(aabb((0.0, 0.0, 0.0), (1.0, 1.0, 1.0)), "x".into(), None)
            .is_err()
    );
    assert!(state.delete_checkpoint(0).is_err());
    assert!(state.set_label(0, "x".into()).is_err());
}

#[test]
fn expected_kind_boundaries() {
    use CheckpointKind::{End, Split, Start};
    assert_eq!(expected_kind(0, 4), Start);
    assert_eq!(expected_kind(1, 4), Split);
    assert_eq!(expected_kind(2, 4), Split);
    assert_eq!(expected_kind(3, 4), End);
    // Single-checkpoint track: index 0 is Start (the i == 0 branch wins
    // over the last-index branch).
    assert_eq!(expected_kind(0, 1), Start);
}
