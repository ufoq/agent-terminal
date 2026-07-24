use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

use super::{
    PaneSnapshot, PaneSpec, PaneTarget, Zellij, parse_panes,
    process::{ProcessOutput, invoke},
};
use crate::{
    domain::{SessionName, TerminalPaneId},
    error::Error,
};

macro_rules! os_args {
    ($($value:expr),* $(,)?) => {
        [$(OsString::from($value)),*]
    };
}

pub struct ZellijCli {
    executable: PathBuf,
    config: PathBuf,
    timeout: Duration,
}

impl ZellijCli {
    #[must_use]
    pub const fn new(executable: PathBuf, config: PathBuf, timeout: Duration) -> Self {
        Self {
            executable,
            config,
            timeout,
        }
    }

    fn checked(&self, arguments: &[OsString], timeout: Duration) -> Result<ProcessOutput, Error> {
        let output = self.invoke(arguments, timeout)?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(Error::ZellijFailed {
                message: format!(
                    "exit {}: {}",
                    output
                        .status
                        .code()
                        .map_or_else(|| "signal".to_owned(), |code| code.to_string()),
                    output.stderr.trim()
                ),
            })
        }
    }

    fn invoke(&self, arguments: &[OsString], timeout: Duration) -> Result<ProcessOutput, Error> {
        invoke(&self.executable, &self.config, arguments, timeout)
    }
}

impl Zellij for ZellijCli {
    fn create_background(
        &self,
        session: &SessionName,
        layout: &Path,
        timeout: Duration,
    ) -> Result<(), Error> {
        self.checked(
            &os_args![
                "attach",
                "--create-background",
                session.as_str(),
                "options",
                "--default-layout",
                layout.as_os_str(),
                "--session-serialization",
                "false",
                "--show-release-notes",
                "false",
                "--show-startup-tips",
                "false",
            ],
            timeout,
        )?;
        Ok(())
    }

    fn session_exists(&self, session: &SessionName, timeout: Duration) -> Result<bool, Error> {
        let output = self.invoke(
            &os_args!["list-sessions", "--short", "--no-formatting"],
            timeout,
        )?;
        if output.status.success() {
            Ok(output.stdout.lines().any(|line| line == session.as_str()))
        } else if output.stderr.contains("No active zellij sessions") {
            Ok(false)
        } else {
            Err(Error::ZellijFailed {
                message: output.stderr.trim().to_owned(),
            })
        }
    }

    fn list_panes(
        &self,
        session: &SessionName,
        timeout: Duration,
    ) -> Result<Vec<PaneSnapshot>, Error> {
        let output = self.checked(
            &os_args![
                "--session",
                session.as_str(),
                "action",
                "list-panes",
                "--json",
            ],
            timeout,
        )?;
        parse_panes(&output.stdout)
    }

    fn create_pane(&self, spec: &PaneSpec, timeout: Duration) -> Result<TerminalPaneId, Error> {
        let mut arguments = Vec::from(os_args![
            "--session",
            spec.session.as_str(),
            "run",
            "--name",
            spec.title.as_str(),
            "--cwd",
            spec.cwd.as_os_str(),
            "--",
        ]);
        arguments.extend(spec.command.iter().map(OsString::from));
        let output = self.checked(&arguments, timeout)?;
        output.stdout.trim().parse()
    }

    fn dump_screen(
        &self,
        target: &PaneTarget,
        path: &Path,
        timeout: Duration,
    ) -> Result<(), Error> {
        if path.exists() {
            return Err(Error::InvalidInput {
                message: format!("screen dump path already exists: {}", path.display()),
            });
        }
        self.checked(
            &os_args![
                "--session",
                target.session.as_str(),
                "action",
                "dump-screen",
                "--path",
                path.as_os_str(),
                "--pane-id",
                target.pane_id.get().to_string(),
                "--full",
            ],
            timeout,
        )?;
        if !path.is_file() {
            return Err(Error::ZellijFailed {
                message: format!("dump-screen did not create {}", path.display()),
            });
        }
        Ok(())
    }

    fn paste(&self, target: &PaneTarget, text: &str, timeout: Duration) -> Result<(), Error> {
        self.checked(
            &os_args![
                "--session",
                target.session.as_str(),
                "action",
                "paste",
                "--pane-id",
                target.pane_id.get().to_string(),
                text,
            ],
            timeout,
        )?;
        Ok(())
    }

    fn send_keys(
        &self,
        target: &PaneTarget,
        keys: &[String],
        timeout: Duration,
    ) -> Result<(), Error> {
        let mut arguments = Vec::from(os_args![
            "--session",
            target.session.as_str(),
            "action",
            "send-keys",
            "--pane-id",
            target.pane_id.get().to_string(),
        ]);
        arguments.extend(keys.iter().map(OsString::from));
        self.checked(&arguments, timeout)?;
        Ok(())
    }

    fn close_pane(&self, target: &PaneTarget, timeout: Duration) -> Result<(), Error> {
        self.checked(
            &os_args![
                "--session",
                target.session.as_str(),
                "action",
                "close-pane",
                "--pane-id",
                target.pane_id.get().to_string(),
            ],
            timeout,
        )?;
        Ok(())
    }

    fn kill_session(&self, session: &SessionName, timeout: Duration) -> Result<(), Error> {
        self.checked(&os_args!["kill-session", session.as_str()], timeout)?;
        Ok(())
    }

    fn command_timeout(&self) -> Duration {
        self.timeout
    }
}
