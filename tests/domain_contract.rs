#![allow(clippy::disallowed_methods)]

// allow: SIZE_OK — deterministic domain contracts stay centralized in one integration target.

use std::str::FromStr;

use agent_terminal::domain::{
    BoundedScreen, JobName, JobState, Key, SessionName, TerminalPaneId, bound_screen,
};
use agent_terminal::{
    error::Error,
    output::{
        CommandData, Issued, JobSummary, ListData, PressData, ReadData, Response, SendData,
        StartData, StopData,
    },
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

#[test]
fn job_name_accepts_every_legal_initial_class() -> Result<(), Box<dyn std::error::Error>> {
    for initial in (b'a'..=b'z').chain(b'0'..=b'9') {
        let value = char::from(initial).to_string();
        assert_eq!(JobName::from_str(&value)?.as_str(), value);
    }
    Ok(())
}

#[test]
fn job_name_accepts_separators_after_the_first_character() -> Result<(), Box<dyn std::error::Error>>
{
    for valid in ["a-b", "a_b", "a.b", "a-_.9"] {
        assert_eq!(JobName::from_str(valid)?.as_str(), valid);
    }
    Ok(())
}

#[test]
fn job_name_rejects_whitespace_controls_and_non_ascii() {
    for invalid in [
        "space name",
        "tab\tname",
        "line\nname",
        "return\rname",
        "vertical\u{000b}tab",
        "form\u{000c}feed",
        "null\0byte",
        "delete\u{007f}",
        "café",
        "a\u{2003}b",
    ] {
        assert!(JobName::from_str(invalid).is_err(), "accepted {invalid:?}");
    }
}

#[test]
fn job_name_serializes_as_a_transparent_string() -> Result<(), Box<dyn std::error::Error>> {
    let job = JobName::from_str("dev-server")?;
    assert_eq!(serde_json::to_value(job)?, json!("dev-server"));
    Ok(())
}

#[test]
fn job_name_display_and_order_are_lexical() -> Result<(), Box<dyn std::error::Error>> {
    let alpha = JobName::from_str("alpha")?;
    let beta = JobName::from_str("beta")?;

    assert_eq!(alpha.to_string(), "alpha");
    assert!(alpha < beta);
    Ok(())
}

#[test]
fn session_name_accepts_the_complete_public_alphabet() -> Result<(), Box<dyn std::error::Error>> {
    let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    assert_eq!(SessionName::new(alphabet.to_owned())?.as_str(), alphabet);
    Ok(())
}

#[test]
fn session_name_rejects_empty_punctuation_and_unicode() {
    for invalid in [
        "",
        ".",
        "session.name",
        "session/name",
        "session name",
        "세션",
    ] {
        assert!(
            SessionName::new(invalid.to_owned()).is_err(),
            "accepted {invalid:?}"
        );
    }
}

#[test]
fn session_name_serializes_transparently() -> Result<(), Box<dyn std::error::Error>> {
    let session = SessionName::new("Agent-Terminal_42".to_owned())?;
    assert_eq!(serde_json::to_value(session)?, json!("Agent-Terminal_42"));
    Ok(())
}

#[test]
fn terminal_pane_id_round_trips_zero_and_u32_max() -> Result<(), Box<dyn std::error::Error>> {
    for value in [0, u32::MAX] {
        let pane_id = TerminalPaneId::new(value);
        let rendered = pane_id.to_string();
        assert_eq!(rendered, format!("terminal_{value}"));
        assert_eq!(TerminalPaneId::from_str(&rendered)?.get(), value);
    }
    Ok(())
}

#[test]
fn terminal_pane_id_rejects_malformed_and_overflowing_values() {
    for invalid in [
        "",
        "42",
        "terminal_",
        "terminal_-1",
        "terminal_1x",
        "terminal_4294967296",
    ] {
        assert!(
            TerminalPaneId::from_str(invalid).is_err(),
            "accepted {invalid:?}"
        );
    }
}

#[test]
fn terminal_pane_id_serializes_as_a_number() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(serde_json::to_value(TerminalPaneId::new(42))?, json!(42));
    Ok(())
}

