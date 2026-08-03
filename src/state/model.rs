use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    domain::{JobName, SessionName, TerminalPaneId},
    error::Error,
    paths::project_digest,
};

const STATE_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    pub version: u32,
    pub project_root: PathBuf,
    pub owner_nonce: String,
    pub session: SessionName,
    pub jobs: BTreeMap<JobName, JobRecord>,
}

impl Registry {
    pub fn new(project_root: PathBuf) -> Result<Self, Error> {
        let owner_nonce = Uuid::new_v4().simple().to_string();
        let session_value = expected_session_name(&project_root, &owner_nonce);
        let session = SessionName::new(session_value)?;
        Ok(Self {
            version: STATE_VERSION,
            project_root,
            owner_nonce,
            session,
            jobs: BTreeMap::new(),
        })
    }

    pub fn validate(&self, project_root: &Path) -> Result<(), String> {
        if self.version != STATE_VERSION || self.project_root != project_root {
            return Err("state belongs to a different project or version".to_owned());
        }
        if !valid_nonce(&self.owner_nonce) {
            return Err("owner nonce is not 32 lowercase hexadecimal characters".to_owned());
        }
        if self.session.as_str() != expected_session_name(project_root, &self.owner_nonce) {
            return Err("session name does not match project ownership metadata".to_owned());
        }
        for (job, record) in &self.jobs {
            JobName::from_str(job.as_str()).map_err(|error| error.to_string())?;
            match record {
                JobRecord::PendingStart(pending) => validate_pending(job, pending)?,
                JobRecord::Active(active) => validate_active(job, active)?,
                JobRecord::PendingRemove(pending) => validate_active(job, &pending.job)?,
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum JobRecord {
    PendingStart(PendingStart),
    Active(ActiveJob),
    PendingRemove(PendingRemove),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingStart {
    pub operation_nonce: String,
    pub title: String,
    pub cwd: PathBuf,
    pub command: Vec<String>,
    pub pane_id: Option<TerminalPaneId>,
    pub created_millis: u64,
}

impl PendingStart {
    #[must_use]
    fn new(cwd: PathBuf, command: Vec<String>) -> Self {
        let operation_nonce = Uuid::new_v4().simple().to_string();
        let title = format!("agent-terminal:pending:{}", &operation_nonce[..12]);
        Self {
            operation_nonce,
            title,
            cwd,
            command,
            pane_id: None,
            created_millis: now_millis(),
        }
    }

    #[must_use]
    pub fn for_job(job: &JobName, cwd: PathBuf, command: Vec<String>) -> Self {
        let mut pending = Self::new(cwd, command);
        pending.title = format!("agent-terminal:{job}:{}", &pending.operation_nonce[..12]);
        pending
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveJob {
    pub operation_nonce: String,
    pub title: String,
    pub cwd: PathBuf,
    pub command: Vec<String>,
    pub pane_id: TerminalPaneId,
}

impl ActiveJob {
    #[must_use]
    pub fn from_pending(pending: PendingStart, pane_id: TerminalPaneId) -> Self {
        Self {
            operation_nonce: pending.operation_nonce,
            title: pending.title,
            cwd: pending.cwd,
            command: pending.command,
            pane_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRemove {
    pub job: ActiveJob,
}

#[must_use]
pub fn elapsed_since(created_millis: u64) -> std::time::Duration {
    let elapsed = now_millis().saturating_sub(created_millis);
    std::time::Duration::from_millis(elapsed)
}

#[must_use]
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[must_use]
fn expected_session_name(project_root: &Path, owner_nonce: &str) -> String {
    format!(
        "agent-terminal-{}-{}",
        &project_digest(project_root)[..12],
        &owner_nonce[..8]
    )
}

fn valid_nonce(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn validate_pending(job: &JobName, pending: &PendingStart) -> Result<(), String> {
    if !valid_nonce(&pending.operation_nonce) {
        return Err(format!("pending operation nonce for {job} is invalid"));
    }
    let expected = format!("agent-terminal:{job}:{}", &pending.operation_nonce[..12]);
    if pending.title != expected {
        return Err(format!("pending pane title for {job} is not owned"));
    }
    validate_command(job, &pending.cwd, &pending.command)
}

fn validate_active(job: &JobName, active: &ActiveJob) -> Result<(), String> {
    if !valid_nonce(&active.operation_nonce) {
        return Err(format!("active operation nonce for {job} is invalid"));
    }
    let expected = format!("agent-terminal:{job}:{}", &active.operation_nonce[..12]);
    if active.title != expected {
        return Err(format!("active pane title for {job} is not owned"));
    }
    validate_command(job, &active.cwd, &active.command)
}

fn validate_command(job: &JobName, cwd: &Path, command: &[String]) -> Result<(), String> {
    if !cwd.is_absolute() || command.is_empty() {
        return Err(format!("command metadata for {job} is invalid"));
    }
    Ok(())
}
