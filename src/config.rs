use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use crate::{error::Error, paths::ProjectPaths};

/// Behavior-neutral zellij settings. Also written to the user-level config to
/// suppress zellij's first-run setup wizard (see `ensure_user_zellij_config`).
const CONFIG: &str = r"show_release_notes false
show_startup_tips false
session_serialization false
simplified_ui true
pane_frames false
";

pub fn write_private_files(paths: &ProjectPaths, owner_nonce: &str) -> Result<(), Error> {
    if owner_nonce.len() < 12 || !owner_nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::StateCorrupt {
            path: paths.state_file(),
            source: serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "owner nonce is not lowercase hexadecimal",
            )),
        });
    }
    fs::create_dir_all(paths.project_dir()).map_err(|source| Error::StateIo {
        action: "create controller directory",
        path: paths.project_dir().to_path_buf(),
        source,
    })?;
    set_private_dir(paths.project_dir())?;
    ensure_user_zellij_config()?;
    write_private(&paths.config_file(), CONFIG)?;
    let layout = format!(
        "layout {{\n    pane name=\"agent-terminal:keeper:{}\"\n}}\n",
        &owner_nonce[..12]
    );
    write_private(&paths.layout_file(), &layout)
}

/// Environment variable holding the XDG base directory for user configs.
const XDG_CONFIG_HOME_ENV: &str = "XDG_CONFIG_HOME";

/// Environment variable holding the user's home directory.
const HOME_ENV: &str = "HOME";

/// Path of the user-level zellij config, resolved from an environment lookup.
///
/// Resolution order:
/// 1. `$XDG_CONFIG_HOME/zellij/config.kdl`
/// 2. `$HOME/.config/zellij/config.kdl`
/// 3. `None` when neither variable resolves (no config to ensure).
///
/// Empty values are treated as absent. This is a pure function of the lookup
/// so tests can drive it with closures instead of mutating the process
/// environment (which is unsafe on Rust 2024).
fn user_zellij_config_path(lookup: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let base = lookup(XDG_CONFIG_HOME_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            lookup(HOME_ENV)
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })?;
    Some(base.join("zellij").join("config.kdl"))
}

/// Ensure the user-level zellij config exists, resolved from an environment
/// lookup closure (see [`user_zellij_config_path`] for the resolution order).
///
/// Zellij runs a first-run setup wizard when the user-level config does not
/// exist, which hijacks the session and prevents the keeper pane from
/// appearing; pre-creating the file with behavior-neutral settings sidesteps
/// the wizard. An existing user config is left untouched, and when neither
/// `XDG_CONFIG_HOME` nor `HOME` resolves the operation is skipped entirely.
///
/// Public so integration tests can drive it with closures instead of mutating
/// the process environment (which is unsafe on Rust 2024).
pub fn ensure_user_zellij_config_with(
    lookup: &dyn Fn(&str) -> Option<OsString>,
) -> Result<(), Error> {
    let Some(config_path) = user_zellij_config_path(lookup) else {
        return Ok(());
    };
    if config_path.exists() {
        return Ok(());
    }
    let dir = config_path.parent().ok_or_else(|| Error::StateIo {
        action: "ensure user zellij configuration",
        path: config_path.clone(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "config path has no parent directory",
        ),
    })?;
    fs::create_dir_all(dir).map_err(|source| Error::StateIo {
        action: "ensure user zellij configuration",
        path: config_path.clone(),
        source,
    })?;
    set_private_dir(dir)?;
    fs::write(&config_path, CONFIG).map_err(|source| Error::StateIo {
        action: "ensure user zellij configuration",
        path: config_path.clone(),
        source,
    })?;
    set_private_file(&config_path)
}

/// Ensure the user-level zellij config exists, resolved from the process
/// environment. See [`ensure_user_zellij_config_with`].
fn ensure_user_zellij_config() -> Result<(), Error> {
    ensure_user_zellij_config_with(&|var| std::env::var_os(var))
}

fn write_private(path: &Path, contents: &str) -> Result<(), Error> {
    fs::write(path, contents).map_err(|source| Error::StateIo {
        action: "write controller configuration",
        path: path.to_path_buf(),
        source,
    })?;
    set_private_file(path)
}

#[cfg(unix)]
fn set_private_dir(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| Error::StateIo {
        action: "set controller directory permissions",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn set_private_dir(_path: &Path) -> Result<(), Error> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| Error::StateIo {
        action: "set controller file permissions",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<(), Error> {
    Ok(())
}
