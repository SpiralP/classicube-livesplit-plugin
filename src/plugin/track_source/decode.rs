use std::{cell::RefCell, mem};

use crate::plugin::splits::geometry::{
    Aabb, Checkpoint, CheckpointKind, Track, aabb_from_min_size,
};

/// Result of feeding a single chat line to the receiver.
pub enum FrameOutcome {
    /// Line is not one of ours (no `LS ` prefix). Caller should let it
    /// render normally.
    NotOurs,
    /// Line looked like ours but didn't parse / didn't match the current
    /// state. Caller chat-prints the diagnostic and falls through so the
    /// raw line still renders.
    ParseError(String),
    /// Line is part of a track in progress. Caller suppresses it.
    Buffered,
    /// Final line of a track. Caller loads the track and suppresses.
    Loaded(Track),
}

#[derive(Debug)]
enum State {
    Idle,
    NeedStart {
        name: String,
    },
    /// At least Start is in `slots`; the most recent checkpoint already
    /// has its label populated. Next line is `cp` / `end` / `title`.
    NeedNext {
        name: String,
        slots: Vec<Checkpoint>,
    },
    /// The most recent push has an empty label. Next line must be
    /// `LS label <text>` (or `LS title <name>` to reset).
    NeedLabel {
        name: String,
        slots: Vec<Checkpoint>,
        awaiting_kind: CheckpointKind,
    },
}

impl State {
    fn label(&self) -> &'static str {
        match self {
            State::Idle => "Idle",
            State::NeedStart { .. } => "NeedStart",
            State::NeedNext { .. } => "NeedNext",
            State::NeedLabel { .. } => "NeedLabel",
        }
    }
}

thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State::Idle) };
}

#[cfg(test)]
fn reset_state() {
    STATE.with(|s| *s.borrow_mut() = State::Idle);
}

/// Feed one chat line. Updates the thread-local state machine and
/// returns the outcome the caller should react to.
pub fn feed_chat_line(text: &str) -> FrameOutcome {
    let Some(after_prefix) = text.strip_prefix("LS ") else {
        return FrameOutcome::NotOurs;
    };

    let (keyword, rest) = match after_prefix.find(' ') {
        Some(i) => (&after_prefix[..i], &after_prefix[i + 1..]),
        None => (after_prefix, ""),
    };

    let parsed = match keyword {
        "title" => parse_title(rest),
        "label" => parse_label(rest),
        "start" => parse_checkpoint(rest).map(|(aabb, label)| Line::Start { aabb, label }),
        "cp" => parse_checkpoint(rest).map(|(aabb, label)| Line::Cp { aabb, label }),
        "end" => parse_checkpoint(rest).map(|(aabb, label)| Line::End { aabb, label }),
        other => Err(format!("unknown keyword `{other}`")),
    };

    let line = match parsed {
        Ok(l) => l,
        Err(e) => return FrameOutcome::ParseError(e),
    };

    STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        transition(&mut state, line)
    })
}

enum Line {
    Title { name: String },
    Start { aabb: Aabb, label: Option<String> },
    Cp { aabb: Aabb, label: Option<String> },
    End { aabb: Aabb, label: Option<String> },
    Label { text: String },
}

fn parse_title(rest: &str) -> Result<Line, String> {
    if rest.trim().is_empty() {
        return Err("title name is empty".to_string());
    }
    Ok(Line::Title {
        name: rest.to_string(),
    })
}

fn parse_label(rest: &str) -> Result<Line, String> {
    if rest.trim().is_empty() {
        return Err("label text is empty".to_string());
    }
    Ok(Line::Label {
        text: rest.to_string(),
    })
}

fn parse_checkpoint(rest: &str) -> Result<(Aabb, Option<String>), String> {
    let mut parts = rest.splitn(3, ' ');
    let min_str = parts
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "missing min triple".to_string())?;
    let size_str = parts
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "missing size triple".to_string())?;
    let label_str = parts.next();

    let min = parse_u16_triple(min_str)?;
    let size = parse_u8_triple(size_str)?;
    let aabb = aabb_from_min_size(min, size);

    let label = match label_str {
        Some(s) => {
            if s.trim().is_empty() {
                return Err("inline label is empty".to_string());
            }
            Some(s.to_string())
        }
        None => None,
    };

    Ok((aabb, label))
}

