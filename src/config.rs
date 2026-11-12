use std::{fs, os::unix::fs::PermissionsExt as _, path::Path};

use crate::{
    error::Error,
    paths::{ProjectPaths, ensure_private_dir},
};

const CONFIG: &str = "show_release_notes false\nshow_startup_tips false\nsession_serialization false\nsimplified_ui true\npane_frames false\n";

pub fn write_session_files(
    paths: &ProjectPaths,
    keeper: &Path,
    zellij: &Path,
) -> Result<(), Error> {
    ensure_private_dir(&paths.project_dir())?;
    ensure_private_dir(&paths.xdg_config())?;
    ensure_private_dir(&paths.xdg_data())?;
    ensure_private_dir(&paths.xdg_cache())?;
    let owners = if paths.scope().is_global() {
        "-".to_owned()
    } else {
        paths.owners_file().display().to_string()
    };
    let ready = if paths.scope().is_global() {
        "-".to_owned()
    } else {
        paths.ready_file().display().to_string()
    };
    let layout = format!(
        "layout {{\n    pane name=\"agent-terminal:keeper\" command=\"{}\" {{\n        args \"__keeper\" \"{}\" \"{}\" \"{}\" \"{}\"\n    }}\n}}\n",
        kdl_string(&keeper.display().to_string()),
        kdl_string(&owners),
        kdl_string(&ready),
        kdl_string(&zellij.display().to_string()),
        kdl_string(&paths.lock_file().display().to_string()),
    );
    write_private(&paths.layout_file(), &layout)?;
    let config = format!(
        "{CONFIG}default_layout \"{}\"\n",
        kdl_string(&paths.layout_file().display().to_string())
    );
    write_private(&paths.config_file(), &config)?;
    Ok(())
}

fn write_private(path: &Path, contents: &str) -> Result<(), Error> {
    fs::write(path, contents)
        .map_err(|source| Error::state_io("write session file", path, source))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|source| Error::state_io("set session file permissions", path, source))
}

fn kdl_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
