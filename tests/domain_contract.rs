#![allow(clippy::disallowed_methods)]

use std::str::FromStr;

use agent_terminal::domain::{BoundedScreen, JobName, JobState, Key, bound_screen};
use agent_terminal::{
    error::Error,
    output::{CommandData, Issued, PressData, Response, SendData, StartData},
};
use serde_json::json;

#[test]
fn job_name_accepts_only_stable_agent_handles() -> Result<(), Box<dyn std::error::Error>> {
    for valid in ["a", "dev-server", "tests.watch", "api_2"] {
        assert_eq!(JobName::from_str(valid)?.as_str(), valid);
    }

    for invalid in ["", "UPPER", "-leading", "space name", "slash/name"] {
        assert!(JobName::from_str(invalid).is_err(), "accepted {invalid:?}");
    }

    assert!(JobName::from_str(&"a".repeat(65)).is_err());
    Ok(())
}

#[test]
fn key_parser_normalizes_the_public_grammar() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        ("Enter", "Enter"),
        ("PageDown", "PageDown"),
        ("F12", "F12"),
        ("Ctrl+D", "Ctrl d"),
        ("Alt+x", "Alt x"),
    ];

    for (input, expected) in cases {
        assert_eq!(Key::from_str(input)?.zellij_token(), expected);
    }

    for invalid in ["enter", "F13", "Ctrl++", "Ctrl+é", "Shift+Enter"] {
        assert!(Key::from_str(invalid).is_err(), "accepted {invalid:?}");
    }
    Ok(())
}

#[test]
fn bounded_screen_keeps_utf8_tail_by_lines_and_bytes() {
    let source = "old\n중간\nnew-1\nnew-2\n";
    assert_eq!(
        bound_screen(source, 2, 32),
        BoundedScreen {
            screen: "new-1\nnew-2\n".to_owned(),
            truncated: true,
        }
    );

    let byte_limited = bound_screen("alpha\nβeta\ngamma", 20, 9);
    assert_eq!(byte_limited.screen, "eta\ngamma");
    assert!(byte_limited.truncated);
}

#[test]
fn start_response_matches_the_frozen_json_contract() -> Result<(), Box<dyn std::error::Error>> {
    let response = Response::ok(CommandData::Start(StartData {
        job: JobName::from_str("tests")?,
        state: JobState::Exited,
        exit_code: Some(7),
    }));

    assert_eq!(
        serde_json::to_value(response)?,
        json!({
            "status": "ok",
            "data": {"job": "tests", "state": "exited", "exit_code": 7}
        })
    );
    Ok(())
}

#[test]
fn input_responses_use_public_string_discriminators() -> Result<(), Box<dyn std::error::Error>> {
    let job = JobName::from_str("repl")?;
    let send = serde_json::to_value(Response::ok(CommandData::Send(SendData {
        job: job.clone(),
        issued: Issued::Text,
        submitted: true,
    })))?;
    let press = serde_json::to_value(Response::ok(CommandData::Press(PressData {
        job,
        issued: Issued::Keys,
        keys: vec!["Ctrl+D".to_owned(), "Alt+!".to_owned()],
    })))?;

    assert_eq!(
        send,
        json!({"status":"ok","data":{"job":"repl","issued":"text","submitted":true}})
    );
    assert_eq!(
        press,
        json!({"status":"ok","data":{"job":"repl","issued":"keys","keys":["Ctrl+D","Alt+!"]}})
    );
    Ok(())
}

#[test]
fn key_grammar_preserves_public_names() -> Result<(), Box<dyn std::error::Error>> {
    let control = Key::from_str("Ctrl+d")?;
    let alt = Key::from_str("Alt+!")?;

    assert_eq!(control.public_name(), "Ctrl+D");
    assert_eq!(control.zellij_token(), "Ctrl d");
    assert_eq!(alt.public_name(), "Alt+!");
    assert_eq!(alt.zellij_token(), "Alt !");
    assert!(Key::from_str("Ctrl+7").is_err());
    Ok(())
}

#[test]
fn bounded_screen_keeps_exactly_two_hundred_logical_lines() {
    for trailing_newline in [false, true] {
        let mut source = (1..=201)
            .map(|number| format!("line-{number}"))
            .collect::<Vec<_>>()
            .join("\n");
        if trailing_newline {
            source.push('\n');
        }
        let bounded = bound_screen(&source, 200, usize::MAX);
        assert!(bounded.truncated);
        assert!(!bounded.screen.contains("line-1\n"));
        assert!(bounded.screen.starts_with("line-2\n"));
        assert_eq!(bounded.screen.lines().count(), 200);
    }
}

#[test]
fn serialization_failures_use_a_documented_error_code() -> Result<(), Box<dyn std::error::Error>> {
    let source = serde_json::from_str::<serde_json::Value>("{")
        .err()
        .ok_or("expected malformed JSON")?;
    let error = Error::StateSerialize { source };
    assert_eq!(error.kind(), "state_io");
    Ok::<(), Box<dyn std::error::Error>>(())
}
