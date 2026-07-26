use std::{path::PathBuf, time::Duration};

use agent_terminal::{
    domain::{SessionName, TerminalPaneId},
    state::{ActiveJob, PendingStart, Registry, elapsed_since},
};
use tempfile::TempDir;

use super::common::{
    TestResult, active_metadata_mut, create_project, job, only_record_mut,
    registries_for_all_phases, session_for, set_operation_nonce,
};

#[test]
fn new_registries_have_unique_ownership() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;

    let first = Registry::new(project.clone())?;
    let second = Registry::new(project)?;

    assert_ne!(first.owner_nonce, second.owner_nonce);
    assert_ne!(first.session, second.session);
    Ok(())
}

#[test]
fn session_identity_is_derived_from_project_and_owner_nonce() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let registry = Registry::new(project.clone())?;

    assert_eq!(
        registry.session,
        session_for(&project, &registry.owner_nonce)?
    );
    Ok(())
}

#[test]
fn pending_start_constructor_creates_valid_owned_metadata() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let job = job("api")?;
    let command = vec!["cargo".to_owned(), "run".to_owned()];
    let pending = PendingStart::for_job(&job, project.clone(), command.clone());
    let mut registry = Registry::new(project.clone())?;
    registry.jobs.insert(
        job,
        agent_terminal::state::JobRecord::PendingStart(pending.clone()),
    );

    assert_eq!(pending.operation_nonce.len(), 32);
    assert!(
        pending
            .operation_nonce
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    assert_eq!(
        pending.title,
        format!("agent-terminal:api:{}", &pending.operation_nonce[..12])
    );
    assert_eq!(pending.cwd, project);
    assert_eq!(pending.command, command);
    assert_eq!(pending.pane_id, None);
    assert!(registry.validate(&registry.project_root).is_ok());
    Ok(())
}

#[test]
fn active_job_conversion_preserves_pending_metadata() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let job = job("api")?;
    let pending = PendingStart::for_job(
        &job,
        project,
        vec!["sh".to_owned(), "-c".to_owned(), "serve".to_owned()],
    );
    let pane_id = TerminalPaneId::new(73);

    let active = ActiveJob::from_pending(pending.clone(), pane_id);

    assert_eq!(active.operation_nonce, pending.operation_nonce);
    assert_eq!(active.title, pending.title);
    assert_eq!(active.cwd, pending.cwd);
    assert_eq!(active.command, pending.command);
    assert_eq!(active.pane_id, pane_id);
    Ok(())
}

#[test]
fn elapsed_since_future_timestamp_saturates_at_zero() {
    assert_eq!(elapsed_since(u64::MAX), Duration::ZERO);
}

#[test]
fn registry_validation_accepts_all_three_valid_phases() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;

    for registry in registries_for_all_phases(&project)? {
        assert!(registry.validate(&project).is_ok());
    }
    Ok(())
}

#[test]
fn registry_validation_rejects_unsupported_version() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let mut registry = Registry::new(project.clone())?;
    registry.version += 1;

    assert!(registry.validate(&project).is_err());
    Ok(())
}

#[test]
fn registry_validation_rejects_project_root_mismatch() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let other = create_project(temp.path(), "other")?;
    let registry = Registry::new(project)?;

    assert!(registry.validate(&other).is_err());
    Ok(())
}

#[test]
fn registry_validation_rejects_nonce_length_boundaries() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;

    for nonce in ["a".repeat(31), "b".repeat(33)] {
        let mut registry = Registry::new(project.clone())?;
        registry.owner_nonce.clone_from(&nonce);
        registry.session = session_for(&project, &nonce)?;
        assert!(registry.validate(&project).is_err());

        for mut phased in registries_for_all_phases(&project)? {
            set_operation_nonce(only_record_mut(&mut phased)?, &nonce);
            assert!(phased.validate(&project).is_err());
        }
    }
    Ok(())
}

#[test]
fn registry_validation_rejects_uppercase_and_non_hex_nonce() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;

    for nonce in ["A".repeat(32), "g".repeat(32)] {
        let mut registry = Registry::new(project.clone())?;
        registry.owner_nonce.clone_from(&nonce);
        registry.session = session_for(&project, &nonce)?;
        assert!(registry.validate(&project).is_err());

        for mut phased in registries_for_all_phases(&project)? {
            set_operation_nonce(only_record_mut(&mut phased)?, &nonce);
            assert!(phased.validate(&project).is_err());
        }
    }
    Ok(())
}

#[test]
fn registry_validation_rejects_session_mismatch() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let mut registry = Registry::new(project.clone())?;
    registry.session = SessionName::new("agent-terminal-wrong-owner".to_owned())?;

    assert!(registry.validate(&project).is_err());
    Ok(())
}

#[test]
fn registry_validation_rejects_unowned_titles_in_every_phase() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;

    for mut registry in registries_for_all_phases(&project)? {
        let (title, _, _) = active_metadata_mut(only_record_mut(&mut registry)?);
        *title = "foreign-pane".to_owned();
        assert!(registry.validate(&project).is_err());
    }
    Ok(())
}

#[test]
fn registry_validation_rejects_relative_cwd_in_every_phase() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;

    for mut registry in registries_for_all_phases(&project)? {
        let (_, cwd, _) = active_metadata_mut(only_record_mut(&mut registry)?);
        *cwd = PathBuf::from("relative");
        assert!(registry.validate(&project).is_err());
    }
    Ok(())
}

#[test]
fn registry_validation_rejects_empty_command_in_every_phase() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;

    for mut registry in registries_for_all_phases(&project)? {
        let (_, _, command) = active_metadata_mut(only_record_mut(&mut registry)?);
        command.clear();
        assert!(registry.validate(&project).is_err());
    }
    Ok(())
}
