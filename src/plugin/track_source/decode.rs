#[cfg(test)]
mod tests;

use std::{cell::RefCell, mem};

use crate::plugin::splits::geometry::{
    Aabb, Checkpoint, CheckpointKind, Track, Trigger, aabb_from_min_size,
    validate_pause_resume_pairing,
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
    /// `LS title` accepted; next valid line is `cp` or `map` (which
    /// becomes the Start-kind checkpoint).
    NeedFirst {
        name: String,
    },
    /// At least one checkpoint is in `slots`; the most recent one
    /// already has its label populated. Next line is `cp` / `map` /
    /// `pause` / `unpause` / `end` / `title`.
    NeedNext {
        name: String,
        slots: Vec<Checkpoint>,
    },
    /// The most recent checkpoint push has an empty label. Next line
    /// must be `LS label <text>` (or `LS title <name>` to reset).
    NeedLabel {
        name: String,
        slots: Vec<Checkpoint>,
    },
}

thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State::Idle) };
}

/// Strip one leading `&X` color code (where X is ASCII alphanumeric —
/// covers stock `&0`-`&f` and CPE custom codes like `&S`). The encoder's
/// `MAX_LINE_CP` budget already anticipates a server-prepended color
/// prefix on echo (observed on ClassiCube official servers: lines come
/// through as `&7LS title …`); without this strip the receiver would
/// `NotOurs` every frame the chained `mb` form produces. MCGalaxy
/// collapses runs of color codes to a single code before broadcast, so
/// at most one `&X` prefix ever reaches us.
fn strip_leading_color_code(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'&' && bytes[1].is_ascii_alphanumeric() {
        &s[2..]
    } else {
        s
    }
}

/// Feed one chat line. Updates the thread-local state machine and
/// returns the outcome the caller should react to.
pub fn feed_chat_line(text: &str) -> FrameOutcome {
    let text = strip_leading_color_code(text);
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
        "cp" => parse_aabb_line(rest).map(|(aabb, label)| Line::Aabb {
            kind: CheckpointKind::Split,
            aabb,
            label,
        }),
        "map" => parse_map(rest),
        "pause" => parse_aabb_line(rest).map(|(aabb, label)| Line::Aabb {
            kind: CheckpointKind::Pause,
            aabb,
            label,
        }),
        "unpause" => parse_aabb_line(rest).map(|(aabb, label)| Line::Aabb {
            kind: CheckpointKind::Resume,
            aabb,
            label,
        }),
        "end" => parse_end(rest),
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
    Title {
        name: String,
    },
    /// `LS cp` (Split kind), `LS pause` (Pause kind), or `LS unpause`
    /// (Resume kind). Carries the kind the line keyword implies; the
    /// state machine downgrades to `Start` for the first checkpoint
    /// and promotes to `End` on `LS end`.
    Aabb {
        kind: CheckpointKind,
        aabb: Aabb,
        label: Option<String>,
    },
    Map {
        name: String,
        label: Option<String>,
    },
    End,
    Label {
        text: String,
    },
}

fn parse_title(rest: &str) -> Result<Line, String> {
    if rest.trim().is_empty() {
        return Err("title name is empty".to_string());
    }
    Ok(Line::Title {
        name: rest.to_string(),
    })
}

fn parse_map(rest: &str) -> Result<Line, String> {
    if rest.trim().is_empty() {
        return Err("map name is empty".to_string());
    }
    let mut parts = rest.splitn(2, ' ');
    let name = parts.next().expect("splitn yields >= 1 part").to_string();
    if name.is_empty() {
        return Err("map name is empty".to_string());
    }
    let label = match parts.next() {
        Some(s) => {
            if s.trim().is_empty() {
                return Err("inline label is empty".to_string());
            }
            Some(s.to_string())
        }
        None => None,
    };
    Ok(Line::Map { name, label })
}

fn parse_label(rest: &str) -> Result<Line, String> {
    if rest.trim().is_empty() {
        return Err("label text is empty".to_string());
    }
    Ok(Line::Label {
        text: rest.to_string(),
    })
}

fn parse_end(rest: &str) -> Result<Line, String> {
    if !rest.is_empty() {
        return Err(format!("`LS end` takes no arguments, got `{rest}`"));
    }
    Ok(Line::End)
}

