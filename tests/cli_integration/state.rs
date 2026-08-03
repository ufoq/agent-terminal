use std::{fs, time::Instant};

use serde_json::Value;

use agent_terminal::paths::scope_digest;

use super::{TestResult, project_paths, registry_value};
use crate::support::cli_harness::{
    CorruptKind, DeterministicHarness, assert_json_error, hold_state_lock, write_corrupt_state,
};

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
    assert_json_error(&harness.run(&["list"]), "state_corrupt", 2);
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
        elapsed < std::time::Duration::from_secs(2),
        "lock attempt took {elapsed:?}"
    );
}

#[test]
fn state_root_that_is_a_file_returns_state_io() -> TestResult {
    let mut harness = DeterministicHarness::new();
    let state_file = harness.root.path().join("state-file");
    fs::write(&state_file, b"not a directory")?;
    harness.state_dir = state_file;

    assert_json_error(&harness.run(&["list"]), "state_io", 2);
    Ok(())
}

#[test]
fn scoped_projects_component_that_is_a_file_returns_state_io() -> TestResult {
    let harness = DeterministicHarness::new();
    let projects_dir = harness
        .state_dir
        .join("scopes")
        .join(scope_digest("standalone"))
        .join("projects");
    fs::create_dir_all(
        projects_dir
            .parent()
            .ok_or("scoped projects path has no parent")?,
    )?;
    fs::write(projects_dir, b"not a directory")?;

    assert_json_error(&harness.run(&["list"]), "state_io", 2);
    Ok(())
}

#[test]
fn lock_path_that_is_a_directory_returns_state_io() -> TestResult {
    let harness = DeterministicHarness::new();
    let paths = project_paths(&harness)?;
    fs::create_dir_all(paths.lock_file())?;

    assert_json_error(&harness.run(&["list"]), "state_io", 2);
    Ok(())
}

#[test]
fn state_path_that_is_a_directory_returns_state_io() -> TestResult {
    let harness = DeterministicHarness::new();
    let paths = project_paths(&harness)?;
    fs::create_dir_all(paths.state_file())?;

    assert_json_error(&harness.run(&["list"]), "state_io", 2);
    Ok(())
}

#[test]
fn empty_state_file_is_corrupt() -> TestResult {
    let harness = DeterministicHarness::new();
    let paths = project_paths(&harness)?;
    fs::create_dir_all(paths.project_dir())?;
    fs::write(paths.state_file(), [])?;

    assert_json_error(&harness.run(&["list"]), "state_corrupt", 2);
    Ok(())
}

#[test]
fn wrong_top_level_json_shapes_are_corrupt() -> TestResult {
    for contents in ["null", "[]", r#""string""#, "42", "{}"] {
        let harness = DeterministicHarness::new();
        let paths = project_paths(&harness)?;
        fs::create_dir_all(paths.project_dir())?;
        fs::write(paths.state_file(), contents)?;

        assert_json_error(&harness.run(&["list"]), "state_corrupt", 2);
    }
    Ok(())
}

#[test]
fn unsupported_state_version_is_corrupt() -> TestResult {
    let harness = DeterministicHarness::new();
    assert_json_error(
        &harness.run(&["start", "job", "--", "/bin/true"]),
        "zellij_not_found",
        2,
    );
    let paths = project_paths(&harness)?;
    let mut registry = registry_value(&harness)?;
    registry["version"] = Value::from(3);
    fs::write(paths.state_file(), serde_json::to_vec(&registry)?)?;

    assert_json_error(&harness.run(&["list"]), "state_corrupt", 2);
    Ok(())
}

#[test]
fn registry_for_another_project_is_corrupt() -> TestResult {
    let harness = DeterministicHarness::new();
    assert_json_error(
        &harness.run(&["start", "job", "--", "/bin/true"]),
        "zellij_not_found",
        2,
    );
    let other_project = harness.root.path().join("other-project");
    fs::create_dir(&other_project)?;
    let paths = project_paths(&harness)?;
    let mut registry = registry_value(&harness)?;
    registry["project_root"] = Value::from(other_project.to_string_lossy().into_owned());
    fs::write(paths.state_file(), serde_json::to_vec(&registry)?)?;

    assert_json_error(&harness.run(&["list"]), "state_corrupt", 2);
    Ok(())
}

#[test]
fn held_lock_rejects_every_stateful_command() {
    let harness = DeterministicHarness::new();
    let _lock = hold_state_lock(&harness);

    for args in [
        &["list"][..],
        &["start", "job", "--", "/bin/true"],
        &["read", "job"],
        &["send", "job", "--", "text"],
        &["press", "job", "--", "Enter"],
        &["stop", "job"],
    ] {
        assert_json_error(&harness.run(args), "lock_busy", 1);
    }
}
