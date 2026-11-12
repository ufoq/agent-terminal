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
    #[error(
        "terminal session is absent; no jobs are visible in this project and scope; start again if needed"
    )]
    SessionLost,
    #[error("no pi process was found in the ancestor chain; start a job from pi, or use --global")]
    NoPiAncestor,
    #[error("another controller operation is in progress")]
    LockBusy,
    #[error("zellij executable was not found")]
    ZellijNotFound {
        #[source]
        source: io::Error,
    },
    #[error("zellij operation failed: {message}")]
    ZellijFailed { message: String },
    #[error("terminal backend client timed out; the operation outcome is unknown")]
    ZellijTimeout,
    #[error("state {action} failed: {cause}")]
    StateIo {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
        cause: String,
    },
}

impl Error {
    #[must_use]
    pub fn state_io(
        action: &'static str,
        path: impl AsRef<std::path::Path>,
        source: io::Error,
    ) -> Self {
        let path = path.as_ref();
        let cause = source.raw_os_error().map_or_else(
            || source.kind().to_string(),
            |code| format!("{} (os error {code})", source.kind()),
        );
        Self::StateIo {
            action,
            path: path.to_path_buf(),
            source,
            cause,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::InvalidInput { .. } => "invalid_input",
            Self::JobExists { .. } => "job_exists",
            Self::JobNotFound { .. } => "job_not_found",
            Self::SessionLost => "session_lost",
            Self::NoPiAncestor => "no_pi_ancestor",
            Self::LockBusy => "lock_busy",
            Self::ZellijNotFound { .. } => "zellij_not_found",
            Self::ZellijFailed { .. } | Self::ZellijTimeout => "zellij_failed",
            Self::StateIo { .. } => "state_io",
        }
    }

    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::JobExists { .. } | Self::JobNotFound { .. } | Self::LockBusy => 1,
            Self::InvalidInput { .. }
            | Self::NoPiAncestor
            | Self::ZellijNotFound { .. }
            | Self::ZellijFailed { .. }
            | Self::ZellijTimeout
            | Self::SessionLost
            | Self::StateIo { .. } => 2,
        }
    }

    #[must_use]
    pub fn public_message(&self) -> String {
        match self {
            Self::StateIo { action, cause, .. } => {
                format!("state operation {action} failed: {cause}")
            }
            Self::ZellijFailed { message } | Self::InvalidInput { message } => message.clone(),
            _ => self.to_string(),
        }
    }
}
