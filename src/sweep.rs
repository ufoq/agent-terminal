use std::{
    fs,
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use fs2::FileExt as _;

use crate::{
    error::Error,
    owner::{any_dead, read_owners},
    paths::ProjectPaths,
    zellij::{ZellijCli, job_panes, keeper_pane, unknown_panes},
};

const SHUTDOWN: Duration = Duration::from_secs(10);

pub fn sweep(paths: &ProjectPaths, zellij: &ZellijCli) -> Result<(), Error> {
    clear_scratch(paths)?;
    sweep_global(paths, zellij)?;
    if !paths.scope().is_global() {
        sweep_scope(
            paths,
            zellij,
            &paths.scope_root(),
            &paths.scope_dir_name(),
            Some(paths.project_digest()),
        )?;
    }
    sweep_scopes(paths, zellij, paths.scope().is_global())
}

fn clear_scratch(paths: &ProjectPaths) -> Result<(), Error> {
    let scratch = paths.scratch_dir();
    match fs::remove_dir_all(&scratch) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(Error::state_io("clear runtime scratch", &scratch, source));
        }
    }
    Ok(())
}

fn sweep_global(paths: &ProjectPaths, zellij: &ZellijCli) -> Result<(), Error> {
    let projects = paths
        .runtime_root()
        .join("scopes")
        .join("global")
        .join("projects");
    let entries = match fs::read_dir(&projects) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            prune_empty(paths, projects.parent().unwrap_or(&projects));
            return Ok(());
        }
        Err(source) => return Err(Error::state_io("list global projects", &projects, source)),
    };
    for entry in entries {
        let entry = entry
            .map_err(|source| Error::state_io("read global project entry", &projects, source))?;
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let digest = entry.file_name().to_string_lossy().into_owned();
        let target = ProjectPaths::for_target(
            paths.runtime_root(),
            crate::paths::Scope::Global,
            &digest,
            None,
        );
        let _guard = if digest == paths.project_digest() {
            None
        } else {
            match try_lock(&target)? {
                Some(guard) => Some(guard),
                None => continue,
            }
        };
        sweep_project(&target, zellij)?;
    }
    prune_empty(paths, projects.parent().unwrap_or(&projects));
    Ok(())
}

fn cleanup_current_if_idle(paths: &ProjectPaths, zellij: &ZellijCli) -> Result<(), Error> {
    if !paths.project_dir().is_dir() {
        return Ok(());
    }
    if !zellij.session_exists_named(&paths.session_name(), zellij.command_timeout())? {
        remove_project(paths)?;
        return Ok(());
    }
    let panes = zellij.list_panes_for(&paths.session_name(), zellij.command_timeout())?;
    let idle = job_panes(&panes).is_empty() && unknown_panes(&panes).is_empty();
    let unhealthy = if paths.scope().is_global() {
        idle
    } else {
        keeper_pane(&panes).is_none() || idle
    };
    if unhealthy && shutdown(&paths.session_name(), zellij)? {
        remove_project(paths)?;
    }
    Ok(())
}

fn sweep_project(paths: &ProjectPaths, zellij: &ZellijCli) -> Result<(), Error> {
    if paths.scope().is_global() {
        return cleanup_current_if_idle(paths, zellij);
    }
    if !paths.project_dir().is_dir() {
        return Ok(());
    }
    let owners = read_owners(&paths.owners_file()).unwrap_or_default();
    if owners.is_empty() {
        if zellij.session_exists_named(&paths.session_name(), zellij.command_timeout())?
            && !shutdown(&paths.session_name(), zellij)?
        {
            return Ok(());
        }
        remove_project(paths)?;
        return Ok(());
    }
    let dead = any_dead(Path::new("/proc"), &owners)?;
    let mut keep = false;
    if zellij.session_exists_named(&paths.session_name(), zellij.command_timeout())? {
        if !dead {
            let panes = zellij.list_panes_for(&paths.session_name(), zellij.command_timeout())?;
            keep = !unknown_panes(&panes).is_empty()
                || (keeper_pane(&panes).is_some() && !job_panes(&panes).is_empty());
        }
        if !keep && !shutdown(&paths.session_name(), zellij)? {
            return Ok(());
        }
    }
    if !keep {
        remove_project(paths)?;
    }
    Ok(())
}