#[test]
fn key_parser_accepts_every_named_key() -> Result<(), Box<dyn std::error::Error>> {
    for name in [
        "Enter",
        "Tab",
        "Esc",
        "Backspace",
        "Delete",
        "Insert",
        "Home",
        "End",
        "PageUp",
        "PageDown",
        "Up",
        "Down",
        "Left",
        "Right",
    ] {
        let key = Key::from_str(name)?;
        assert_eq!(key.public_name(), name);
        assert_eq!(key.zellij_token(), name);
    }
    Ok(())
}

#[test]
fn key_parser_accepts_exactly_f1_through_f12() -> Result<(), Box<dyn std::error::Error>> {
    for number in 1..=12 {
        let name = format!("F{number}");
        let key = Key::from_str(&name)?;
        assert_eq!(key.public_name(), name);
        assert_eq!(key.zellij_token(), name);
    }
    Ok(())
}

#[test]
fn key_parser_rejects_noncanonical_function_keys() {
    for invalid in ["F0", "F13", "f1", "F01", "F+1", "F 1", "F1 "] {
        assert!(Key::from_str(invalid).is_err(), "accepted {invalid:?}");
    }
}

#[test]
fn control_keys_normalize_ascii_letter_case() -> Result<(), Box<dyn std::error::Error>> {
    for letter in b'a'..=b'z' {
        let lower = char::from(letter);
        let upper = lower.to_ascii_uppercase();
        let public_name = format!("Ctrl+{upper}");
        let zellij_token = format!("Ctrl {lower}");

        for input_letter in [lower, upper] {
            let key = Key::from_str(&format!("Ctrl+{input_letter}"))?;
            assert_eq!(key.public_name(), public_name);
            assert_eq!(key.zellij_token(), zellij_token);
        }
    }
    Ok(())
}

#[test]
fn control_keys_reject_non_letters_and_multiple_characters() {
    for invalid in [
        "Ctrl+",
        "Ctrl+0",
        "Ctrl++",
        "Ctrl+!",
        "Ctrl+é",
        "Ctrl+ab",
        "Ctrl+Enter",
    ] {
        assert!(Key::from_str(invalid).is_err(), "accepted {invalid:?}");
    }
}

#[test]
fn alt_keys_preserve_printable_ascii() -> Result<(), Box<dyn std::error::Error>> {
    for byte in 0x20_u8..=0x7e {
        let character = char::from(byte);
        let public_name = format!("Alt+{character}");
        let zellij_token = format!("Alt {character}");
        let key = Key::from_str(&public_name)?;

        assert_eq!(key.public_name(), public_name);
        assert_eq!(key.zellij_token(), zellij_token);
    }
    Ok(())
}

#[test]
fn alt_keys_reject_controls_unicode_and_noncanonical_named_keys() {
    for byte in (0_u8..=0x1f).chain(std::iter::once(0x7f)) {
        let invalid = format!("Alt+{}", char::from(byte));
        assert!(Key::from_str(&invalid).is_err(), "accepted {invalid:?}");
    }

    for invalid in ["Alt+é", "Alt+Enter", "Alt+Tab", "Alt+xx", "alt+x", "ALT+x"] {
        assert!(Key::from_str(invalid).is_err(), "accepted {invalid:?}");
    }
}

#[test]
fn bounded_screen_empty_input_is_not_truncated() {
    assert_eq!(
        bound_screen("", 20, 20),
        BoundedScreen {
            screen: String::new(),
            truncated: false,
        }
    );
}

#[test]
fn bounded_screen_zero_lines_returns_empty() {
    assert_eq!(
        bound_screen("one\ntwo\n", 0, usize::MAX),
        BoundedScreen {
            screen: String::new(),
            truncated: true,
        }
    );
}

#[test]
fn bounded_screen_zero_bytes_returns_empty() {
    assert_eq!(
        bound_screen("visible", usize::MAX, 0),
        BoundedScreen {
            screen: String::new(),
            truncated: true,
        }
    );
}

#[test]
fn bounded_screen_exact_limits_do_not_truncate() {
    let source = "one\ntwo";
    assert_eq!(
        bound_screen(source, 2, source.len()),
        BoundedScreen {
            screen: source.to_owned(),
            truncated: false,
        }
    );
}

