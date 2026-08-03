mod cli;
mod process;

use std::{path::Path, time::Duration};

use serde::Deserialize;

use crate::{
    domain::{SessionName, TerminalPaneId},
    error::Error,
};

pub use cli::ZellijCli;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneTarget {
    pub session: SessionName,
    pub pane_id: TerminalPaneId,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSpec {
    pub session: SessionName,
    pub cwd: std::path::PathBuf,
    pub title: String,
    pub command: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaneSnapshot {
    pub id: u32,
    pub is_plugin: bool,
    pub title: String,
    pub exited: bool,
    pub exit_status: Option<i32>,
}

pub trait Zellij {
    fn create_background(
        &self,
        session: &SessionName,
        layout: &Path,
        timeout: Duration,
    ) -> Result<(), Error>;
    fn session_exists(&self, session: &SessionName, timeout: Duration) -> Result<bool, Error>;
    fn list_panes(
        &self,
        session: &SessionName,
        timeout: Duration,
    ) -> Result<Vec<PaneSnapshot>, Error>;
    fn create_pane(&self, spec: &PaneSpec, timeout: Duration) -> Result<TerminalPaneId, Error>;
    fn dump_screen(&self, target: &PaneTarget, path: &Path, timeout: Duration)
    -> Result<(), Error>;
    fn paste(&self, target: &PaneTarget, text: &str, timeout: Duration) -> Result<(), Error>;
    fn send_keys(
        &self,
        target: &PaneTarget,
        keys: &[String],
        timeout: Duration,
    ) -> Result<(), Error>;
    fn close_pane(&self, target: &PaneTarget, timeout: Duration) -> Result<(), Error>;
    fn kill_session(&self, session: &SessionName, timeout: Duration) -> Result<(), Error>;
    fn command_timeout(&self) -> Duration;
}

pub fn parse_panes(stdout: &str) -> Result<Vec<PaneSnapshot>, Error> {
    serde_json::from_str(stdout).map_err(|source| {
        tracing::warn!(error = %source, "invalid list-panes JSON from terminal backend");
        Error::ZellijFailed {
            message: "terminal backend returned invalid pane data".to_owned(),
        }
    })
}

#[must_use]
pub fn find_owned_pane<'a>(
    panes: &'a [PaneSnapshot],
    target: &PaneTarget,
) -> Option<&'a PaneSnapshot> {
    panes.iter().find(|pane| {
        !pane.is_plugin && pane.id == target.pane_id.get() && pane.title == target.title
    })
}