fn parse_u16_triple(s: &str) -> Result<[u16; 3], String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 3 {
        return Err(format!(
            "expected 3 comma-separated min values, got {}",
            parts.len()
        ));
    }
    let mut out = [0u16; 3];
    for (i, p) in parts.iter().enumerate() {
        out[i] = p
            .parse::<u16>()
            .map_err(|e| format!("min[{i}] `{p}`: {e}"))?;
    }
    Ok(out)
}

fn parse_u8_triple(s: &str) -> Result<[u8; 3], String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 3 {
        return Err(format!(
            "expected 3 comma-separated size values, got {}",
            parts.len()
        ));
    }
    let mut out = [0u8; 3];
    for (i, p) in parts.iter().enumerate() {
        out[i] = p
            .parse::<u8>()
            .map_err(|e| format!("size[{i}] `{p}`: {e}"))?;
    }
    Ok(out)
}

fn transition(state: &mut State, line: Line) -> FrameOutcome {
    // `LS title` is the universal reset from any state.
    if let Line::Title { name } = line {
        *state = State::NeedStart { name };
        return FrameOutcome::Buffered;
    }

    let prev_label = state.label();
    let taken = mem::replace(state, State::Idle);

    match (taken, line) {
        (State::NeedStart { name }, Line::Start { aabb, label }) => match label {
            Some(label) => {
                *state = State::NeedNext {
                    name,
                    slots: vec![Checkpoint {
                        kind: CheckpointKind::Start,
                        aabb,
                        label,
                    }],
                };
                FrameOutcome::Buffered
            }
            None => {
                *state = State::NeedLabel {
                    name,
                    slots: vec![Checkpoint {
                        kind: CheckpointKind::Start,
                        aabb,
                        label: String::new(),
                    }],
                    awaiting_kind: CheckpointKind::Start,
                };
                FrameOutcome::Buffered
            }
        },
        (State::NeedNext { name, mut slots }, Line::Cp { aabb, label }) => match label {
            Some(label) => {
                slots.push(Checkpoint {
                    kind: CheckpointKind::Split,
                    aabb,
                    label,
                });
                *state = State::NeedNext { name, slots };
                FrameOutcome::Buffered
            }
            None => {
                slots.push(Checkpoint {
                    kind: CheckpointKind::Split,
                    aabb,
                    label: String::new(),
                });
                *state = State::NeedLabel {
                    name,
                    slots,
                    awaiting_kind: CheckpointKind::Split,
                };
                FrameOutcome::Buffered
            }
        },
        (State::NeedNext { name, mut slots }, Line::End { aabb, label }) => match label {
            Some(label) => {
                slots.push(Checkpoint {
                    kind: CheckpointKind::End,
                    aabb,
                    label,
                });
                let track = Track {
                    name,
                    checkpoints: slots,
                };
                FrameOutcome::Loaded(track)
            }
            None => {
                slots.push(Checkpoint {
                    kind: CheckpointKind::End,
                    aabb,
                    label: String::new(),
                });
                *state = State::NeedLabel {
                    name,
                    slots,
                    awaiting_kind: CheckpointKind::End,
                };
                FrameOutcome::Buffered
            }
        },
        (
            State::NeedLabel {
                name,
                mut slots,
                awaiting_kind,
            },
            Line::Label { text },
        ) => {
            let last = slots
                .last_mut()
                .expect("NeedLabel always has at least one slot");
            last.label = text;
            match awaiting_kind {
                CheckpointKind::End => {
                    let track = Track {
                        name,
                        checkpoints: slots,
                    };
                    FrameOutcome::Loaded(track)
                }
                CheckpointKind::Start | CheckpointKind::Split => {
                    *state = State::NeedNext { name, slots };
                    FrameOutcome::Buffered
                }
            }
        }

        // --- ParseError fall-throughs ---
        (taken, Line::Start { .. }) => {
            *state = taken;
            FrameOutcome::ParseError(format!("unexpected `start`, state is `{prev_label}`"))
        }
        (taken @ (State::Idle | State::NeedStart { .. }), Line::Cp { .. } | Line::End { .. }) => {
            *state = taken;
            FrameOutcome::ParseError("no `LS start` yet".to_string())
        }
        (taken @ State::NeedLabel { .. }, Line::Cp { .. } | Line::End { .. }) => {
            *state = taken;
            FrameOutcome::ParseError("previous checkpoint not yet labeled".to_string())
        }
        (taken @ (State::Idle | State::NeedStart { .. }), Line::Label { .. }) => {
            *state = taken;
            FrameOutcome::ParseError("no checkpoint to label".to_string())
        }
        (taken @ State::NeedNext { .. }, Line::Label { .. }) => {
            *state = taken;
            FrameOutcome::ParseError("checkpoint already has a label".to_string())
        }

        // Title was handled above.
        (_, Line::Title { .. }) => unreachable!("title handled above"),
    }
}

