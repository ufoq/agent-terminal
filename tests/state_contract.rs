use std::{fs, path::Path, str::FromStr, time::Duration};

use agent_terminal::{
    domain::JobName,
    error::Error,
    paths::ProjectPaths,
    state::{JobRecord, PendingStart, Registry, StateStore},
};
use tempfile::TempDir;

fn create_project(parent: &Path, name: &str) -> Result<std::path::PathBuf, std::io::Error> {
    let project = parent.join(name);
    fs::create_dir_all(&project)?;
    project.canonicalize()
}

#[test]
fn project_paths_isolate_equal_job_names() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let state_root = temp.path().join("state");
    let first = ProjectPaths::new(&create_project(temp.path(), "first")?, Some(&state_root))?;
    let second = ProjectPaths::new(&create_project(temp.path(), "second")?, Some(&state_root))?;

    assert_ne!(first.project_dir(), second.project_dir());
    assert_eq!(first.state_file().file_name(), Some("state.json".as_ref()));
    assert_eq!(first.lock_file().file_name(), Some("state.lock".as_ref()));
    Ok(())
}

#[test]
fn state_lock_fails_fast_when_an_operation_is_active() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let store = StateStore::new(ProjectPaths::new(
        &project,
        Some(&temp.path().join("state")),
    )?);
    let first_lock = store.try_lock()?;

    assert!(matches!(store.try_lock(), Err(Error::LockBusy)));
    drop(first_lock);
    assert!(store.try_lock().is_ok());
    Ok(())
}

#[test]
fn bootstrap_lock_serializes_different_projects() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let state_root = temp.path().join("state");
    let first = StateStore::new(ProjectPaths::new(
        &create_project(temp.path(), "first")?,
        Some(&state_root),
    )?);
    let second = StateStore::new(ProjectPaths::new(
        &create_project(temp.path(), "second")?,
        Some(&state_root),
    )?);

    let first_lock = first.lock_bootstrap(Duration::from_secs(1))?;
    assert!(matches!(
        second.lock_bootstrap(Duration::ZERO),
        Err(Error::LockBusy)
    ));
    drop(first_lock);
    assert!(second.lock_bootstrap(Duration::from_secs(1)).is_ok());
    Ok(())
}

#[test]
fn registry_round_trips_pending_start_atomically() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let paths = ProjectPaths::new(&project, Some(&temp.path().join("state")))?;
    let store = StateStore::new(paths);
    let job = JobName::from_str("api")?;

    {
        let mut locked = store.try_lock()?;
        let mut registry = locked.load_or_create(&project)?;
        registry.jobs.insert(
            job.clone(),
            JobRecord::PendingStart(PendingStart::for_job(
                &job,
                project.clone(),
                vec!["sh".to_owned(), "-c".to_owned(), "sleep 30".to_owned()],
            )),
        );
        locked.save(&registry)?;
    }

    let mut locked = store.try_lock()?;
    let loaded = locked.load_or_create(&project)?;
    assert!(matches!(
        loaded.jobs.get(&job),
        Some(JobRecord::PendingStart(_))
    ));
    assert_eq!(loaded.project_root, project);
    Ok(())
}

#[test]
fn corrupt_state_is_a_typed_error() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let paths = ProjectPaths::new(&project, Some(&temp.path().join("state")))?;
    fs::create_dir_all(paths.project_dir())?;
    fs::write(paths.state_file(), b"not json")?;
    let store = StateStore::new(paths);
    let mut locked = store.try_lock()?;

    assert!(matches!(
        locked.load_or_create(&project),
        Err(Error::StateCorrupt { .. })
    ));
    Ok(())
}

#[test]
fn new_registry_has_owned_session_identity() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let registry = Registry::new(project.clone())?;

    assert_eq!(registry.project_root, project);
    assert!(registry.session.as_str().starts_with("agent-terminal-"));
    assert!(registry.jobs.is_empty());
    Ok(())
}
