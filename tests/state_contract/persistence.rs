use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use agent_terminal::{
    domain::TerminalPaneId,
    error::Error,
    paths::ProjectPaths,
    state::{ActiveJob, JobRecord, PendingRemove, PendingStart, Registry, StateStore},
};
use tempfile::TempDir;

use super::common::{TestResult, create_project, job};

#[test]
fn load_missing_state_returns_unsaved_registry() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let paths = ProjectPaths::new(&project, Some(&temp.path().join("state")))?;
    let state_file = paths.state_file();
    let store = StateStore::new(paths);
    let mut locked = store.try_lock()?;

    let registry = locked.load_or_create(&project)?;

    assert_eq!(registry.project_root, project);
    assert!(registry.jobs.is_empty());
    assert!(registry.validate(&registry.project_root).is_ok());
    assert!(!state_file.exists());
    Ok(())
}

#[test]
fn all_job_phases_round_trip_without_field_loss() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let paths = ProjectPaths::new(&project, Some(&temp.path().join("state")))?;
    let store = StateStore::new(paths);
    let mut registry = Registry::new(project.clone())?;
    let pending_job = job("pending")?;
    let active_job = job("active")?;
    let removing_job = job("removing")?;
    let pending = PendingStart::for_job(
        &pending_job,
        project.clone(),
        vec!["pending-command".to_owned()],
    );
    let active_pending = PendingStart::for_job(
        &active_job,
        project.clone(),
        vec!["active-command".to_owned()],
    );
    let removing_pending = PendingStart::for_job(
        &removing_job,
        project.clone(),
        vec!["removing-command".to_owned()],
    );
    registry
        .jobs
        .insert(pending_job, JobRecord::PendingStart(pending));
    registry.jobs.insert(
        active_job,
        JobRecord::Active(ActiveJob::from_pending(
            active_pending,
            TerminalPaneId::new(12),
        )),
    );
    registry.jobs.insert(
        removing_job,
        JobRecord::PendingRemove(PendingRemove {
            job: ActiveJob::from_pending(removing_pending, TerminalPaneId::new(13)),
            force_authorized: true,
        }),
    );

    let mut locked = store.try_lock()?;
    locked.save(&registry)?;
    let loaded = locked.load_or_create(&project)?;

    assert_eq!(loaded, registry);
    Ok(())
}

#[test]
fn large_registry_and_command_round_trip() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let store = StateStore::new(ProjectPaths::new(
        &project,
        Some(&temp.path().join("state")),
    )?);
    let mut registry = Registry::new(project.clone())?;
    let large_argument = "x".repeat(64 * 1024);
    for index in 0..128 {
        let job = job(&format!("worker-{index:03}"))?;
        registry.jobs.insert(
            job.clone(),
            JobRecord::PendingStart(PendingStart::for_job(
                &job,
                project.clone(),
                vec!["runner".to_owned(), large_argument.clone()],
            )),
        );
    }

    let mut locked = store.try_lock()?;
    locked.save(&registry)?;
    let loaded = locked.load_or_create(&project)?;

    assert_eq!(loaded, registry);
    Ok(())
}

#[test]
fn atomic_replacement_never_exposes_partial_json() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let paths = ProjectPaths::new(&project, Some(&temp.path().join("state")))?;
    let state_file = paths.state_file();
    let store = StateStore::new(paths);
    let first = Registry::new(project.clone())?;
    let second = Registry::new(project)?;
    let mut locked = store.try_lock()?;
    locked.save(&first)?;
    let writer_done = Arc::new(AtomicBool::new(false));
    let writer_done_clone = Arc::clone(&writer_done);
    let first_for_writer = first.clone();
    let second_for_writer = second.clone();

    let writer = thread::spawn(move || {
        let result = (|| -> Result<(), Error> {
            for iteration in 0..200 {
                let registry = if iteration % 2 == 0 {
                    &second_for_writer
                } else {
                    &first_for_writer
                };
                locked.save(registry)?;
            }
            Ok(())
        })();
        writer_done_clone.store(true, Ordering::Release);
        result
    });

    let mut reads = 0_usize;
    while !writer_done.load(Ordering::Acquire) {
        let bytes = fs::read(&state_file)?;
        let observed: Registry = serde_json::from_slice(&bytes)?;
        assert!(observed == first || observed == second);
        reads += 1;
    }
    let writer_result = writer
        .join()
        .map_err(|_| std::io::Error::other("state writer thread panicked"))?;
    writer_result?;
    assert!(reads > 0);
    Ok(())
}

#[test]
fn state_path_shape_collisions_return_typed_errors() -> TestResult {
    let temp = TempDir::new()?;
    let state_root = temp.path().join("state");

    let project_dir_collision = ProjectPaths::new(
        &create_project(temp.path(), "project-dir-collision")?,
        Some(&state_root),
    )?;
    let project_parent = project_dir_collision
        .project_dir()
        .parent()
        .ok_or_else(|| std::io::Error::other("project state directory has no parent"))?;
    fs::create_dir_all(project_parent)?;
    fs::write(project_dir_collision.project_dir(), b"collision")?;
    assert!(matches!(
        StateStore::new(project_dir_collision).try_lock(),
        Err(Error::StateIo { .. })
    ));

    let lock_collision = ProjectPaths::new(
        &create_project(temp.path(), "lock-collision")?,
        Some(&state_root),
    )?;
    fs::create_dir_all(lock_collision.lock_file())?;
    assert!(matches!(
        StateStore::new(lock_collision).try_lock(),
        Err(Error::StateIo { .. })
    ));

    let state_collision_project = create_project(temp.path(), "state-collision")?;
    let state_collision = ProjectPaths::new(&state_collision_project, Some(&state_root))?;
    let state_collision_store = StateStore::new(state_collision.clone());
    let mut locked = state_collision_store.try_lock()?;
    fs::create_dir(state_collision.state_file())?;
    assert!(matches!(
        locked.load_or_create(&state_collision_project),
        Err(Error::StateIo { .. })
    ));
    Ok(())
}
