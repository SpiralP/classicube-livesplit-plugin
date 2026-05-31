use super::*;

fn json(c: &Command) -> String {
    serde_json::to_string(c).unwrap()
}

#[test]
fn unit_variants_serialize_camel_case() {
    assert_eq!(json(&Command::Start), r#"{"command":"start"}"#);
    assert_eq!(json(&Command::Split), r#"{"command":"split"}"#);
    assert_eq!(
        json(&Command::SplitOrStart),
        r#"{"command":"splitOrStart"}"#
    );
    assert_eq!(json(&Command::Ping), r#"{"command":"ping"}"#);
    assert_eq!(
        json(&Command::InitializeGameTime),
        r#"{"command":"initializeGameTime"}"#
    );
}

#[test]
fn reset_without_save_attempt_omits_field() {
    assert_eq!(
        json(&Command::Reset { save_attempt: None }),
        r#"{"command":"reset"}"#
    );
}

#[test]
fn reset_with_save_attempt_uses_camel_case_key() {
    assert_eq!(
        json(&Command::Reset {
            save_attempt: Some(true)
        }),
        r#"{"command":"reset","saveAttempt":true}"#
    );
}

#[test]
fn set_game_time_serializes_secs_and_padded_nanos() {
    let cmd = Command::SetGameTime {
        time: TimeSpan(Duration::new(83, 25)),
    };
    assert_eq!(
        json(&cmd),
        r#"{"command":"setGameTime","time":"83.000000025"}"#
    );
}

#[test]
fn set_current_timing_method_serializes_camel_case() {
    assert_eq!(
        json(&Command::SetCurrentTimingMethod {
            timing_method: TimingMethod::GameTime,
        }),
        r#"{"command":"setCurrentTimingMethod","timingMethod":"GameTime"}"#
    );
    assert_eq!(
        json(&Command::SetCurrentTimingMethod {
            timing_method: TimingMethod::RealTime,
        }),
        r#"{"command":"setCurrentTimingMethod","timingMethod":"RealTime"}"#
    );
}

#[test]
fn set_loading_times_zero_pads_to_nine_digits() {
    let cmd = Command::SetLoadingTimes {
        time: TimeSpan(Duration::from_millis(1500)),
    };
    assert_eq!(
        json(&cmd),
        r#"{"command":"setLoadingTimes","time":"1.500000000"}"#
    );
}

#[test]
fn line_unit_variants() {
    assert_eq!(Command::Start.to_line().as_deref(), Some("start"));
    assert_eq!(Command::Split.to_line().as_deref(), Some("split"));
    assert_eq!(
        Command::SplitOrStart.to_line().as_deref(),
        Some("startorsplit")
    );
    assert_eq!(Command::Pause.to_line().as_deref(), Some("pause"));
    assert_eq!(Command::Resume.to_line().as_deref(), Some("resume"));
    assert_eq!(Command::UndoSplit.to_line().as_deref(), Some("undosplit"));
    assert_eq!(Command::SkipSplit.to_line().as_deref(), Some("skipsplit"));
    assert_eq!(
        Command::PauseGameTime.to_line().as_deref(),
        Some("pausegametime")
    );
    assert_eq!(
        Command::ResumeGameTime.to_line().as_deref(),
        Some("unpausegametime")
    );
    assert_eq!(Command::Ping.to_line().as_deref(), Some("ping"));
}

#[test]
fn line_reset_ignores_save_attempt() {
    assert_eq!(
        Command::Reset { save_attempt: None }.to_line().as_deref(),
        Some("reset")
    );
    assert_eq!(
        Command::Reset {
            save_attempt: Some(true)
        }
        .to_line()
        .as_deref(),
        Some("reset")
    );
}

#[test]
fn line_initialize_game_time_has_no_desktop_equivalent() {
    assert!(Command::InitializeGameTime.to_line().is_none());
}

#[test]
fn line_set_game_time_serializes_secs_and_padded_nanos() {
    let cmd = Command::SetGameTime {
        time: TimeSpan(Duration::new(83, 25)),
    };
    assert_eq!(cmd.to_line().as_deref(), Some("setgametime 83.000000025"));
}

#[test]
fn line_set_current_timing_method_uses_switchto() {
    assert_eq!(
        Command::SetCurrentTimingMethod {
            timing_method: TimingMethod::GameTime,
        }
        .to_line()
        .as_deref(),
        Some("switchto gametime")
    );
    assert_eq!(
        Command::SetCurrentTimingMethod {
            timing_method: TimingMethod::RealTime,
        }
        .to_line()
        .as_deref(),
        Some("switchto realtime")
    );
}

#[test]
fn line_set_loading_times_zero_pads_to_nine_digits() {
    let cmd = Command::SetLoadingTimes {
        time: TimeSpan(Duration::from_millis(1500)),
    };
    assert_eq!(
        cmd.to_line().as_deref(),
        Some("setloadingtimes 1.500000000")
    );
}

fn ts(d: Duration) -> String {
    TimeSpan(d).to_string()
}

#[test]
fn timespan_zero() {
    assert_eq!(ts(Duration::ZERO), "0.000000000");
}

#[test]
fn timespan_whole_second_pads_fractional() {
    assert_eq!(ts(Duration::from_secs(1)), "1.000000000");
    assert_eq!(ts(Duration::from_secs(42)), "42.000000000");
}

#[test]
fn timespan_sub_second() {
    assert_eq!(ts(Duration::from_millis(500)), "0.500000000");
    assert_eq!(ts(Duration::from_micros(1)), "0.000001000");
}

#[test]
fn timespan_single_nano_padded_to_nine_digits() {
    assert_eq!(ts(Duration::new(0, 1)), "0.000000001");
}

#[test]
fn timespan_max_subsec_nanos() {
    assert_eq!(ts(Duration::new(7, 999_999_999)), "7.999999999");
}

#[test]
fn timespan_large_secs_value() {
    assert_eq!(
        ts(Duration::from_secs(u64::MAX)),
        "18446744073709551615.000000000"
    );
}

#[test]
fn timespan_from_duration_preserves_inner() {
    let d = Duration::new(123, 456);
    let TimeSpan(inner) = TimeSpan::from(d);
    assert_eq!(inner, d);
}

#[test]
fn timespan_serde_path_matches_display() {
    // The serde serializer hook (`TimeSpan::serialize`) should produce
    // the same string as `Display`. Verified transitively via the
    // `Command::SetGameTime` JSON test above, but pin it directly here
    // so a future refactor that splits the two encoders trips this
    // test rather than the higher-level one.
    let span = TimeSpan(Duration::new(60, 250_000));
    let display = span.to_string();
    let cmd = Command::SetGameTime { time: span };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(
        json.contains(&format!(r#""time":"{display}""#)),
        "JSON {json:?} did not contain Display output {display:?}"
    );
}
