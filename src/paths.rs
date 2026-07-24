use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::error::Error;

#[derive(Debug, Clone)]
pub struct ProjectPaths {
    project_root: PathBuf,
    state_root: PathBuf,
    project_dir: PathBuf,
}

impl ProjectPaths {
    pub fn new(project_root: &Path, state_root: Option<&Path>) -> Result<Self, Error> {
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
        let project_dir = state_root
            .join("projects")
            .join(project_digest(&project_root));
        Ok(Self {
            project_root,
            state_root,
            project_dir,
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
    pub fn state_root(&self) -> &Path {
        &self.state_root
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
        self.state_root.join("bootstrap.lock")
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
