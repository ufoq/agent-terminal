use std::{
    ffi::OsString,
    fs::File,
    io::{self, Read as _, Seek as _, SeekFrom},
    path::Path,
    process::{Command, ExitStatus, Stdio},
    time::Duration,
};

use wait_timeout::ChildExt as _;

use crate::error::Error;

const CAPTURE_LIMIT: u64 = 1024 * 1024;

pub(super) struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn invoke(
    executable: &Path,
    config_dir: &Path,
    xdg_config: &Path,
    xdg_data: &Path,
    xdg_cache: &Path,
    scratch: &Path,
    arguments: &[OsString],
    timeout: Duration,
) -> Result<ProcessOutput, Error> {
    scratch
        .metadata()
        .map_err(|source| Error::state_io("inspect zellij scratch directory", scratch, source))?;
    let mut stdout = tempfile::tempfile()
        .map_err(|source| Error::state_io("create zellij capture file", scratch, source))?;
    let mut stderr = tempfile::tempfile()
        .map_err(|source| Error::state_io("create zellij capture file", scratch, source))?;
    let mut child = Command::new(executable)
        .args(arguments)
        .env("ZELLIJ_CONFIG_DIR", config_dir)
        .env("XDG_CONFIG_HOME", xdg_config)
        .env("XDG_DATA_HOME", xdg_data)
        .env("XDG_CACHE_HOME", xdg_cache)
        .env_remove("ZELLIJ_CONFIG_FILE")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout.try_clone().map_err(|source| {
            Error::state_io("clone zellij capture handle", scratch, source)
        })?))
        .stderr(Stdio::from(stderr.try_clone().map_err(|source| {
            Error::state_io("clone zellij capture handle", scratch, source)
        })?))
        .spawn()
        .map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                Error::ZellijNotFound { source }
            } else {
                tracing::warn!(error = %source, "could not start zellij");
                Error::ZellijFailed {
                    message: "could not start the terminal backend".to_owned(),
                }
            }
        })?;
    let status = match child.wait_timeout(timeout) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _kill = child.kill();
            let _reap = child.wait();
            return Err(Error::ZellijTimeout);
        }
        Err(source) => {
            let _kill = child.kill();
            let _reap = child.wait();
            tracing::warn!(error = %source, "waiting for zellij failed");
            return Err(Error::ZellijFailed {
                message: "waiting for the terminal backend failed".to_owned(),
            });
        }
    };
    Ok(ProcessOutput {
        status,
        stdout: read_capture(&mut stdout, scratch)?,
        stderr: read_capture(&mut stderr, scratch)?,
    })
}

fn read_capture(file: &mut File, scratch: &Path) -> Result<String, Error> {
    let mut bytes = Vec::new();
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.take(CAPTURE_LIMIT).read_to_end(&mut bytes))
        .map_err(|source| Error::state_io("read zellij capture file", scratch, source))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