/// Parse the body of an `LS cp` / `LS pause` / `LS unpause` line —
/// two coord triples plus an optional trailing label.
fn parse_aabb_line(rest: &str) -> Result<(Aabb, Option<String>), String> {
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
        *state = State::NeedFirst { name };
        return FrameOutcome::Buffered;
    }

    let taken = mem::replace(state, State::Idle);

    match (taken, line) {
        // First checkpoint is always Start, regardless of the line's
        // declared kind. Pause/Resume kinds on the first line are
        // refused (the kind reported on the line came from the keyword,
        // so a "LS pause ..." as the first checkpoint would land here
        // with kind = Pause; reject it).
        (
            State::NeedFirst { name },
            Line::Aabb {
                kind: CheckpointKind::Pause | CheckpointKind::Resume,
                ..
            },
        ) => {
            *state = State::NeedFirst { name };
            FrameOutcome::ParseError(
                "first checkpoint must be `LS cp` or `LS map` (Start kind), not `LS pause` / `LS \
                 unpause`"
                    .to_string(),
            )
        }
        (
            State::NeedFirst { name },
            Line::Aabb {
                kind: _,
                aabb,
                label,
            },
        ) => push_first(state, name, Trigger::Aabb(aabb), label),
        (State::NeedFirst { name }, Line::Map { name: map, label }) => {
            push_first(state, name, Trigger::MapLoaded(map), label)
        }
        (State::NeedNext { name, slots }, Line::Aabb { kind, aabb, label }) => {
            push_next(state, name, slots, kind, Trigger::Aabb(aabb), label)
        }
        (State::NeedNext { name, slots }, Line::Map { name: map, label }) => push_next(
            state,
            name,
            slots,
            CheckpointKind::Split,
            Trigger::MapLoaded(map),
            label,
        ),
        (State::NeedNext { name, mut slots }, Line::End) => {
            if slots.len() < 2 {
                *state = State::NeedNext { name, slots };
                return FrameOutcome::ParseError(
                    "track needs at least 2 checkpoints before `LS end`".to_string(),
                );
            }
            // End promotes the previous Split (cp / map) to End. Pause
            // / Resume kinds can't be the last checkpoint — they'd
            // otherwise lose their pause-counter side effect to the
            // End promotion. Reject so the author's track stays well-
            // formed.
            let last_kind = slots.last().expect("len >= 2 above").kind;
            if !matches!(last_kind, CheckpointKind::Split) {
                *state = State::NeedNext { name, slots };
                return FrameOutcome::ParseError(format!(
                    "last checkpoint before `LS end` must be a plain checkpoint (`LS cp` / `LS \
                     map`), not {last_kind:?}"
                ));
            }
            slots.last_mut().expect("len >= 2 above").kind = CheckpointKind::End;
            let track = Track {
                name,
                checkpoints: slots,
            };
            // Belt-and-braces: the decoder can construct a `Track` that
            // `encode_for_chat` would have rejected (e.g. operator
            // hand-writing `LS …` frames from a sign), so re-validate
            // before adopting. Drop to `Idle` on failure — the track is
            // structurally broken; a fresh `LS title …` is the
            // recovery path.
            if let Err(e) = validate_pause_resume_pairing(&track) {
                *state = State::Idle;
                return FrameOutcome::ParseError(e.to_string());
            }
            FrameOutcome::Loaded(track)
        }
        (State::NeedLabel { name, mut slots }, Line::Label { text }) => {
            let last = slots
                .last_mut()
                .expect("NeedLabel always has at least one slot");
            last.label = text;
            *state = State::NeedNext { name, slots };
            FrameOutcome::Buffered
        }

        // --- ParseError fall-throughs ---
        (
            taken @ State::Idle,
            Line::Aabb { .. } | Line::Map { .. } | Line::End | Line::Label { .. },
        ) => {
            *state = taken;
            FrameOutcome::ParseError("no `LS title` yet".to_string())
        }
        (taken @ State::NeedFirst { .. }, Line::End) => {
            *state = taken;
            FrameOutcome::ParseError("no checkpoints before `LS end`".to_string())
        }
        (taken @ State::NeedFirst { .. }, Line::Label { .. }) => {
            *state = taken;
            FrameOutcome::ParseError("no checkpoint to label".to_string())
        }
        (taken @ State::NeedLabel { .. }, Line::Aabb { .. } | Line::Map { .. } | Line::End) => {
            *state = taken;
            FrameOutcome::ParseError("previous checkpoint not yet labeled".to_string())
        }
        (taken @ State::NeedNext { .. }, Line::Label { .. }) => {
            *state = taken;
            FrameOutcome::ParseError("checkpoint already has a label".to_string())
        }

        // Title was handled above.
        (_, Line::Title { .. }) => unreachable!("title handled above"),
    }
}

/// Build the first checkpoint (Start) from a NeedFirst transition.
fn push_first(
    state: &mut State,
    name: String,
    trigger: Trigger,
    label: Option<String>,
) -> FrameOutcome {
    match label {
        Some(label) => {
            *state = State::NeedNext {
                name,
                slots: vec![Checkpoint {
                    kind: CheckpointKind::Start,
                    trigger,
                    label,
                }],
            };
        }
        None => {
            *state = State::NeedLabel {
                name,
                slots: vec![Checkpoint {
                    kind: CheckpointKind::Start,
                    trigger,
                    label: String::new(),
                }],
            };
        }
    }
    FrameOutcome::Buffered
}

/// Build a subsequent checkpoint (Split / Pause / Resume) from a
/// NeedNext transition. The `kind` argument is the kind the wire
/// keyword implied — `LS cp` / `LS map` -> Split, `LS pause` ->
/// Pause, `LS unpause` -> Resume. `LS end` later promotes the last
/// Split to End.
fn push_next(
    state: &mut State,
    name: String,
    mut slots: Vec<Checkpoint>,
    kind: CheckpointKind,
    trigger: Trigger,
    label: Option<String>,
) -> FrameOutcome {
    match label {
        Some(label) => {
            slots.push(Checkpoint {
                kind,
                trigger,
                label,
            });
            *state = State::NeedNext { name, slots };
        }
        None => {
            slots.push(Checkpoint {
                kind,
                trigger,
                label: String::new(),
            });
            *state = State::NeedLabel { name, slots };
        }
    }
    FrameOutcome::Buffered
}
