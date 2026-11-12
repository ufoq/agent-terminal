#![allow(clippy::missing_const_for_fn)]

mod process;

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;

use crate::domain::JobName;
use crate::{
    error::Error,
    paths::{ProjectPaths, ensure_private_dir},
};

use process::{ProcessOutput, invoke};

macro_rules! os_args {
    ($($value:expr),* $(,)?) => {
        [$(OsString::from($value)),*]
    };
}

pub const KEEPER_TITLE: &str = "agent-terminal:keeper";
const JOB_PREFIX: &str = "agent-terminal:job:";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaneSnapshot {
    pub id: u32,
    pub is_plugin: bool,
    pub title: String,
    pub exited: bool,
}

impl PaneSnapshot {
    #[must_use]
    pub fn job_name(&self) -> Option<crate::domain::JobName> {
        if self.is_plugin || self.exited {
            return None;
        }
        self.title
            .strip_prefix(JOB_PREFIX)
            .and_then(|name| name.parse().ok())
    }

    #[must_use]
    pub fn is_keeper(&self) -> bool {
        !self.is_plugin && !self.exited && self.title == KEEPER_TITLE
    }

    #[must_use]
    pub fn is_unknown(&self) -> bool {
        !self.is_plugin && self.title != KEEPER_TITLE && self.title_job_name().is_none()
    }

    fn title_job_name(&self) -> Option<JobName> {
        self.title
            .strip_prefix(JOB_PREFIX)
            .and_then(|name| name.parse().ok())
    }
}

#[derive(Debug, Clone)]
pub struct PaneSpec {
    pub session: String,
    pub cwd: PathBuf,
    pub title: String,
    pub command: Vec<String>,
    pub close_on_exit: bool,
}

pub struct ZellijCli {
    executable: PathBuf,
    paths: ProjectPaths,
    timeout: Duration,
}

impl ZellijCli {
    #[must_use]
    pub const fn new(executable: PathBuf, paths: ProjectPaths, timeout: Duration) -> Self {
        Self {
            executable,
            paths,
            timeout,
        }
    }

    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    #[must_use]
    pub fn paths(&self) -> &ProjectPaths {
        &self.paths
    }

    #[must_use]
    pub fn session(&self) -> String {
        self.paths.session_name()
    }

    fn checked(&self, arguments: &[OsString], timeout: Duration) -> Result<ProcessOutput, Error> {
        let output = self.invoke(arguments, timeout)?;
        if output.status.success() {
            Ok(output)
        } else {
            tracing::warn!(
                status = %output.status,
                argument_count = arguments.len(),
                stdout_captured_text_bytes = output.stdout.len(),
                stderr_captured_text_bytes = output.stderr.len(),
                "zellij command failed"
            );
            Err(Error::ZellijFailed {
                message: "terminal backend operation failed".to_owned(),
            })
        }
    }

    fn invoke(&self, arguments: &[OsString], timeout: Duration) -> Result<ProcessOutput, Error> {
        let scratch = self.paths.scratch_dir();
        ensure_private_dir(&scratch)?;
        invoke(
            &self.executable,
            &self.paths.project_dir(),
            &self.paths.xdg_config(),
            &self.paths.xdg_data(),
            &self.paths.xdg_cache(),
            &scratch,
            arguments,
            timeout,
        )
    }

    pub fn create_background(&self, timeout: Duration) -> Result<(), Error> {
        self.checked(
            &os_args!["attach", "--create-background", self.session(),],
            timeout,
        )?;
        Ok(())
    }

    pub fn session_exists(&self, timeout: Duration) -> Result<bool, Error> {
        self.session_exists_named(&self.session(), timeout)
    }

    pub fn session_exists_named(&self, name: &str, timeout: Duration) -> Result<bool, Error> {
        let output = self.invoke(
            &os_args!["list-sessions", "--short", "--no-formatting"],
            timeout,
        )?;
        if output.status.success() {
            Ok(output.stdout.lines().any(|line| line == name))
        } else if output.stderr.contains("No active zellij sessions") {
            Ok(false)
        } else {
            tracing::warn!(
                status = %output.status,
                stdout_captured_text_bytes = output.stdout.len(),
                stderr_captured_text_bytes = output.stderr.len(),
                "zellij list-sessions failed"
            );
            Err(Error::ZellijFailed {
                message: "terminal backend operation failed".to_owned(),
            })
        }
    }

    pub fn list_panes(&self, timeout: Duration) -> Result<Vec<PaneSnapshot>, Error> {
        self.list_panes_for(&self.session(), timeout)
    }

    pub fn list_panes_for(
        &self,
        name: &str,
        timeout: Duration,
    ) -> Result<Vec<PaneSnapshot>, Error> {
        let output = self.checked(
            &os_args!["--session", name, "action", "list-panes", "--json",],
            timeout,
        )?;
        serde_json::from_str(&output.stdout).map_err(|source| {
            tracing::warn!(
                line = source.line(),
                column = source.column(),
                stdout_captured_text_bytes = output.stdout.len(),
                stderr_captured_text_bytes = output.stderr.len(),
                "invalid list-panes JSON"
            );
            Error::ZellijFailed {
                message: "terminal backend returned invalid pane data".to_owned(),
            }
        })
    }

