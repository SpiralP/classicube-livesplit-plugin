use super::*;

fn parse(s: &str) -> Result<ServerResponse, serde_json::Error> {
    serde_json::from_str(s)
}

#[test]
fn parses_success_null() {
    assert!(matches!(
        parse(r#"{"success":null}"#).unwrap(),
        ServerResponse::Success { .. }
    ));
}

#[test]
fn parses_success_with_string_payload() {
    assert!(matches!(
        parse(r#"{"success":"01:23.456"}"#).unwrap(),
        ServerResponse::Success { .. }
    ));
}

#[test]
fn parses_error_with_code_only() {
    match parse(r#"{"error":{"code":"Busy"}}"#).unwrap() {
        ServerResponse::Error {
            error: ServerError { code, message },
        } => {
            assert_eq!(code, "Busy");
            assert!(message.is_none());
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn parses_error_with_code_and_message() {
    match parse(r#"{"error":{"code":"InvalidCommand","message":"expected value"}}"#).unwrap() {
        ServerResponse::Error {
            error: ServerError { code, message },
        } => {
            assert_eq!(code, "InvalidCommand");
            assert_eq!(message.as_deref(), Some("expected value"));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn parses_future_error_code_as_string() {
    match parse(r#"{"error":{"code":"SomeFutureCode"}}"#).unwrap() {
        ServerResponse::Error {
            error: ServerError { code, .. },
        } => assert_eq!(code, "SomeFutureCode"),
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn parses_event_known_name() {
    match parse(r#"{"event":"Splitted"}"#).unwrap() {
        ServerResponse::Event { event } => assert_eq!(event, "Splitted"),
        other => panic!("expected Event, got {other:?}"),
    }
}

#[test]
fn parses_event_future_name() {
    match parse(r#"{"event":"SomeFutureEvent"}"#).unwrap() {
        ServerResponse::Event { event } => assert_eq!(event, "SomeFutureEvent"),
        other => panic!("expected Event, got {other:?}"),
    }
}

#[test]
fn rejects_unknown_shape() {
    assert!(parse(r#"{"weird":1}"#).is_err());
}
