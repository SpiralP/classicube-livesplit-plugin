#[cfg(test)]
mod tests;

use std::{fmt, time::Duration};

use serde::{Serialize, Serializer};

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "command", rename_all = "camelCase")]
pub enum Command {
    Start,
    Split,
    SplitOrStart,
    #[serde(rename_all = "camelCase")]
    Reset {
        #[serde(skip_serializing_if = "Option::is_none")]
        save_attempt: Option<bool>,
    },
    Pause,
    Resume,
    UndoSplit,
    SkipSplit,
    // The desktop line protocol has no `initgametime` command — it
    // initializes game time only as a side effect of `setgametime`
    // (`CommandServer.cs:setgametime` -> `LiveSplitState.SetGameTime`,
    // which sets `LoadingTimes` so `IsGameTimeInitialized` becomes true).
    // So `to_line()` renders this as `setgametime 0`. It's only ever
    // emitted immediately after `Start`, where the game-time origin is 0,
    // and `setgametime` leaves `IsGameTimePaused` untouched — so it can't
    // unpause an in-progress map-load pause window.
    InitializeGameTime,
    SetGameTime {
        time: TimeSpan,
    },
    PauseGameTime,
    ResumeGameTime,
    SetLoadingTimes {
        time: TimeSpan,
    },
    #[serde(rename_all = "camelCase")]
    SetCurrentTimingMethod {
        timing_method: TimingMethod,
    },
    Ping,
}

/// An inbound timer-originated event we choose to act on. LiveSplit pushes
/// `{"event": "<name>"}` frames for every state change (see livesplit-core's
/// `Event` enum in `src/networking/server_protocol.rs`, which has no
/// `rename_all` so the wire strings are the bare PascalCase variant names:
/// `"SplitUndone"`, `"Reset"`, `"Started"`, ...). We only react to the two
/// *backward* events; the forward auto-events are echoes of our own
/// geometry-driven commands and acting on them would double-advance the
/// cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerEvent {
    SplitUndone,
    Reset,
}

impl TimerEvent {
    /// Parse the inbound `{"event": "<name>"}` payload string. Returns
    /// `None` for events we deliberately ignore (forward auto-events are
    /// echoes of our own commands; acting on them would double-advance).
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "SplitUndone" => Some(Self::SplitUndone),
            "Reset" => Some(Self::Reset),
            _ => None,
        }
    }
}

/// Matches livesplit-core's `TimingMethod` enum byte-for-byte on the wire
/// (variants serialize as the bare PascalCase names, `"RealTime"` /
/// `"GameTime"`), so the JSON command shape exactly mirrors
/// `Command::SetCurrentTimingMethod` in
/// `livesplit-core/src/networking/server_protocol.rs`.
#[derive(Clone, Copy, Debug, Serialize)]
pub enum TimingMethod {
    RealTime,
    GameTime,
}

impl TimingMethod {
    /// Desktop line-protocol token for `switchto <token>`; see
    /// `LiveSplit/src/LiveSplit.Core/Server/CommandServer.cs:453-466`.
    fn line_token(self) -> &'static str {
        match self {
            Self::RealTime => "realtime",
            Self::GameTime => "gametime",
        }
    }
}

impl Command {
    /// Render as a single line for the LiveSplit desktop's legacy line
    /// protocol (`CommandServer.cs:ProcessMessage`). No trailing `\n` —
    /// the caller adds the line terminator. The `Option` is retained for
    /// any future JSON-only command, but no current variant returns `None`.
    pub fn to_line(&self) -> Option<String> {
        Some(match self {
            Self::Start => "start".into(),
            Self::Split => "split".into(),
            Self::SplitOrStart => "startorsplit".into(),
            Self::Reset { .. } => "reset".into(),
            Self::Pause => "pause".into(),
            Self::Resume => "resume".into(),
            Self::UndoSplit => "undosplit".into(),
            Self::SkipSplit => "skipsplit".into(),
            Self::InitializeGameTime => "setgametime 0".into(),
            Self::SetGameTime { time } => format!("setgametime {time}"),
            Self::PauseGameTime => "pausegametime".into(),
            Self::ResumeGameTime => "unpausegametime".into(),
            Self::SetLoadingTimes { time } => format!("setloadingtimes {time}"),
            Self::SetCurrentTimingMethod { timing_method } => {
                format!("switchto {}", timing_method.line_token())
            }
            Self::Ping => "ping".into(),
        })
    }
}

/// A duration formatted as `<secs>.<9-digit-nanos>` to match
/// livesplit-core's wire format (see `serialize_time_span` in
/// livesplit-core/src/networking/server_protocol.rs:103-109). The
/// desktop's `TimeSpanParser.Parse` truncates fractions past 7 digits
/// and accepts the same shape, so this `Display` impl serves both the
/// JSON (LSO) and line-protocol (desktop) encoders.
#[derive(Clone, Copy, Debug)]
pub struct TimeSpan(pub Duration);

impl From<Duration> for TimeSpan {
    fn from(d: Duration) -> Self {
        Self(d)
    }
}

impl fmt::Display for TimeSpan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secs = self.0.as_secs();
        let nanos = self.0.subsec_nanos();
        write!(f, "{secs}.{nanos:09}")
    }
}

impl Serialize for TimeSpan {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}
