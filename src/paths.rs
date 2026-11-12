#![allow(clippy::missing_const_for_fn)]

use std::{
    fs,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use fs2::FileExt as _;
use sha2::{Digest as _, Sha256};

use crate::error::Error;

const DIGEST_HEX_LEN: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Owner(String),
    Global,
}

impl Scope {
    fn key(&self) -> &str {
        match self {
            Self::Owner(key) => key,
            Self::Global => "global",
        }
    }

    #[must_use]
    pub const fn is_global(&self) -> bool {
        matches!(self, Self::Global)
    }
}

#[derive(Debug, Clone)]
pub struct ProjectPaths {
    runtime_root: PathBuf,
    project_root: PathBuf,
    scope: Scope,
    project_digest: String,
    scope_digest: Option<String>,
}

impl ProjectPaths {
    pub fn new(project_root: &Path, runtime_root: &Path, scope: Scope) -> Result<Self, Error> {
        let project_root = project_root
            .canonicalize()
            .map_err(|source| Error::state_io("canonicalize project root", project_root, source))?;
        if !project_root.is_dir() {
            return Err(Error::InvalidInput {
                message: format!(
                    "project root is not a directory: {}",
                    project_root.display()
                ),
            });
        }
        let project_digest = digest(&[b"project\0", project_root.as_os_str().as_encoded_bytes()]);
        let scope_digest = match &scope {
            Scope::Owner(key) => Some(digest(&[
                b"scope\0",
                project_root.as_os_str().as_encoded_bytes(),
                b"\0",
                key.as_bytes(),
            ])),
            Scope::Global => None,
        };
        Ok(Self {
            runtime_root: runtime_root.to_path_buf(),
            project_root,
            scope,
            project_digest,
            scope_digest,
        })
    }

    #[must_use]
    pub fn for_target(
        runtime_root: &Path,
        scope: Scope,
        project_digest: &str,
        scope_digest: Option<&str>,
    ) -> Self {
        Self {
            runtime_root: runtime_root.to_path_buf(),
            project_root: PathBuf::new(),
            scope,
            project_digest: project_digest.to_owned(),
            scope_digest: scope_digest.map(str::to_owned),
        }
    }

    pub fn metadata_lock(&self) -> Result<fs::File, Error> {
        let path = self.runtime_root.join(".meta.lock");
        ensure_private_dir(&self.runtime_root)?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(|source| Error::state_io("open metadata lock", &path, source))?;
        file.lock_exclusive()
            .map_err(|source| Error::state_io("lock metadata", &path, source))?;
        Ok(file)
    }

    #[must_use]
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    #[must_use]
    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }

    #[must_use]
    pub fn project_digest(&self) -> &str {
        &self.project_digest
    }

    #[must_use]
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    #[must_use]
    pub fn scope_digest(&self) -> Option<&str> {
        self.scope_digest.as_deref()
    }

    #[must_use]
    pub fn scope_key(&self) -> &str {
        self.scope.key()
    }

    #[must_use]
    pub fn scope_root(&self) -> PathBuf {
        self.runtime_root.join("scopes").join(self.scope_dir_name())
    }

    #[must_use]
    pub fn scope_dir_name(&self) -> String {
        self.scope_digest
            .clone()
            .unwrap_or_else(|| "global".to_owned())
    }

    #[must_use]
    pub fn project_dir(&self) -> PathBuf {
        self.scope_root()
            .join("projects")
            .join(&self.project_digest)
    }

    #[must_use]
    pub fn lock_file(&self) -> PathBuf {
        self.runtime_root
            .join("locks")
            .join(format!("{}.lock", self.project_digest))
    }

    #[must_use]
    pub fn scratch_dir(&self) -> PathBuf {
        self.runtime_root.join("scratch").join(&self.project_digest)
    }

    #[must_use]
    pub fn xdg_root(&self) -> PathBuf {
        self.runtime_root.join("xdg")
    }

    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.project_dir().join("config.kdl")
    }

    #[must_use]
    pub fn layout_file(&self) -> PathBuf {
        self.project_dir().join("layout.kdl")
    }

    #[must_use]
    pub fn owners_file(&self) -> PathBuf {
        self.project_dir().join("owners")
    }

    #[must_use]
    pub fn ready_file(&self) -> PathBuf {
        self.project_dir().join("ready")
    }

    #[must_use]
    pub fn session_name(&self) -> String {
        let project = short(&self.project_digest);
        self.scope_digest.as_deref().map_or_else(
            || format!("at-global-{project}"),
            |scope| format!("at-{project}-{}", short(scope)),
        )
    }

    #[must_use]
    pub fn xdg_config(&self) -> PathBuf {
        self.xdg_root().join("config")
    }

    #[must_use]
    pub fn xdg_data(&self) -> PathBuf {
        self.xdg_root().join("data")
    }

    #[must_use]
    pub fn xdg_cache(&self) -> PathBuf {
        self.xdg_root().join("cache")
    }
}

/// Find the nearest Git root at or above `start`.
///
/// Returns the directory containing `.git`, or `start` itself when no Git root is found.
pub fn find_project_root(start: &Path) -> Result<PathBuf, Error> {
    let start = start
        .canonicalize()
        .map_err(|source| Error::state_io("canonicalize project start", start, source))?;
    let mut current = start.as_path();
    loop {
        if current.join(".git").exists() {
            return Ok(current.to_path_buf());
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => return Ok(start),
        }
    }
}

pub fn runtime_root(state_dir: Option<&Path>) -> Result<PathBuf, Error> {
    let root = if let Some(path) = state_dir {
        path.to_path_buf()
    } else {
        match std::env::var_os("XDG_RUNTIME_DIR") {
            Some(value) if !value.is_empty() => PathBuf::from(value).join("agent-terminal"),
            _ => std::env::temp_dir().join(format!("agent-terminal-{}", effective_uid()?)),
        }
    };
    ensure_runtime_root(&root)?;
    root.canonicalize()
        .map_err(|source| Error::state_io("resolve runtime root", &root, source))
}

fn effective_uid() -> Result<u32, Error> {
    fs::metadata("/proc/self")
        .map(|metadata| metadata.uid())
        .map_err(|source| {
            Error::state_io("read effective uid", PathBuf::from("/proc/self"), source)
        })
}

fn ensure_runtime_root(path: &Path) -> Result<(), Error> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .map_err(|source| Error::state_io("create runtime root", path, source))?;
            fs::symlink_metadata(path)
                .map_err(|source| Error::state_io("inspect runtime root", path, source))?
        }
        Err(source) => {
            return Err(Error::state_io("inspect runtime root", path, source));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(Error::InvalidInput {
            message: "runtime root is not a real directory".to_owned(),
        });
    }
    if metadata.uid() != effective_uid()? {
        return Err(Error::InvalidInput {
            message: "runtime root is not owned by the current user".to_owned(),
        });
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|source| Error::state_io("set runtime root permissions", path, source))
}

pub fn ensure_private_dir(path: &Path) -> Result<(), Error> {
    fs::create_dir_all(path)
        .map_err(|source| Error::state_io("create private directory", path, source))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|source| Error::state_io("set private directory permissions", path, source))
}

fn digest(parts: &[&[u8]]) -> String {
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
        hasher.update([0_u8]);
    }
    let hex = hasher.finalize();
    let mut output = String::with_capacity(DIGEST_HEX_LEN * 2);
    for byte in &hex[..DIGEST_HEX_LEN] {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn short(value: &str) -> &str {
    value.get(..12).unwrap_or(value)
}
