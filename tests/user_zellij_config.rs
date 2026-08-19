use std::{ffi::OsString, fs, path::Path};

use agent_terminal::config::ensure_user_zellij_config_with;
use tempfile::TempDir;

/// The exact minimal KDL content the ensure step writes. Mirrors
/// `src/config.rs::CONFIG`.
const MINIMAL_KDL: &str = "show_release_notes false\nshow_startup_tips false\nsession_serialization false\nsimplified_ui true\npane_frames false\n";

/// Builds a lookup closure from a static map of env-var → path.
/// Keys absent from the map are treated as unset; `None` values are treated
/// as unset (so the caller can explicitly express "not present").
fn env_lookup<'a>(
    envs: &'a [(&'a str, Option<&'a Path>)],
) -> impl Fn(&str) -> Option<OsString> + 'a {
    move |var: &str| {
        envs.iter()
            .find(|(k, _)| *k == var)
            .and_then(|(_, v)| v.map(|path| path.as_os_str().to_os_string()))
    }
}

#[test]
fn missing_config_created_under_xdg_config_home() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let xdg = temp.path().join("xdg");
    let envs = [("XDG_CONFIG_HOME", Some(xdg.as_path())), ("HOME", None)];
    let lookup = env_lookup(&envs);

    assert!(ensure_user_zellij_config_with(&lookup).is_ok());

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
    let envs = [("XDG_CONFIG_HOME", Some(xdg.as_path()))];
    let lookup = env_lookup(&envs);

    assert!(ensure_user_zellij_config_with(&lookup).is_ok());

    assert_eq!(fs::read_to_string(&config)?, existing);
    Ok(())
}

#[test]
fn home_fallback_when_xdg_config_home_unset() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let home = temp.path().join("home");
    let envs = [("XDG_CONFIG_HOME", None), ("HOME", Some(home.as_path()))];
    let lookup = env_lookup(&envs);

    assert!(ensure_user_zellij_config_with(&lookup).is_ok());

    let config = home.join(".config").join("zellij").join("config.kdl");
    assert!(config.exists(), "config not created under HOME/.config");
    assert_eq!(fs::read_to_string(&config)?, MINIMAL_KDL);
    Ok(())
}

#[test]
fn skipped_when_xdg_config_home_and_home_unset() {
    let envs: [(&str, Option<&Path>); 0] = [];
    let lookup = env_lookup(&envs);
    assert!(ensure_user_zellij_config_with(&lookup).is_ok());
}
