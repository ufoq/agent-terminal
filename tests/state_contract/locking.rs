use std::{
    fs,
    sync::{Arc, Barrier, mpsc},
    thread,
    time::{Duration, Instant},
};

use agent_terminal::{
    error::Error,
    paths::ProjectPaths,
    state::{Registry, StateStore},
};
use tempfile::TempDir;

use super::common::{TestResult, create_project};

#[test]
fn many_state_lock_contenders_have_one_winner() -> TestResult {
    const CONTENDERS: usize = 24;

    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let store = Arc::new(StateStore::new(ProjectPaths::new(
        &project,
        Some(&temp.path().join("state")),
    )?));
    let start = Arc::new(Barrier::new(CONTENDERS + 1));
    let release_winner = Arc::new(Barrier::new(2));
    let (result_sender, result_receiver) = mpsc::channel();
    let mut contenders = Vec::with_capacity(CONTENDERS);

    for _ in 0..CONTENDERS {
        let store = Arc::clone(&store);
        let start = Arc::clone(&start);
        let release_winner = Arc::clone(&release_winner);
        let result_sender = result_sender.clone();
        contenders.push(thread::spawn(move || -> Result<(), String> {
            start.wait();
            match store.try_lock() {
                Ok(lock) => {
                    result_sender
                        .send(true)
                        .map_err(|error| error.to_string())?;
                    release_winner.wait();
                    drop(lock);
                    Ok(())
                }
                Err(Error::LockBusy) => {
                    result_sender
                        .send(false)
                        .map_err(|error| error.to_string())?;
                    Ok(())
                }
                Err(error) => Err(error.to_string()),
            }
        }));
    }
    drop(result_sender);
    start.wait();

    let winners = result_receiver
        .iter()
        .take(CONTENDERS)
        .filter(|won| *won)
        .count();
    assert_eq!(winners, 1);
    release_winner.wait();
    for contender in contenders {
        let result = contender
            .join()
            .map_err(|_| std::io::Error::other("lock contender thread panicked"))?;
        result.map_err(std::io::Error::other)?;
    }
    Ok(())
}

#[test]
fn different_project_state_locks_are_independent() -> TestResult {
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

    let first_lock = first.try_lock()?;
    let second_lock = second.try_lock()?;

    assert_ne!(first.paths().lock_file(), second.paths().lock_file());
    drop(second_lock);
    drop(first_lock);
    Ok(())
}

#[test]
fn private_modes_are_created_or_repaired() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let paths = ProjectPaths::new(&project, Some(&temp.path().join("state")))?;
    let store = StateStore::new(paths.clone());
    let registry = Registry::new(project)?;

    let mut locked = store.try_lock()?;
    locked.save(&registry)?;
    drop(locked);
    let bootstrap = store.lock_bootstrap(Duration::ZERO)?;
    drop(bootstrap);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(paths.state_root(), fs::Permissions::from_mode(0o777))?;
        fs::set_permissions(paths.project_dir(), fs::Permissions::from_mode(0o777))?;
        fs::set_permissions(paths.lock_file(), fs::Permissions::from_mode(0o666))?;
        fs::set_permissions(paths.state_file(), fs::Permissions::from_mode(0o666))?;
        fs::set_permissions(
            paths.bootstrap_lock_file(),
            fs::Permissions::from_mode(0o666),
        )?;

        let mut repaired = store.try_lock()?;
        repaired.save(&registry)?;
        drop(repaired);
        let repaired_bootstrap = store.lock_bootstrap(Duration::ZERO)?;
        drop(repaired_bootstrap);

        assert_eq!(
            fs::metadata(paths.state_root())?.permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(paths.project_dir())?.permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(paths.lock_file())?.permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(paths.state_file())?.permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(paths.bootstrap_lock_file())?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[cfg(not(unix))]
    {
        assert!(paths.state_file().is_file());
        assert!(paths.lock_file().is_file());
        assert!(paths.bootstrap_lock_file().is_file());
    }
    Ok(())
}

#[test]
fn bootstrap_lock_honors_release_and_timeout_boundaries() -> TestResult {
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
    let first_lock = first.lock_bootstrap(Duration::ZERO)?;

    assert!(matches!(
        second.lock_bootstrap(Duration::ZERO),
        Err(Error::LockBusy)
    ));
    let timeout = Duration::from_millis(40);
    let started = Instant::now();
    assert!(matches!(
        second.lock_bootstrap(timeout),
        Err(Error::LockBusy)
    ));
    assert!(started.elapsed() >= timeout);

    drop(first_lock);
    assert!(second.lock_bootstrap(Duration::ZERO).is_ok());
    Ok(())
}
