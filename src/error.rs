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
    #[error("job {job:?} is still running")]
    JobStillRunning { job: String },
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
            Self::JobStillRunning { .. } => "job_still_running",
            Self::LockBusy => "lock_busy",
            Self::ZellijNotFound { .. } => "zellij_not_found",
            Self::ZellijFailed { .. } | Self::PendingStartAbsent { .. } => "zellij_failed",
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
            | Self::JobStillRunning { .. }
            | Self::LockBusy => 1,
            Self::InvalidInput { .. }
            | Self::PendingStartAbsent { .. }
            | Self::ZellijNotFound { .. }
            | Self::ZellijFailed { .. }
            | Self::StateIo { .. }
            | Self::StateCorrupt { .. }
            | Self::StateSerialize { .. } => 2,
        }
    }

    #[must_use]
    pub const fn hint(&self) -> Option<&'static str> {
        match self {
            Self::JobNotFound { .. } => Some("Run list to see known jobs."),
            Self::JobNotRunning { .. } => Some("Read the job before sending input."),
            Self::JobStillRunning { .. } => Some("Retry stop with --force to close the pane."),
            Self::LockBusy => Some("Retry after the current controller operation finishes."),
            Self::ZellijNotFound { .. } => Some("Install Zellij and ensure it is on PATH."),
            _ => None,
        }
    }
}
