use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::error::Error;

/// Find the nearest Git root at or above `start`.
///
/// Returns the directory containing `.git`, or `start` itself when no Git root is found.
/// The returned path is not canonicalized; canonicalize it before using as a project root.
pub fn find_project_root(start: &Path) -> Result<PathBuf, Error> {
    let mut current = start;
    loop {
        let git = current.join(".git");
        if git.exists() {
            return Ok(current.to_path_buf());
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    Ok(start.to_path_buf())
}

#[derive(Debug, Clone)]
pub struct ProjectPaths {
    project_root: PathBuf,
    scope_root: PathBuf,
    project_dir: PathBuf,
    scope_digest: String,
}

impl ProjectPaths {
    pub fn new(project_root: &Path, state_root: Option<&Path>, scope: &str) -> Result<Self, Error> {
        let project_root = project_root
            .canonicalize()
            .map_err(|source| Error::StateIo {
                action: "canonicalize project root",
                path: project_root.to_path_buf(),
                source,
            })?;
        if !project_root.is_dir() {
            return Err(Error::InvalidInput {
                message: format!(
                    "project root is not a directory: {}",
                    project_root.display()
                ),
            });
        }
        let state_root = match state_root {
            Some(path) => path.to_path_buf(),
            None => ProjectDirs::from("dev", "agent-terminal", "agent-terminal")
                .and_then(|dirs| dirs.state_dir().map(Path::to_path_buf))
                .ok_or_else(|| Error::InvalidInput {
                    message: "could not determine the operating-system state directory".to_owned(),
                })?,
        };
        let scope_digest = scope_digest(scope);
        let scope_root = state_root.join("scopes").join(&scope_digest);
        let project_dir = scope_root
            .join("projects")
            .join(project_digest(&project_root));
        Ok(Self {
            project_root,
            scope_root,
            project_dir,
            scope_digest,
        })
    }

    #[must_use]
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    #[must_use]
    pub fn project_dir(&self) -> &Path {
        &self.project_dir
    }

    #[must_use]
    pub fn scope_root(&self) -> &Path {
        &self.scope_root
    }

    /// Directory holding this scope's private Zellij socket namespace.
    ///
    /// Derived from the scope digest under the OS temp dir so the IPC socket
    /// path stays well under Zellij's ~107-byte limit; the digest already
    /// isolates one agent scope from another.
    #[must_use]
    pub fn zellij_socket_dir(&self) -> PathBuf {
        std::env::temp_dir().join(format!("agent-terminal-{}", self.scope_digest))
    }

    #[must_use]
    pub fn state_file(&self) -> PathBuf {
        self.project_dir.join("state.json")
    }

    #[must_use]
    pub fn lock_file(&self) -> PathBuf {
        self.project_dir.join("state.lock")
    }

    #[must_use]
    pub fn bootstrap_lock_file(&self) -> PathBuf {
        self.scope_root.join("bootstrap.lock")
    }

    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.project_dir.join("config.kdl")
    }

    #[must_use]
    pub fn layout_file(&self) -> PathBuf {
        self.project_dir.join("layout.kdl")
    }
}

#[must_use]
pub fn project_digest(project_root: &Path) -> String {
    let hash = blake3::hash(project_root.as_os_str().as_encoded_bytes());
    hash.to_hex()[..24].to_owned()
}

/// Environment variable that selects the agent scope.
///
/// When absent, the CLI falls back to [`PI_SESSION_ID_ENV`] (pi/omp's native
/// per-session identity) and then to the stable literal scope `standalone` so
/// direct users keep persistent state across invocations. `OpenCode` injects a
/// stable per-session identity (the session id) through this variable.
pub const SCOPE_ENV: &str = "AGENT_TERMINAL_SCOPE";

/// Environment variable pi (and omp, which is pi-based) injects into bash
/// tool commands to identify the current session.
///
/// Used as a fallback scope source so agent-terminal gets per-session
/// isolation under pi even without the extension.
pub const PI_SESSION_ID_ENV: &str = "PI_SESSION_ID";

/// Resolve the agent scope from a set of environment variable lookups.
///
/// Resolution order:
/// 1. [`SCOPE_ENV`] (`AGENT_TERMINAL_SCOPE`) — explicit, overridable.
/// 2. [`PI_SESSION_ID_ENV`] (`PI_SESSION_ID`) — pi/omp native session id.
/// 3. `"standalone"` — default for direct CLI use.
///
/// Empty or whitespace-only values are treated as absent.
#[must_use]
pub fn resolve_scope_from(lookup: impl Fn(&str) -> Option<String>) -> String {
    for var in [SCOPE_ENV, PI_SESSION_ID_ENV] {
        if let Some(value) = lookup(var) {
            if !value.trim().is_empty() {
                return value;
            }
        }
    }
    "standalone".to_owned()
}

/// Resolve the agent scope from the process environment.
///
/// See [`resolve_scope_from`] for the resolution order.
#[must_use]
pub fn resolve_scope() -> String {
    resolve_scope_from(|var| std::env::var(var).ok())
}

#[must_use]
pub fn scope_digest(scope: &str) -> String {
    let hash = blake3::hash(&[&b"agent-terminal-scope\0"[..], scope.as_bytes()].concat());
    hash.to_hex()[..24].to_owned()
}
