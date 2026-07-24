use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read as _},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
use uuid::Uuid;
use wait_timeout::ChildExt as _;

use crate::error::Error;

const CAPTURE_LIMIT: u64 = 1024 * 1024;

pub(super) struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

pub(super) fn invoke(
    executable: &Path,
    config: &Path,
    arguments: &[OsString],
    timeout: Duration,
) -> Result<ProcessOutput, Error> {
    let parent = config.parent().ok_or_else(|| Error::ZellijFailed {
        message: format!("config path has no parent: {}", config.display()),
    })?;
    fs::create_dir_all(parent).map_err(|source| Error::StateIo {
        action: "create zellij capture directory",
        path: parent.to_path_buf(),
        source,
    })?;
    let token = Uuid::new_v4().simple().to_string();
    let captures = CapturePaths {
        stdout: parent.join(format!(".{token}.stdout")),
        stderr: parent.join(format!(".{token}.stderr")),
    };
    let stdout = create_private(&captures.stdout)?;
    let stderr = create_private(&captures.stderr)?;
    let mut child = Command::new(executable)
        .arg("--config")
        .arg(config)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                Error::ZellijNotFound { source }
            } else {
                Error::ZellijFailed {
                    message: format!("could not start zellij: {source}"),
                }
            }
        })?;
    let status = match child.wait_timeout(timeout) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _kill = child.kill();
            let _reap = child.wait();
            return Err(Error::ZellijFailed {
                message: format!("command timed out after {} ms", timeout.as_millis()),
            });
        }
        Err(source) => {
            let _kill = child.kill();
            let _reap = child.wait();
            return Err(Error::ZellijFailed {
                message: format!("waiting for zellij failed: {source}"),
            });
        }
    };
    Ok(ProcessOutput {
        status,
        stdout: read_capture(&captures.stdout)?,
        stderr: read_capture(&captures.stderr)?,
    })
}

fn create_private(path: &Path) -> Result<File, Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path).map_err(|source| Error::StateIo {
        action: "create zellij capture file",
        path: path.to_path_buf(),
        source,
    })
}

fn read_capture(path: &Path) -> Result<String, Error> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(CAPTURE_LIMIT).read_to_end(&mut bytes))
        .map_err(|source| Error::StateIo {
            action: "read zellij capture file",
            path: path.to_path_buf(),
            source,
        })?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

struct CapturePaths {
    stdout: PathBuf,
    stderr: PathBuf,
}

impl Drop for CapturePaths {
    fn drop(&mut self) {
        for path in [&self.stdout, &self.stderr] {
            if let Err(error) = fs::remove_file(path)
                && error.kind() != io::ErrorKind::NotFound
            {
                tracing::debug!(path = %path.display(), %error, "capture cleanup failed");
            }
        }
    }
}
