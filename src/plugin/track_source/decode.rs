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
    /// `LS title` accepted; next valid line is `cp` or `map` (which
    /// becomes the Start-kind checkpoint). `scope` is always `None`
    /// on entry — no `LS map` directive has been seen yet — so the
    /// first AABB pushes with `map: None`.
    NeedFirst {
        name: String,
        scope: Option<String>,
    },
    /// At least one checkpoint is in `slots`; the most recent one
    /// already has its label populated. Next line is `cp` / `map` /
    /// `end` / `title`. `scope` is the running AABB scope: `None`
    /// before any `LS map`, else `Some(name)` of the most recent map.
    NeedNext {
        name: String,
        slots: Vec<Checkpoint>,
        scope: Option<String>,
    },
    /// The most recent push has an empty label. Next line must be
    /// `LS label <text>` (or `LS title <name>` to reset). `scope`
    /// threads through unchanged until the label arrives.
    NeedLabel {
        name: String,
        slots: Vec<Checkpoint>,
        scope: Option<String>,
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
        "cp" => parse_checkpoint(rest).map(|(aabb, label)| Line::Cp { aabb, label }),
        "map" => parse_map(rest),
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
    Title { name: String },
    Cp { aabb: Aabb, label: Option<String> },
    Map { name: String, label: Option<String> },
    End,
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
        *state = State::NeedFirst { name, scope: None };
        return FrameOutcome::Buffered;
    }

    let taken = mem::replace(state, State::Idle);

    match (taken, line) {
        (State::NeedFirst { name, scope }, Line::Cp { aabb, label }) => {
            let trigger = Trigger::Aabb {
                aabb,
                map: scope.clone(),
            };
            match label {
                Some(label) => {
                    *state = State::NeedNext {
                        name,
                        slots: vec![Checkpoint {
                            kind: CheckpointKind::Start,
                            trigger,
                            label,
                        }],
                        scope,
                    };
                    FrameOutcome::Buffered
                }
                None => {
                    *state = State::NeedLabel {
                        name,
                        slots: vec![Checkpoint {
                            kind: CheckpointKind::Start,
                            trigger,
                            label: String::new(),
                        }],
                        scope,
                    };
                    FrameOutcome::Buffered
                }
            }
        }
        (State::NeedFirst { name, scope: _ }, Line::Map { name: map, label }) => {
            let trigger = Trigger::MapLoaded(map.clone());
            // The Start MapLoaded itself also opens the next section:
            // subsequent `LS cp` lines are scoped to `map`.
            let new_scope = Some(map);
            match label {
                Some(label) => {
                    *state = State::NeedNext {
                        name,
                        slots: vec![Checkpoint {
                            kind: CheckpointKind::Start,
                            trigger,
                            label,
                        }],
                        scope: new_scope,
                    };
                    FrameOutcome::Buffered
                }
                None => {
                    *state = State::NeedLabel {
                        name,
                        slots: vec![Checkpoint {
                            kind: CheckpointKind::Start,
                            trigger,
                            label: String::new(),
                        }],
                        scope: new_scope,
                    };
                    FrameOutcome::Buffered
                }
            }
        }
        (
            State::NeedNext {
                name,
                mut slots,
                scope,
            },
            Line::Cp { aabb, label },
        ) => {
            let trigger = Trigger::Aabb {
                aabb,
                map: scope.clone(),
            };
            match label {
                Some(label) => {
                    slots.push(Checkpoint {
                        kind: CheckpointKind::Split,
                        trigger,
                        label,
                    });
                    *state = State::NeedNext { name, slots, scope };
                    FrameOutcome::Buffered
                }
                None => {
                    slots.push(Checkpoint {
                        kind: CheckpointKind::Split,
                        trigger,
                        label: String::new(),
                    });
                    *state = State::NeedLabel { name, slots, scope };
                    FrameOutcome::Buffered
                }
            }
        }
        (
            State::NeedNext {
                name,
                mut slots,
                scope: _,
            },
            Line::Map { name: map, label },
        ) => {
            let trigger = Trigger::MapLoaded(map.clone());
            let new_scope = Some(map);
            match label {
                Some(label) => {
                    slots.push(Checkpoint {
                        kind: CheckpointKind::Split,
                        trigger,
                        label,
                    });
                    *state = State::NeedNext {
                        name,
                        slots,
                        scope: new_scope,
                    };
                    FrameOutcome::Buffered
                }
                None => {
                    slots.push(Checkpoint {
                        kind: CheckpointKind::Split,
                        trigger,
                        label: String::new(),
                    });
                    *state = State::NeedLabel {
                        name,
                        slots,
                        scope: new_scope,
                    };
                    FrameOutcome::Buffered
                }
            }
        }
        (
            State::NeedNext {
                name,
                mut slots,
                scope,
            },
            Line::End,
        ) => {
            if slots.len() < 2 {
                *state = State::NeedNext { name, slots, scope };
                return FrameOutcome::ParseError(
                    "track needs at least 2 checkpoints before `LS end`".to_string(),
                );
            }
            slots.last_mut().expect("len >= 2 above").kind = CheckpointKind::End;
            FrameOutcome::Loaded(Track {
                name,
                checkpoints: slots,
            })
        }
        (
            State::NeedLabel {
                name,
                mut slots,
                scope,
            },
            Line::Label { text },
        ) => {
            let last = slots
                .last_mut()
                .expect("NeedLabel always has at least one slot");
            last.label = text;
            *state = State::NeedNext { name, slots, scope };
            FrameOutcome::Buffered
        }

        // --- ParseError fall-throughs ---
        (
            taken @ State::Idle,
            Line::Cp { .. } | Line::Map { .. } | Line::End | Line::Label { .. },
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
        (taken @ State::NeedLabel { .. }, Line::Cp { .. } | Line::Map { .. } | Line::End) => {
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
