use serde_json::Value;

use super::TestResult;
use crate::support::cli_harness::{DeterministicHarness, assert_json_error};

#[test]
fn invalid_global_flag_rejected() {
    let harness = DeterministicHarness::new();
    assert_json_error(&harness.run(&["--unknown", "list"]), "invalid_input", 2);
}

#[test]
fn missing_subcommand_returns_json_invalid_input() {
    let harness = DeterministicHarness::new();
    assert_json_error(&harness.run(&[]), "invalid_input", 2);
}

#[test]
fn subcommand_help_remains_plain_text() -> TestResult {
    let harness = DeterministicHarness::new();

    let output = harness.run(&["start", "--help"]);

    assert!(output.status.success());
    assert!(serde_json::from_slice::<Value>(&output.stdout).is_err());
    assert!(String::from_utf8(output.stdout)?.contains("Usage:"));
    Ok(())
}

#[test]
fn pretty_state_error_preserves_error_semantics() -> TestResult {
    let harness = DeterministicHarness::new();

    let compact = harness.run(&["read", "missing"]);
    let pretty = harness.run(&["--pretty", "read", "missing"]);

    assert_json_error(&compact, "job_not_found", 1);
    assert_json_error(&pretty, "job_not_found", 1);
    assert_eq!(
        serde_json::from_slice::<Value>(&compact.stdout)?,
        serde_json::from_slice::<Value>(&pretty.stdout)?
    );
    Ok(())
}

#[test]
fn job_not_found_error_uses_flat_envelope() -> TestResult {
    let harness = DeterministicHarness::new();

    let output = harness.run(&["read", "missing"]);

    assert_json_error(&output, "job_not_found", 1);
    let response = serde_json::from_slice::<Value>(&output.stdout)?;
    assert_eq!(response["status"], "error");
    assert_eq!(response["code"], "job_not_found");
    assert!(
        response["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty())
    );
    Ok(())
}

#[test]
fn invalid_input_error_uses_flat_envelope() -> TestResult {
    let harness = DeterministicHarness::new();

    let output = harness.run(&["--unknown", "list"]);

    assert_json_error(&output, "invalid_input", 2);
    let response = serde_json::from_slice::<Value>(&output.stdout)?;
    assert_eq!(response["status"], "error");
    assert_eq!(response["code"], "invalid_input");
    assert!(
        response["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty())
    );
    Ok(())
}

#[test]
fn invalid_job_name_rejected_for_each_command() {
    let harness = DeterministicHarness::new();
    for args in [
        &["start", "UPPER", "--", "/bin/true"][..],
        &["read", "UPPER"][..],
        &["send", "UPPER", "--", "text"],
        &["press", "UPPER", "--", "Enter"],
        &["stop", "UPPER"],
    ] {
        assert_json_error(&harness.run(args), "invalid_input", 2);
    }
}

#[test]
fn valid_job_name_boundary_accepted() {
    let harness = DeterministicHarness::new();
    let valid = "a".repeat(64);
    let invalid = "a".repeat(65);
    assert_json_error(&harness.run(&["read", &valid]), "job_not_found", 1);
    assert_json_error(&harness.run(&["read", &invalid]), "invalid_input", 2);
}

#[test]
fn read_missing_job_returns_job_not_found() {
    let harness = DeterministicHarness::new();
    assert_json_error(&harness.run(&["read", "missing"]), "job_not_found", 1);
}

#[test]
fn send_missing_job_returns_job_not_found() {
    let harness = DeterministicHarness::new();
    assert_json_error(
        &harness.run(&["send", "missing", "--", "text"]),
        "job_not_found",
        1,
    );
}

#[test]
fn press_missing_job_returns_job_not_found() {
    let harness = DeterministicHarness::new();
    assert_json_error(
        &harness.run(&["press", "missing", "--", "Enter"]),
        "job_not_found",
        1,
    );
}

#[test]
fn stop_missing_job_returns_job_not_found() {
    let harness = DeterministicHarness::new();
    assert_json_error(&harness.run(&["stop", "missing"]), "job_not_found", 1);
}

#[test]
fn missing_zellij_reports_zellij_not_found() {
    let harness = DeterministicHarness::new();
    assert_json_error(
        &harness.run(&["start", "job", "--", "/bin/true"]),
        "zellij_not_found",
        2,
    );
    assert_json_error(&harness.run(&["list"]), "zellij_not_found", 2);
    assert_json_error(&harness.run(&["stop", "job"]), "zellij_not_found", 2);
}
