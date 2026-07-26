use std::{fs, path::PathBuf};

use agent_terminal::paths::ProjectPaths;
use serde_json::Value;

use super::TestResult;
use crate::support::cli_harness::{
    DeterministicHarness, assert_json_error, assert_json_ok, binary, init_git_project,
};

#[test]
fn nonexistent_project_path_fails_typed() {
    let mut harness = DeterministicHarness::new();
    harness.project = harness.root.path().join("missing-project");
    assert_json_error(&harness.run(&["list"]), "state_io", 2);
}

#[test]
fn project_path_that_is_not_a_directory_fails_typed() -> TestResult {
    let mut harness = DeterministicHarness::new();
    let file = harness.root.path().join("project-file");
    fs::write(&file, b"not a directory")?;
    harness.project = file;
    assert_json_error(&harness.run(&["list"]), "invalid_input", 2);
    Ok(())
}

#[test]
fn state_dir_flag_overrides_env() -> TestResult {
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
fn state_directory_environment_is_used_without_flag() -> TestResult {
    let harness = DeterministicHarness::new();
    let env_state = harness.root.path().join("env-state");
    let mut command = binary();
    command
        .env("PATH", "/nonexistent")
        .env("AGENT_TERMINAL_STATE", &env_state)
        .arg("--project")
        .arg(&harness.project)
        .arg("list");

    let output = command.output()?;

    let _response = assert_json_ok(&output);
    let paths = ProjectPaths::new(&harness.project, Some(&env_state))?;
    assert!(paths.lock_file().is_file());
    assert!(!harness.state_dir.join("projects").exists());
    Ok(())
}

#[test]
fn relative_project_path_is_canonicalized() -> TestResult {
    let mut harness = DeterministicHarness::new();
    harness.project = PathBuf::from("project");
    harness.current_dir(harness.root.path().to_path_buf());

    let output = harness.run(&["start", "job", "--", "/bin/true"]);

    assert_json_error(&output, "zellij_not_found", 2);
    let canonical = harness.root.path().join("project").canonicalize()?;
    let paths = ProjectPaths::new(&canonical, Some(&harness.state_dir))?;
    let registry = serde_json::from_slice::<Value>(&fs::read(paths.state_file())?)?;
    assert_eq!(
        registry["project_root"],
        canonical.to_string_lossy().as_ref()
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn project_symlink_uses_the_canonical_scope() -> TestResult {
    use std::os::unix::fs::symlink;

    let mut harness = DeterministicHarness::new();
    let link = harness.root.path().join("project-link");
    symlink(&harness.project, &link)?;
    harness.project = link;

    let output = harness.run(&["start", "job", "--", "/bin/true"]);

    assert_json_error(&output, "zellij_not_found", 2);
    let canonical = harness.project.canonicalize()?;
    let paths = ProjectPaths::new(&canonical, Some(&harness.state_dir))?;
    let registry = serde_json::from_slice::<Value>(&fs::read(paths.state_file())?)?;
    assert_eq!(
        registry["project_root"],
        canonical.to_string_lossy().as_ref()
    );
    Ok(())
}

#[test]
fn implicit_git_root_is_used_as_project_scope() -> TestResult {
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
    assert_json_error(&harness.run(&["read", "gitjob"]), "zellij_not_found", 2);
    Ok(())
}
