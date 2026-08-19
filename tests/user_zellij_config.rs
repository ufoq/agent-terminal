mod support;

use std::fs;

use crate::support::cli_harness::{DeterministicHarness, assert_json_error};
use tempfile::TempDir;

/// The exact minimal KDL content the ensure step writes. Mirrors
/// `src/config.rs::CONFIG`.
const MINIMAL_KDL: &str = "show_release_notes false\nshow_startup_tips false\nsession_serialization false\nsimplified_ui true\npane_frames false\n";

/// Runs the real `start` command against an isolated harness. The user-level
/// zellij config is ensured before the backend spawn, so the call fails
/// deterministically with `zellij_not_found` only after the config step ran.
fn run_start(harness: &DeterministicHarness) {
    let output = harness.run(&["start", "job", "--", "/bin/true"]);
    assert_json_error(&output, "zellij_not_found", 2);
}

#[test]
fn missing_config_created_under_xdg_config_home() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let xdg = temp.path().join("xdg");
    let mut harness = DeterministicHarness::new();
    harness.env("XDG_CONFIG_HOME", &xdg);
    run_start(&harness);

    let config = xdg.join("zellij").join("config.kdl");
    assert!(config.exists(), "config not created under XDG_CONFIG_HOME");
    assert_eq!(fs::read_to_string(&config)?, MINIMAL_KDL);
    Ok(())
}

#[test]
fn existing_config_left_byte_identical() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let xdg = temp.path().join("xdg");
    let zellij_dir = xdg.join("zellij");
    fs::create_dir_all(&zellij_dir)?;
    let config = zellij_dir.join("config.kdl");
    let existing = "user settings\nshow_release_notes true\n";
    fs::write(&config, existing)?;
    let mut harness = DeterministicHarness::new();
    harness.env("XDG_CONFIG_HOME", &xdg);
    run_start(&harness);

    assert_eq!(fs::read_to_string(&config)?, existing);
    Ok(())
}

#[test]
fn home_fallback_when_xdg_config_home_unset() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let home = temp.path().join("home");
    let mut harness = DeterministicHarness::new();
    harness.env("XDG_CONFIG_HOME", "");
    harness.env("HOME", &home);
    run_start(&harness);

    let config = home.join(".config").join("zellij").join("config.kdl");
    assert!(config.exists(), "config not created under HOME/.config");
    assert_eq!(fs::read_to_string(&config)?, MINIMAL_KDL);
    Ok(())
}

#[test]
fn skipped_when_xdg_config_home_and_home_unset() {
    let mut harness = DeterministicHarness::new();
    harness.env("XDG_CONFIG_HOME", "");
    harness.env("HOME", "");
    run_start(&harness);

    assert!(
        !harness.project.join("zellij").join("config.kdl").exists(),
        "config created although both XDG_CONFIG_HOME and HOME are unset"
    );
}

#[cfg(unix)]
#[test]
fn created_config_has_0600_mode() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt as _;

    let temp = TempDir::new()?;
    let xdg = temp.path().join("xdg");
    let mut harness = DeterministicHarness::new();
    harness.env("XDG_CONFIG_HOME", &xdg);
    run_start(&harness);

    let config = xdg.join("zellij").join("config.kdl");
    assert_eq!(fs::metadata(&config)?.permissions().mode() & 0o777, 0o600);
    Ok(())
}
