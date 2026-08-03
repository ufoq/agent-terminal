use std::{io, path::PathBuf};

use thiserror::Error as ThisError;

#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum Error {
    #[error("{message}")]
    InvalidInput { message: String },
    #[error("job {job:?} already exists")]
    JobExists { job: String },
    #[error("job {job:?} was not found")]
    JobNotFound { job: String },
    #[error("job {job:?} is not running")]
    JobNotRunning { job: String },
    #[error("read the job before deciding whether to resend")]
    DeliveryUncertain,
    #[error("pending start for job {job:?} has no owned pane after the adoption deadline")]
    PendingStartAbsent { job: String },
    #[error("another controller operation is in progress")]
    LockBusy,
    #[error("zellij executable was not found")]
    ZellijNotFound {
        #[source]
        source: io::Error,
    },
    #[error("zellij operation failed: {message}")]
    ZellijFailed { message: String },
    #[error("zellij operation timed out after the command was dispatched")]
    ZellijTimeout,
    #[error("state {action} failed at {path}: {source}")]
    StateIo {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("state file is corrupt at {path}: {source}")]
    StateCorrupt {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("state serialization failed: {source}")]
    StateSerialize {
        #[source]
        source: serde_json::Error,
    },
}

impl Error {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::InvalidInput { .. } => "invalid_input",
            Self::JobExists { .. } => "job_exists",
            Self::JobNotFound { .. } => "job_not_found",
            Self::JobNotRunning { .. } => "job_not_running",
            Self::DeliveryUncertain => "delivery_uncertain",
            Self::LockBusy => "lock_busy",
            Self::ZellijNotFound { .. } => "zellij_not_found",
            Self::ZellijFailed { .. } | Self::PendingStartAbsent { .. } | Self::ZellijTimeout => {
                "zellij_failed"
            }
            Self::StateIo { .. } | Self::StateSerialize { .. } => "state_io",
            Self::StateCorrupt { .. } => "state_corrupt",
        }
    }

    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::JobExists { .. }
            | Self::JobNotFound { .. }
            | Self::JobNotRunning { .. }
            | Self::DeliveryUncertain
            | Self::LockBusy => 1,
            Self::InvalidInput { .. }
            | Self::PendingStartAbsent { .. }
            | Self::ZellijNotFound { .. }
            | Self::ZellijFailed { .. }
            | Self::ZellijTimeout
            | Self::StateIo { .. }
            | Self::StateCorrupt { .. }
            | Self::StateSerialize { .. } => 2,
        }
    }

    /// Projects an identity-free message for public (model-visible) output.
    /// State paths contain the scope digest (`scopes/<digest>/...` and
    /// `/tmp/agent-terminal-<digest>`), so they must never be published; the
    /// underlying details remain available through tracing.
    #[must_use]
    pub fn public_message(&self) -> String {
        match self {
            Self::StateIo { action, .. } => format!("state operation {action} failed"),
            Self::StateCorrupt { .. } => "state file is corrupt".to_owned(),
            Self::StateSerialize { .. } => "state serialization failed".to_owned(),
            Self::ZellijFailed { message } | Self::InvalidInput { message } => message.clone(),
            _ => self.to_string(),
        }
    }
}