#[test]
fn bounded_screen_counts_trailing_newline_without_phantom_line() {
    let source = "one\ntwo\n";
    assert_eq!(
        bound_screen(source, 2, usize::MAX),
        BoundedScreen {
            screen: source.to_owned(),
            truncated: false,
        }
    );
}

#[test]
fn bounded_screen_advances_to_a_utf8_boundary() {
    assert_eq!(
        bound_screen("éx", usize::MAX, 2),
        BoundedScreen {
            screen: "x".to_owned(),
            truncated: true,
        }
    );
}

#[test]
fn bounded_screen_applies_line_and_byte_limits_independently() {
    let source = "alpha\nbeta\ngamma";
    assert_eq!(bound_screen(source, 2, usize::MAX).screen, "beta\ngamma");
    assert_eq!(bound_screen(source, usize::MAX, 5).screen, "gamma");
}

#[test]
fn start_response_omits_absent_exit_code() -> Result<(), Box<dyn std::error::Error>> {
    let response = Response::ok(CommandData::Start(StartData {
        job: JobName::from_str("server")?,
        state: JobState::Running,
        exit_code: None,
    }));

    assert_eq!(
        serde_json::to_value(response)?,
        json!({"status":"ok","data":{"job":"server","state":"running"}})
    );
    Ok(())
}

#[test]
fn lost_read_response_omits_screen_optionals() -> Result<(), Box<dyn std::error::Error>> {
    let response = Response::ok(CommandData::Read(ReadData {
        job: JobName::from_str("server")?,
        state: JobState::Lost,
        exit_code: None,
        screen_available: false,
        screen: None,
        truncated: None,
    }));

    assert_eq!(
        serde_json::to_value(response)?,
        json!({
            "status":"ok",
            "data":{"job":"server","state":"lost","screen_available":false}
        })
    );
    Ok(())
}

#[test]
fn empty_available_screen_remains_distinguishable_from_no_screen()
-> Result<(), Box<dyn std::error::Error>> {
    let response = Response::ok(CommandData::Read(ReadData {
        job: JobName::from_str("server")?,
        state: JobState::Running,
        exit_code: None,
        screen_available: true,
        screen: Some(String::new()),
        truncated: Some(false),
    }));

    assert_eq!(
        serde_json::to_value(response)?,
        json!({
            "status":"ok",
            "data":{
                "job":"server",
                "state":"running",
                "screen_available":true,
                "screen":"",
                "truncated":false
            }
        })
    );
    Ok(())
}

#[test]
fn stop_response_omits_unavailable_last_screen() -> Result<(), Box<dyn std::error::Error>> {
    let response = Response::ok(CommandData::Stop(StopData {
        job: JobName::from_str("server")?,
        cleaned_up: true,
        forced: false,
        screen_available: false,
        last_screen: None,
        truncated: None,
    }));

    assert_eq!(
        serde_json::to_value(response)?,
        json!({
            "status":"ok",
            "data":{
                "job":"server",
                "cleaned_up":true,
                "forced":false,
                "screen_available":false
            }
        })
    );
    Ok(())
}

#[test]
fn list_response_serializes_all_public_states() -> Result<(), Box<dyn std::error::Error>> {
    let response = Response::ok(CommandData::List(ListData {
        jobs: vec![
            JobSummary {
                job: JobName::from_str("active")?,
                state: JobState::Running,
                exit_code: None,
            },
            JobSummary {
                job: JobName::from_str("done")?,
                state: JobState::Exited,
                exit_code: Some(0),
            },
            JobSummary {
                job: JobName::from_str("missing")?,
                state: JobState::Lost,
                exit_code: None,
            },
        ],
    }));

    assert_eq!(
        serde_json::to_value(response)?,
        json!({
            "status":"ok",
            "data":{
                "jobs":[
                    {"job":"active","state":"running"},
                    {"job":"done","state":"exited","exit_code":0},
                    {"job":"missing","state":"lost"}
                ]
            }
        })
    );
    Ok(())
}
