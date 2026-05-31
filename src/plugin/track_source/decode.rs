#[cfg(test)]
mod tests;

use std::{cell::RefCell, mem};

use crate::plugin::splits::geometry::{
    Aabb, Checkpoint, CheckpointKind, Track, Trigger, aabb_from_min_size,
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
    /// has its label populated. Next line is `cp` / `endcp` / `map` /
    /// `endmap` / `title`.
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
        "start" => parse_checkpoint(rest).map(|(aabb, label)| Line::Start { aabb, label }),
        "cp" => parse_checkpoint(rest).map(|(aabb, label)| Line::Cp { aabb, label }),
        "endcp" => parse_checkpoint(rest).map(|(aabb, label)| Line::End { aabb, label }),
        "map" => parse_consume_to_eol(rest, "map name").map(|name| Line::Map { name }),
        "endmap" => parse_consume_to_eol(rest, "endmap name").map(|name| Line::EndMap { name }),
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
    Map { name: String },
    EndMap { name: String },
    Label { text: String },
}

fn parse_title(rest: &str) -> Result<Line, String> {
    parse_consume_to_eol(rest, "title name").map(|name| Line::Title { name })
}

fn parse_consume_to_eol(rest: &str, what: &str) -> Result<String, String> {
    if rest.trim().is_empty() {
        return Err(format!("{what} is empty"));
    }
    Ok(rest.to_string())
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
                        trigger: Trigger::Aabb(aabb),
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
                        trigger: Trigger::Aabb(aabb),
                        label: String::new(),
                    }],
                    awaiting_kind: CheckpointKind::Start,
                };
                FrameOutcome::Buffered
            }
        },
        (State::NeedStart { name }, Line::Map { name: map }) => {
            *state = State::NeedLabel {
                name,
                slots: vec![Checkpoint {
                    kind: CheckpointKind::Start,
                    trigger: Trigger::MapLoaded(map),
                    label: String::new(),
                }],
                awaiting_kind: CheckpointKind::Start,
            };
            FrameOutcome::Buffered
        }
        (State::NeedNext { name, mut slots }, Line::Cp { aabb, label }) => match label {
            Some(label) => {
                slots.push(Checkpoint {
                    kind: CheckpointKind::Split,
                    trigger: Trigger::Aabb(aabb),
                    label,
                });
                *state = State::NeedNext { name, slots };
                FrameOutcome::Buffered
            }
            None => {
                slots.push(Checkpoint {
                    kind: CheckpointKind::Split,
                    trigger: Trigger::Aabb(aabb),
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
        (State::NeedNext { name, mut slots }, Line::Map { name: map }) => {
            slots.push(Checkpoint {
                kind: CheckpointKind::Split,
                trigger: Trigger::MapLoaded(map),
                label: String::new(),
            });
            *state = State::NeedLabel {
                name,
                slots,
                awaiting_kind: CheckpointKind::Split,
            };
            FrameOutcome::Buffered
        }
        (State::NeedNext { name, mut slots }, Line::End { aabb, label }) => match label {
            Some(label) => {
                slots.push(Checkpoint {
                    kind: CheckpointKind::End,
                    trigger: Trigger::Aabb(aabb),
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
                    trigger: Trigger::Aabb(aabb),
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
        (State::NeedNext { name, mut slots }, Line::EndMap { name: map }) => {
            slots.push(Checkpoint {
                kind: CheckpointKind::End,
                trigger: Trigger::MapLoaded(map),
                label: String::new(),
            });
            *state = State::NeedLabel {
                name,
                slots,
                awaiting_kind: CheckpointKind::End,
            };
            FrameOutcome::Buffered
        }
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
        (taken @ State::Idle, Line::Map { .. } | Line::EndMap { .. }) => {
            *state = taken;
            FrameOutcome::ParseError("no `LS title` yet".to_string())
        }
        (taken @ State::NeedStart { .. }, Line::EndMap { .. }) => {
            *state = taken;
            FrameOutcome::ParseError("no checkpoints before `endmap`".to_string())
        }
        (taken @ State::NeedLabel { .. }, Line::Map { .. } | Line::EndMap { .. }) => {
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
