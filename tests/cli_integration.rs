mod support;

use std::{
    fs,
    time::{Duration, Instant},
};

use agent_terminal::paths::ProjectPaths;
use serde_json::Value;
use support::cli_harness::*;

#[test]
fn invalid_global_flag_rejected() {
    let harness = DeterministicHarness::new();
    assert_json_error(&harness.run(&["--unknown", "list"]), "invalid_input", 2);
}

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
fn invalid_job_name_rejected_for_each_command() {
    let harness = DeterministicHarness::new();
    for args in [
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
fn nonexistent_project_path_fails_typed() {
    let mut harness = DeterministicHarness::new();
    harness.project = harness.root.path().join("missing-project");
    assert_typed_json_error(&harness.run(&["list"]), 2);
}

#[test]
fn project_path_that_is_not_a_directory_fails_typed() -> Result<(), Box<dyn std::error::Error>> {
    let mut harness = DeterministicHarness::new();
    let file = harness.root.path().join("project-file");
    fs::write(&file, b"not a directory")?;
    harness.project = file;
    assert_typed_json_error(&harness.run(&["list"]), 2);
    Ok(())
}

#[test]
fn nonexistent_cwd_fails_typed() {
    let harness = DeterministicHarness::new();
    let missing = harness.root.path().join("missing-cwd");
    assert_typed_json_error(
        &harness.run(&[
            "start",
            "job",
            "--cwd",
            &missing.to_string_lossy(),
            "--",
            "/bin/true",
        ]),
        2,
    );
}

#[test]
fn state_dir_flag_overrides_env() -> Result<(), Box<dyn std::error::Error>> {
    let mut harness = DeterministicHarness::new();
    let env_state = harness.root.path().join("env-state");
    harness.env("AGENT_TERMINAL_STATE", env_state.as_os_str());

    let _listed = assert_json_ok(&harness.run(&["list"]));

    let flag_state = ProjectPaths::new(&harness.project, Some(&harness.state_dir))?;
    assert!(flag_state.lock_file().is_file());
    assert!(!env_state.exists());
    Ok(())
}

#[test]
fn pretty_and_verbose_preserve_json_semantics() {
    let harness = DeterministicHarness::new();
    let compact = assert_json_ok(&harness.run(&["list"]));
    let pretty = assert_json_ok(&harness.run(&["--pretty", "list"]));
    let verbose = assert_json_ok(&harness.run(&["-vv", "list"]));
    assert_eq!(compact, pretty);
    assert_eq!(compact, verbose);
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

#[test]
fn corrupt_json_state_is_detected() {
    let harness = DeterministicHarness::new();
    write_corrupt_state(&harness, CorruptKind::Syntax);
    assert_json_error(&harness.run(&["list"]), "state_corrupt", 2);
}

#[test]
fn corrupt_semantic_state_is_detected() {
    let harness = DeterministicHarness::new();
    write_corrupt_state(&harness, CorruptKind::Semantic);
    let error = assert_typed_json_error(&harness.run(&["list"]), 2);
    assert!(matches!(
        error.pointer("/error/code").and_then(Value::as_str),
        Some("state_corrupt" | "invalid_state")
    ));
}

#[test]
fn held_state_lock_fails_fast() {
    let harness = DeterministicHarness::new();
    let _lock = hold_state_lock(&harness);
    let started = Instant::now();
    let output = harness.run(&["list"]);
    let elapsed = started.elapsed();
    assert_json_error(&output, "lock_busy", 1);
    assert!(
        elapsed < Duration::from_secs(2),
        "lock attempt took {elapsed:?}"
    );
}

#[test]
fn implicit_git_root_is_used_as_project_scope() -> Result<(), Box<dyn std::error::Error>> {
    let mut harness = DeterministicHarness::new();
    let (_git_fixture, git_root, subdir) = init_git_project();
    harness.current_dir(&subdir).use_implicit_project_scope();

    assert_json_error(
        &harness.run(&["start", "gitjob", "--", "/bin/true"]),
        "zellij_not_found",
        2,
    );
    let paths = ProjectPaths::new(&git_root, Some(&harness.state_dir))?;
    let registry: Value = serde_json::from_slice(&fs::read(paths.state_file())?)?;
    assert_eq!(
        registry["project_root"],
        git_root.to_string_lossy().as_ref()
    );
    assert_eq!(
        registry["jobs"]["gitjob"]["cwd"],
        subdir.to_string_lossy().as_ref()
    );

    harness.current_dir(&git_root);
    let error = assert_typed_json_error(&harness.run(&["read", "gitjob"]), 2);
    assert_ne!(
        error.pointer("/error/code").and_then(Value::as_str),
        Some("job_not_found")
    );
    Ok(())
}

#[test]
fn start_without_command_is_rejected() {
    let harness = DeterministicHarness::new();
    assert_json_error(&harness.run(&["start", "job"]), "invalid_input", 2);
}

fn assert_typed_json_error(output: &std::process::Output, expected_exit: i32) -> Value {
    assert_eq!(output.status.code(), Some(expected_exit));
    let parsed = serde_json::from_slice::<Value>(&output.stdout);
    assert!(parsed.is_ok(), "stdout was not one JSON value: {output:?}");
    let value = parsed.unwrap_or(Value::Null);
    assert!(value.is_object());
    assert_eq!(value.get("status").and_then(Value::as_str), Some("error"));
    assert!(
        value
            .pointer("/error/code")
            .and_then(Value::as_str)
            .is_some()
    );
    value
}
