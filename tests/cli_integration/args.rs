use super::{TestResult, persisted_command};
use crate::support::cli_harness::{DeterministicHarness, assert_json_error};

#[test]
fn send_requires_argument_separator() {
    let harness = DeterministicHarness::new();
    assert_json_error(&harness.run(&["send", "job", "text"]), "invalid_input", 2);
    assert_json_error(
        &harness.run(&["send", "job", "--", "text"]),
        "job_not_found",
        1,
    );
}

#[test]
fn empty_send_text_rejected() {
    let harness = DeterministicHarness::new();
    assert_json_error(&harness.run(&["send", "job", "--", ""]), "invalid_input", 2);
}

#[test]
fn unsupported_key_rejected() {
    let harness = DeterministicHarness::new();
    assert_json_error(
        &harness.run(&["press", "job", "--", "F13"]),
        "invalid_input",
        2,
    );
}

#[test]
fn start_without_command_is_rejected() {
    let harness = DeterministicHarness::new();
    assert_json_error(&harness.run(&["start", "job"]), "invalid_input", 2);
}

#[test]
fn start_persists_argv_without_shell_reinterpretation() -> TestResult {
    let harness = DeterministicHarness::new();
    let marker = harness.root.path().join("shell-marker");
    let shell_expression = format!("$(touch {})", marker.display());

    let output = harness.run(&[
        "start",
        "job",
        "--",
        "/bin/printf",
        "argument with spaces",
        &shell_expression,
        ";",
        "*",
    ]);

    assert_json_error(&output, "zellij_not_found", 2);
    assert!(!marker.exists());
    assert_eq!(
        persisted_command(&harness)?,
        vec![
            "/bin/printf".to_owned(),
            "argument with spaces".to_owned(),
            shell_expression,
            ";".to_owned(),
            "*".to_owned(),
        ]
    );
    Ok(())
}

#[test]
fn start_preserves_an_empty_argv_element() -> TestResult {
    let harness = DeterministicHarness::new();

    let output = harness.run(&["start", "job", "--", "/bin/printf", ""]);

    assert_json_error(&output, "zellij_not_found", 2);
    assert_eq!(
        persisted_command(&harness)?,
        vec!["/bin/printf".to_owned(), String::new()]
    );
    Ok(())
}

#[test]
fn start_preserves_unicode_argv() -> TestResult {
    let harness = DeterministicHarness::new();

    let output = harness.run(&["start", "job", "--", "/bin/printf", "日本語", "🦀"]);

    assert_json_error(&output, "zellij_not_found", 2);
    assert_eq!(
        persisted_command(&harness)?,
        vec![
            "/bin/printf".to_owned(),
            "日本語".to_owned(),
            "🦀".to_owned(),
        ]
    );
    Ok(())
}
