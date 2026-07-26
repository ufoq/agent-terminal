use std::fs;

use super::{TestResult, registry_value};
use crate::support::cli_harness::{DeterministicHarness, assert_json_error};

#[test]
fn nonexistent_cwd_fails_typed() {
    let harness = DeterministicHarness::new();
    let missing = harness.root.path().join("missing-cwd");
    assert_json_error(
        &harness.run(&[
            "start",
            "job",
            "--cwd",
            &missing.to_string_lossy(),
            "--",
            "/bin/true",
        ]),
        "state_io",
        2,
    );
}

#[test]
fn relative_cwd_is_resolved_from_invocation_directory() -> TestResult {
    let mut harness = DeterministicHarness::new();
    harness.current_dir(harness.root.path().to_path_buf());

    let output = harness.run(&["start", "job", "--cwd", "project", "--", "/bin/true"]);

    assert_json_error(&output, "zellij_not_found", 2);
    assert_eq!(
        registry_value(&harness)?["jobs"]["job"]["cwd"],
        harness.project.to_string_lossy().as_ref()
    );
    Ok(())
}

#[test]
fn cwd_that_is_a_file_is_rejected() -> TestResult {
    let harness = DeterministicHarness::new();
    let file = harness.root.path().join("cwd-file");
    fs::write(&file, b"not a directory")?;

    let output = harness.run(&[
        "start",
        "job",
        "--cwd",
        &file.to_string_lossy(),
        "--",
        "/bin/true",
    ]);

    assert_json_error(&output, "invalid_input", 2);
    Ok(())
}

#[cfg(unix)]
#[test]
fn cwd_symlink_is_persisted_canonically() -> TestResult {
    use std::os::unix::fs::symlink;

    let harness = DeterministicHarness::new();
    let target = harness.root.path().join("cwd-target");
    let link = harness.root.path().join("cwd-link");
    fs::create_dir(&target)?;
    symlink(&target, &link)?;

    let output = harness.run(&[
        "start",
        "job",
        "--cwd",
        &link.to_string_lossy(),
        "--",
        "/bin/true",
    ]);

    assert_json_error(&output, "zellij_not_found", 2);
    assert_eq!(
        registry_value(&harness)?["jobs"]["job"]["cwd"],
        target.canonicalize()?.to_string_lossy().as_ref()
    );
    Ok(())
}
