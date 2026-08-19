use std::{
    ffi::OsString,
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;

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
/// The file is created atomically with `create_new` (mode 0600 on Unix) so a
/// config written by a concurrent process is never truncated or overwritten;
/// `AlreadyExists` is treated as success.
pub(crate) fn ensure_user_zellij_config_with(
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
        source: io::Error::new(
            io::ErrorKind::InvalidInput,
            "config path has no parent directory",
        ),
    })?;
    fs::create_dir_all(dir).map_err(|source| Error::StateIo {
        action: "ensure user zellij configuration",
        path: config_path.clone(),
        source,
    })?;
    set_private_dir(dir)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    match options.open(&config_path) {
        Ok(mut file) => {
            file.write_all(CONFIG.as_bytes())
                .map_err(|source| Error::StateIo {
                    action: "ensure user zellij configuration",
                    path: config_path.clone(),
                    source,
                })?;
            file.flush().map_err(|source| Error::StateIo {
                action: "ensure user zellij configuration",
                path: config_path.clone(),
                source,
            })?;
            // `mode(0o600)` is filtered by the process umask; re-apply exact
            // permissions after writing so the file is private regardless.
            set_private_file(&config_path)
        }
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(source) => Err(Error::StateIo {
            action: "ensure user zellij configuration",
            path: config_path.clone(),
            source,
        }),
    }
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

#[cfg(test)]
mod tests {
    use super::{CONFIG, ensure_user_zellij_config_with, user_zellij_config_path};
    use std::{
        ffi::OsString,
        fs,
        path::{Path, PathBuf},
    };
    use tempfile::TempDir;

    /// Builds a lookup closure from a static map of env-var → path.
    /// Keys absent from the map are treated as unset; `None` values are
    /// treated as unset (so the caller can explicitly express "not present").
    fn env_lookup<'a>(
        envs: &'a [(&'a str, Option<&'a Path>)],
    ) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |var: &str| {
            envs.iter()
                .find(|(key, _)| *key == var)
                .and_then(|(_, value)| value.map(|path| path.as_os_str().to_os_string()))
        }
    }

    #[test]
    fn xdg_config_home_wins_over_home() {
        let xdg = PathBuf::from("/xdg");
        let home = PathBuf::from("/home");
        let envs = [
            ("XDG_CONFIG_HOME", Some(xdg.as_path())),
            ("HOME", Some(home.as_path())),
        ];
        let lookup = env_lookup(&envs);
        assert_eq!(
            user_zellij_config_path(&lookup),
            Some(PathBuf::from("/xdg/zellij/config.kdl"))
        );
    }

    #[test]
    fn home_is_fallback_when_xdg_config_home_is_empty() {
        let home = PathBuf::from("/home");
        let envs = [
            ("XDG_CONFIG_HOME", Some(Path::new(""))),
            ("HOME", Some(home.as_path())),
        ];
        let lookup = env_lookup(&envs);
        assert_eq!(
            user_zellij_config_path(&lookup),
            Some(PathBuf::from("/home/.config/zellij/config.kdl"))
        );
    }

    #[test]
    fn empty_home_falls_through_to_none() {
        let envs = [
            ("XDG_CONFIG_HOME", Some(Path::new(""))),
            ("HOME", Some(Path::new(""))),
        ];
        let lookup = env_lookup(&envs);
        assert_eq!(user_zellij_config_path(&lookup), None);
    }

    #[test]
    fn both_variables_unset_resolves_to_none() {
        let envs: [(&str, Option<&Path>); 2] = [("XDG_CONFIG_HOME", None), ("HOME", None)];
        let lookup = env_lookup(&envs);
        assert_eq!(user_zellij_config_path(&lookup), None);
    }

    #[test]
    fn variables_absent_from_lookup_treated_as_unset() {
        let envs: [(&str, Option<&Path>); 0] = [];
        let lookup = env_lookup(&envs);
        assert_eq!(user_zellij_config_path(&lookup), None);
    }

    #[test]
    fn missing_config_created_under_xdg_config_home() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let xdg = temp.path().join("xdg");
        let envs = [("XDG_CONFIG_HOME", Some(xdg.as_path())), ("HOME", None)];
        let lookup = env_lookup(&envs);

        ensure_user_zellij_config_with(&lookup)?;

        let config = xdg.join("zellij").join("config.kdl");
        assert!(config.exists(), "config not created under XDG_CONFIG_HOME");
        assert_eq!(fs::read_to_string(&config)?, CONFIG);
        Ok(())
    }

    #[test]
    fn missing_config_created_under_home_fallback() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let home = temp.path().join("home");
        let envs = [("XDG_CONFIG_HOME", None), ("HOME", Some(home.as_path()))];
        let lookup = env_lookup(&envs);

        ensure_user_zellij_config_with(&lookup)?;

        let config = home.join(".config").join("zellij").join("config.kdl");
        assert!(config.exists(), "config not created under HOME/.config");
        assert_eq!(fs::read_to_string(&config)?, CONFIG);
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

        ensure_user_zellij_config_with(&lookup)?;

        assert_eq!(fs::read_to_string(&config)?, existing);
        Ok(())
    }

    #[test]
    fn concurrent_create_second_writer_is_success() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let xdg = temp.path().join("xdg");
        let envs = [("XDG_CONFIG_HOME", Some(xdg.as_path()))];
        let lookup = env_lookup(&envs);

        ensure_user_zellij_config_with(&lookup)?;
        ensure_user_zellij_config_with(&lookup)?;

        let config = xdg.join("zellij").join("config.kdl");
        assert_eq!(fs::read_to_string(&config)?, CONFIG);
        Ok(())
    }

    #[test]
    fn skipped_when_xdg_config_home_and_home_unset() -> Result<(), Box<dyn std::error::Error>> {
        let envs: [(&str, Option<&Path>); 0] = [];
        let lookup = env_lookup(&envs);
        ensure_user_zellij_config_with(&lookup)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn created_config_has_0600_mode() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = TempDir::new()?;
        let xdg = temp.path().join("xdg");
        let envs = [("XDG_CONFIG_HOME", Some(xdg.as_path()))];
        let lookup = env_lookup(&envs);

        ensure_user_zellij_config_with(&lookup)?;

        let config = xdg.join("zellij").join("config.kdl");
        assert_eq!(fs::metadata(&config)?.permissions().mode() & 0o777, 0o600);
        Ok(())
    }
}
