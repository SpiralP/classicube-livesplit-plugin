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
    // No desktop equivalent — the desktop initializes game time on first
    // `setgametime`, so `to_line()` returns `None` for this variant.
    InitializeGameTime,
    SetGameTime {
        time: TimeSpan,
    },
    PauseGameTime,
    ResumeGameTime,
    SetLoadingTimes {
        time: TimeSpan,
    },
    Ping,
}

impl Command {
    /// Render as a single line for the LiveSplit desktop's legacy line
    /// protocol (`CommandServer.cs:ProcessMessage`). No trailing `\n` —
    /// the caller adds the line terminator. Returns `None` for commands
    /// with no desktop equivalent (currently only `InitializeGameTime`).
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
            Self::InitializeGameTime => return None,
            Self::SetGameTime { time } => format!("setgametime {time}"),
            Self::PauseGameTime => "pausegametime".into(),
            Self::ResumeGameTime => "unpausegametime".into(),
            Self::SetLoadingTimes { time } => format!("setloadingtimes {time}"),
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