    pub fn create_pane(&self, spec: &PaneSpec, timeout: Duration) -> Result<(), Error> {
        let mut arguments = Vec::from(os_args![
            "--session",
            spec.session.as_str(),
            "action",
            "new-pane",
            "--name",
            spec.title.as_str(),
            "--cwd",
            spec.cwd.as_os_str(),
        ]);
        if spec.close_on_exit {
            arguments.push(OsString::from("--close-on-exit"));
        }
        arguments.push(OsString::from("--"));
        arguments.extend(spec.command.iter().map(OsString::from));
        self.checked(&arguments, timeout)?;
        Ok(())
    }

    pub fn dump_screen(&self, pane_id: u32, path: &Path, timeout: Duration) -> Result<(), Error> {
        self.checked(
            &os_args![
                "--session",
                self.session(),
                "action",
                "dump-screen",
                "--path",
                path.as_os_str(),
                "--pane-id",
                pane_id.to_string(),
                "--full",
            ],
            timeout,
        )?;
        Ok(())
    }

    pub fn paste(&self, pane_id: u32, text: &str, timeout: Duration) -> Result<(), Error> {
        self.checked(
            &os_args![
                "--session",
                self.session(),
                "action",
                "paste",
                "--pane-id",
                pane_id.to_string(),
                "--",
                text,
            ],
            timeout,
        )?;
        Ok(())
    }

    pub fn send_keys(&self, pane_id: u32, keys: &[String], timeout: Duration) -> Result<(), Error> {
        let mut arguments = Vec::from(os_args![
            "--session",
            self.session(),
            "action",
            "send-keys",
            "--pane-id",
            pane_id.to_string(),
        ]);
        arguments.extend(keys.iter().map(OsString::from));
        self.checked(&arguments, timeout)?;
        Ok(())
    }

    pub fn close_pane(&self, pane_id: u32, timeout: Duration) -> Result<(), Error> {
        self.checked(
            &os_args![
                "--session",
                self.session(),
                "action",
                "close-pane",
                "--pane-id",
                pane_id.to_string(),
            ],
            timeout,
        )?;
        Ok(())
    }

    pub fn kill_session(&self, timeout: Duration) -> Result<(), Error> {
        self.kill_session_named(&self.session(), timeout)
    }

    pub fn kill_session_named(&self, name: &str, timeout: Duration) -> Result<(), Error> {
        self.checked(&os_args!["kill-session", name], timeout)?;
        Ok(())
    }

    pub fn delete_session(&self, timeout: Duration) -> Result<(), Error> {
        self.delete_session_named(&self.session(), timeout)
    }

    pub fn delete_session_named(&self, name: &str, timeout: Duration) -> Result<(), Error> {
        self.checked(&os_args!["delete-session", "--force", name], timeout)?;
        Ok(())
    }

    #[must_use]
    pub const fn command_timeout(&self) -> Duration {
        self.timeout
    }
}

#[must_use]
pub fn keeper_pane(panes: &[PaneSnapshot]) -> Option<&PaneSnapshot> {
    panes.iter().find(|pane| pane.is_keeper())
}

#[must_use]
pub fn job_panes(panes: &[PaneSnapshot]) -> Vec<&PaneSnapshot> {
    panes
        .iter()
        .filter(|pane| pane.job_name().is_some())
        .collect()
}

#[must_use]
pub fn unknown_panes(panes: &[PaneSnapshot]) -> Vec<&PaneSnapshot> {
    panes.iter().filter(|pane| pane.is_unknown()).collect()
}

#[must_use]
pub fn find_job<'a>(
    panes: &'a [PaneSnapshot],
    job: &crate::domain::JobName,
) -> Option<&'a PaneSnapshot> {
    panes
        .iter()
        .find(|pane| pane.job_name().as_ref() == Some(job))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)]
    use super::PaneSnapshot;

    #[test]
    fn job_titles_are_parsed() {
        let pane = PaneSnapshot {
            id: 1,
            is_plugin: false,
            title: "agent-terminal:job:build".to_owned(),
            exited: false,
        };
        assert_eq!(pane.job_name().unwrap().as_str(), "build");
        assert!(!pane.is_unknown());
    }

    #[test]
    fn keeper_is_not_a_job() {
        let pane = PaneSnapshot {
            id: 1,
            is_plugin: false,
            title: "agent-terminal:keeper".to_owned(),
            exited: false,
        };
        assert!(pane.is_keeper());
        assert!(pane.job_name().is_none());
        assert!(!pane.is_unknown());
    }

    #[test]
    fn plugin_is_ignored() {
        let pane = PaneSnapshot {
            id: 1,
            is_plugin: true,
            title: "agent-terminal:job:build".to_owned(),
            exited: false,
        };
        assert!(pane.job_name().is_none());
        assert!(!pane.is_unknown());
    }

    #[test]
    fn exited_keeper_is_not_unknown() {
        let pane = PaneSnapshot {
            id: 1,
            is_plugin: false,
            title: "agent-terminal:keeper".to_owned(),
            exited: true,
        };
        assert!(!pane.is_keeper());
        assert!(!pane.is_unknown());
    }

    #[test]
    fn exited_job_is_not_unknown() {
        let pane = PaneSnapshot {
            id: 1,
            is_plugin: false,
            title: "agent-terminal:job:build".to_owned(),
            exited: true,
        };
        assert!(pane.job_name().is_none());
        assert!(!pane.is_unknown());
    }

    #[test]
    fn unknown_pane_is_unknown() {
        let pane = PaneSnapshot {
            id: 1,
            is_plugin: false,
            title: "vim".to_owned(),
            exited: false,
        };
        assert!(pane.is_unknown());
    }
}