fn sweep_scopes(
    current: &ProjectPaths,
    zellij: &ZellijCli,
    all_projects: bool,
) -> Result<(), Error> {
    let scopes = current.runtime_root().join("scopes");
    let entries = match fs::read_dir(&scopes) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(Error::state_io("list scopes", &scopes, source)),
    };
    for entry in entries {
        let entry = entry.map_err(|source| Error::state_io("read scope entry", &scopes, source))?;
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let scope_dir = entry.path();
        let scope_name = entry.file_name().to_string_lossy().into_owned();
        if scope_name == "global" {
            continue;
        }
        sweep_scope(
            current,
            zellij,
            &scope_dir,
            &scope_name,
            if all_projects {
                None
            } else {
                Some(current.project_digest())
            },
        )?;
    }
    Ok(())
}

fn sweep_scope(
    current: &ProjectPaths,
    zellij: &ZellijCli,
    scope_dir: &Path,
    scope_digest: &str,
    only_project: Option<&str>,
) -> Result<(), Error> {
    let projects = scope_dir.join("projects");
    let entries = match fs::read_dir(&projects) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            prune_empty(current, scope_dir);
            return Ok(());
        }
        Err(source) => return Err(Error::state_io("list scope projects", &projects, source)),
    };
    for entry in entries {
        let entry =
            entry.map_err(|source| Error::state_io("read project entry", &projects, source))?;
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let digest = entry.file_name().to_string_lossy().into_owned();
        if let Some(only) = only_project
            && only != digest
        {
            continue;
        }
        let target = ProjectPaths::for_target(
            current.runtime_root(),
            crate::paths::Scope::Owner(scope_digest.to_owned()),
            &digest,
            Some(scope_digest),
        );
        let _guard = if digest == current.project_digest() {
            None
        } else {
            match try_lock(&target)? {
                Some(guard) => Some(guard),
                None => continue,
            }
        };
        sweep_project(&target, zellij)?;
    }
    prune_empty(current, scope_dir);
    Ok(())
}

fn try_lock(paths: &ProjectPaths) -> Result<Option<fs::File>, Error> {
    let path = paths.lock_file();
    if let Some(parent) = path.parent() {
        crate::paths::ensure_private_dir(parent)?;
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .map_err(|source| Error::state_io("open project lock", &path, source))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(source) => Err(Error::state_io("lock project", &path, source)),
    }
}

fn prune_empty(paths: &ProjectPaths, dir: &Path) {
    let Ok(_metadata) = paths.metadata_lock() else {
        return;
    };
    let _ = fs::remove_dir(dir.join("projects"));
    let _ = fs::remove_dir(dir);
}

pub fn shutdown(session: &str, zellij: &ZellijCli) -> Result<bool, Error> {
    let poll = Duration::from_millis(500);
    let _ = zellij.kill_session_named(session, poll);
    let deadline = Instant::now() + SHUTDOWN;
    while Instant::now() < deadline {
        if !zellij.session_exists_named(session, poll)? {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(100));
    }
    let _ = zellij.delete_session_named(session, poll);
    for _ in 0..10 {
        if !zellij.session_exists_named(session, poll)? {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(false)
}

pub fn remove_project(paths: &ProjectPaths) -> Result<(), Error> {
    let _metadata = paths.metadata_lock()?;
    let result = remove_dir(&paths.project_dir());
    if result.is_ok() {
        if let Some(parent) = paths.project_dir().parent() {
            let _ = fs::remove_dir(parent);
        }
        let _ = fs::remove_dir(paths.scope_root());
    }
    result
}

fn remove_dir(path: &Path) -> Result<(), Error> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::state_io("remove project state", path, source)),
    }
}

#[must_use]
pub fn session_name_for(project_digest: &str, scope_dir: &str) -> String {
    let project = short(project_digest);
    if scope_dir == "global" {
        format!("at-global-{project}")
    } else {
        format!("at-{project}-{}", short(scope_dir))
    }
}

fn short(value: &str) -> &str {
    value.get(..12).unwrap_or(value)
}

#[must_use]
pub fn project_dir_for(root: &Path, scope_dir: &str, project_digest: &str) -> PathBuf {
    root.join("scopes")
        .join(scope_dir)
        .join("projects")
        .join(project_digest)
}