#[cfg(test)]
mod tests {
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

    fn cp(
        kind: CheckpointKind,
        min: (f32, f32, f32),
        max: (f32, f32, f32),
        label: &str,
    ) -> Checkpoint {
        Checkpoint {
            kind,
            aabb: aabb(min, max),
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
        assert_not_ours(feed_chat_line("&ehello"));
    }

    #[test]
    fn ls_without_trailing_space_is_not_ours() {
        let _g = fresh();
        assert_not_ours(feed_chat_line("LSfoo"));
    }

    // ---- ParseError ----

    #[test]
    fn unknown_keyword_errors() {
        let _g = fresh();
        let m = assert_parse_error(feed_chat_line("LS frobnicate stuff"));
        assert!(m.contains("frobnicate"), "{m}");
    }

    #[test]
    fn start_before_title_errors() {
        let _g = fresh();
        let m = assert_parse_error(feed_chat_line("LS start 0,0,0 1,1,1 label"));
        assert!(
            m.contains("unexpected `start`"),
            "expected state-mismatch, got: {m}"
        );
    }

    #[test]
    fn cp_before_start_errors() {
        let _g = fresh();
        let m = assert_parse_error(feed_chat_line("LS cp 0,0,0 1,1,1 label"));
        assert!(m.contains("no `LS start`"), "{m}");
    }

    #[test]
    fn end_before_start_errors() {
        let _g = fresh();
        let m = assert_parse_error(feed_chat_line("LS end 0,0,0 1,1,1 label"));
        assert!(m.contains("no `LS start`"), "{m}");
    }

    #[test]
    fn second_start_errors() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title T"));
        assert_buffered(feed_chat_line("LS start 0,0,0 1,1,1 start"));
        let m = assert_parse_error(feed_chat_line("LS start 0,0,0 1,1,1 second"));
        assert!(m.contains("unexpected `start`"), "{m}");
    }

