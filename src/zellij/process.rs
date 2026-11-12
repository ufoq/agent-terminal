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
                tracing::warn!(kind = ?source.kind(), "could not start zellij");
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
            tracing::warn!(kind = ?source.kind(), "waiting for zellij failed");
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

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)]

    use std::{ffi::OsString, fs, os::unix::fs::PermissionsExt as _, time::Duration};

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn timeout_is_reported_once_without_replay() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("calls.log");
        let executable = dir.path().join("fake-zellij");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nexec sleep 5\n",
            marker.display()
        );
        fs::write(&executable, script).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

        let Err(error) = invoke(
            &executable,
            dir.path(),
            dir.path(),
            dir.path(),
            dir.path(),
            dir.path(),
            &[OsString::from("action")],
            Duration::from_millis(100),
        ) else {
            panic!("timeout was not reported");
        };
        assert!(matches!(error, Error::ZellijTimeout), "{error:?}");
        assert!(error.to_string().contains("outcome is unknown"), "{error}");
        let calls = fs::read_to_string(&marker).unwrap();
        assert_eq!(calls.lines().count(), 1, "backend was replayed: {calls}");
    }
}
