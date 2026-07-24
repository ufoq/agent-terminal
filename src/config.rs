use std::{fs, path::Path};

use crate::{error::Error, paths::ProjectPaths};

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
    write_private(&paths.config_file(), CONFIG)?;
    let layout = format!(
        "layout {{\n    pane name=\"agent-terminal:keeper:{}\"\n}}\n",
        &owner_nonce[..12]
    );
    write_private(&paths.layout_file(), &layout)
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
