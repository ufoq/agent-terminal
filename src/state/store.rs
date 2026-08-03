use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write as _},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use fs2::FileExt as _;

use super::model::Registry;
use crate::{error::Error, paths::ProjectPaths};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct StateStore {
    paths: ProjectPaths,
}

impl StateStore {
    #[must_use]
    pub const fn new(paths: ProjectPaths) -> Self {
        Self { paths }
    }

    #[must_use]
    pub const fn paths(&self) -> &ProjectPaths {
        &self.paths
    }

    pub fn try_lock(&self) -> Result<LockedState, Error> {
        ensure_private_dir(self.paths.project_dir())?;
        let lock_path = self.paths.lock_file();
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| state_io("open lock", &lock_path, source))?;
        set_private_file(&lock_path)?;
        match lock_file.try_lock_exclusive() {
            Ok(()) => Ok(LockedState {
                paths: self.paths.clone(),
                _lock_file: lock_file,
            }),
            Err(source) if source.kind() == io::ErrorKind::WouldBlock => Err(Error::LockBusy),
            Err(source) => Err(state_io("acquire lock", &lock_path, source)),
        }
    }

    pub fn lock_bootstrap(&self, timeout: Duration) -> Result<BootstrapLock, Error> {
        ensure_private_dir(self.paths.scope_root())?;
        let lock_path = self.paths.bootstrap_lock_file();
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| state_io("open bootstrap lock", &lock_path, source))?;
        set_private_file(&lock_path)?;
        let deadline = Instant::now() + timeout;
        loop {
            match lock_file.try_lock_exclusive() {
                Ok(()) => {
                    return Ok(BootstrapLock {
                        _lock_file: lock_file,
                    });
                }
                Err(source) if source.kind() == io::ErrorKind::WouldBlock => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(Error::LockBusy);
                    }
                    thread::sleep(remaining.min(Duration::from_millis(25)));
                }
                Err(source) => {
                    return Err(state_io("acquire bootstrap lock", &lock_path, source));
                }
            }
        }
    }
}

pub struct BootstrapLock {
    _lock_file: File,
}

pub struct LockedState {
    paths: ProjectPaths,
    _lock_file: File,
}

impl LockedState {
    pub fn load_or_create(&mut self, project_root: &Path) -> Result<Registry, Error> {
        let state_path = self.paths.state_file();
        if !state_path.exists() {
            return Registry::new(project_root.to_path_buf());
        }
        let bytes =
            fs::read(&state_path).map_err(|source| state_io("read state", &state_path, source))?;
        let registry: Registry =
            serde_json::from_slice(&bytes).map_err(|source| Error::StateCorrupt {
                path: state_path.clone(),
                source,
            })?;
        registry
            .validate(project_root)
            .map_err(|message| Error::StateCorrupt {
                path: state_path.clone(),
                source: serde_json::Error::io(io::Error::new(io::ErrorKind::InvalidData, message)),
            })?;
        Ok(registry)
    }

    pub fn save(&mut self, registry: &Registry) -> Result<(), Error> {
        ensure_private_dir(self.paths.project_dir())?;
        let state_path = self.paths.state_file();
        let suffix = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_path =
            self.paths
                .project_dir()
                .join(format!(".state.json.{}.{}", std::process::id(), suffix));
        let bytes =
            serde_json::to_vec(registry).map_err(|source| Error::StateSerialize { source })?;
        let result = write_and_rename(&temp_path, &state_path, &bytes);
        if result.is_err()
            && let Err(cleanup_error) = fs::remove_file(&temp_path)
            && cleanup_error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(
                path = %temp_path.display(),
                error = %cleanup_error,
                "failed to remove state temporary file"
            );
        }
        result?;
        if let Err(source) = File::open(self.paths.project_dir()).and_then(|dir| dir.sync_all()) {
            tracing::debug!(
                path = %self.paths.project_dir().display(),
                error = %source,
                "parent directory fsync was unavailable"
            );
        }
        Ok(())
    }
}

fn write_and_rename(temp_path: &Path, state_path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp_path)
        .map_err(|source| state_io("create state temporary file", temp_path, source))?;
    set_private_file(temp_path)?;
    file.write_all(bytes)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .map_err(|source| state_io("write state temporary file", temp_path, source))?;
    fs::rename(temp_path, state_path)
        .map_err(|source| state_io("replace state", state_path, source))
}

fn ensure_private_dir(path: &Path) -> Result<(), Error> {
    fs::create_dir_all(path).map_err(|source| state_io("create state directory", path, source))?;
    set_private_dir(path)
}

#[cfg(unix)]
fn set_private_dir(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|source| state_io("set state directory permissions", path, source))
}

#[cfg(not(unix))]
fn set_private_dir(_path: &Path) -> Result<(), Error> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|source| state_io("set state file permissions", path, source))
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<(), Error> {
    Ok(())
}

fn state_io(action: &'static str, path: &Path, source: io::Error) -> Error {
    Error::StateIo {
        action,
        path: path.to_path_buf(),
        source,
    }
}