    #[test]
    fn cp_in_need_label_errors() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title T"));
        assert_buffered(feed_chat_line("LS start 0,0,0 1,1,1")); // no inline label
        let m = assert_parse_error(feed_chat_line("LS cp 10,0,0 1,1,1 mid"));
        assert!(m.contains("not yet labeled"), "{m}");
    }

    #[test]
    fn end_in_need_label_errors() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title T"));
        assert_buffered(feed_chat_line("LS start 0,0,0 1,1,1"));
        let m = assert_parse_error(feed_chat_line("LS end 10,0,0 1,1,1 end"));
        assert!(m.contains("not yet labeled"), "{m}");
    }

    #[test]
    fn label_in_idle_errors() {
        let _g = fresh();
        let m = assert_parse_error(feed_chat_line("LS label some text"));
        assert!(m.contains("no checkpoint to label"), "{m}");
    }

    #[test]
    fn label_in_need_start_errors() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title T"));
        let m = assert_parse_error(feed_chat_line("LS label premature"));
        assert!(m.contains("no checkpoint to label"), "{m}");
    }

    #[test]
    fn label_after_inline_label_errors() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title T"));
        assert_buffered(feed_chat_line("LS start 0,0,0 1,1,1 inline"));
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
        let m = assert_parse_error(feed_chat_line("LS start 100,64,200 1,1,1 "));
        assert!(m.contains("inline label is empty"), "{m}");
    }

    #[test]
    fn whitespace_only_label_text_errors() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title T"));
        assert_buffered(feed_chat_line("LS start 0,0,0 1,1,1"));
        let m = assert_parse_error(feed_chat_line("LS label    "));
        assert!(m.contains("label text is empty"), "{m}");
    }

    #[test]
    fn non_numeric_coord_errors() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title T"));
        let m = assert_parse_error(feed_chat_line("LS start abc,def,ghi 1,1,1"));
        assert!(m.contains("min"), "{m}");
    }

    #[test]
    fn wrong_size_arity_errors() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title T"));
        let m = assert_parse_error(feed_chat_line("LS start 100,64,200 4,4"));
        assert!(m.contains("3 comma-separated size"), "{m}");
    }

    #[test]
    fn wrong_min_arity_errors() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title T"));
        let m = assert_parse_error(feed_chat_line("LS start 100,64,200,extra 4,4,4"));
        assert!(m.contains("3 comma-separated min"), "{m}");
    }

    #[test]
    fn missing_comma_wrong_shape_errors() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title T"));
        // splitn(3, ' ') sees ["100", "64", "200 4 4 4"] → min parse fails arity.
        let m = assert_parse_error(feed_chat_line("LS start 100 64 200 4 4 4"));
        assert!(m.contains("min") || m.contains("comma"), "{m}");
    }

    #[test]
    fn size_exceeds_u8_errors() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title T"));
        let m = assert_parse_error(feed_chat_line("LS start 100,64,200 4,4,300"));
        assert!(m.contains("size"), "{m}");
    }

    // ---- Buffered ----

    #[test]
    fn title_alone_buffers() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title my track"));
    }

    #[test]
    fn title_then_start_inline_buffers() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title T"));
        assert_buffered(feed_chat_line("LS start 0,0,0 2,4,2 start"));
    }

    #[test]
    fn title_then_start_no_label_buffers() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title T"));
        assert_buffered(feed_chat_line("LS start 0,0,0 2,4,2"));
    }

    #[test]
    fn title_then_start_then_label_buffers() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title T"));
        assert_buffered(feed_chat_line("LS start 0,0,0 2,4,2"));
        assert_buffered(feed_chat_line("LS label start"));
    }

    #[test]
    fn title_then_start_then_cp_mixed_label_forms_buffer() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title T"));
        assert_buffered(feed_chat_line("LS start 0,0,0 2,4,2 start"));
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
        // finalizing line is `LS label <text>` not `LS end <coords> <label>`.
        // Inline cp line: "LS end 30,0,0 2,4,2 <label>" = 21 + label cp.
        // Cap is 61, so label needs > 40 cp to overflow inline.
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
        assert_eq!(lines.len(), 1 + 2 + 1, "expected overflow on End");
        assert!(lines.last().unwrap().starts_with("LS label "));
        feed_all_but_last(&lines);
        let decoded = assert_loaded(feed_chat_line(lines.last().unwrap()));
        assert_eq!(decoded, track);
    }

    #[test]
    fn refeed_title_resets_buffer() {
        let _g = fresh();
        assert_buffered(feed_chat_line("LS title T1"));
        assert_buffered(feed_chat_line("LS start 0,0,0 2,4,2 start"));
        assert_buffered(feed_chat_line("LS cp 10,0,0 2,4,2 mid"));
        // Re-fed title resets.
        assert_buffered(feed_chat_line("LS title T2"));
        // Now the only valid next is start; cp must error.
        let m = assert_parse_error(feed_chat_line("LS cp 10,0,0 2,4,2 mid"));
        assert!(m.contains("no `LS start`"), "{m}");
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
        // First cp gets an inline label, second cp's label is delivered
        // via a follow-up `LS label` line. Both should round-trip into
        // an identical runtime track.
        let lines = [
            "LS title mixed",
            "LS start 0,0,0 2,4,2 start",
            "LS cp 10,0,0 2,4,2",
            "LS label split 1",
            "LS end 30,0,0 2,4,2 end",
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
}
