use std::{fs, str::FromStr};

use agent_terminal::{
    domain::{JobName, TerminalPaneId},
    error::Error,
    state::{ActiveJob, JobRecord, PendingRemove, PendingStart},
};

use crate::{
    fixture::Fixture,
    support::{AfterPaste, Fault, Operation, pane},
};

#[test]
fn stale_last_job_flag_does_not_kill_a_new_job() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let old = JobName::from_str("old")?;
    let new = JobName::from_str("new")?;
    fixture.registry.jobs.insert(
        old,
        JobRecord::PendingRemove(PendingRemove {
            job: fixture.active(&JobName::from_str("old")?, 1),
        }),
    );
    fixture
        .registry
        .jobs
        .insert(new.clone(), JobRecord::Active(fixture.active(&new, 2)));
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.panes = vec![
        fixture.keeper(),
        pane(2, "agent-terminal:new:0123456789ab", false),
    ];
    fixture.save_current()?;

    let listed = fixture.controller().list()?;

    assert_eq!(listed.jobs.len(), 1);
    assert_eq!(listed.jobs[0].job, new);
    assert_eq!(fixture.fake.state()?.killed_sessions, 0);
    Ok(())
}

#[test]
fn removal_kills_the_session_when_it_is_now_last() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("old")?;
    fixture.registry.jobs.insert(
        job,
        JobRecord::PendingRemove(PendingRemove {
            job: fixture.active(&JobName::from_str("old")?, 1),
        }),
    );
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.panes = vec![fixture.keeper()];
    fixture.save_current()?;

    fixture.controller().list()?;

    assert_eq!(fixture.fake.state()?.killed_sessions, 1);
    Ok(())
}

#[test]
fn backend_failure_does_not_discard_pending_start() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("pending")?;
    fixture.registry.jobs.insert(
        job.clone(),
        JobRecord::PendingStart(PendingStart::for_job(
            &job,
            fixture.project.clone(),
            vec!["sh".to_owned()],
        )),
    );
    fixture.fake.state()?.session_lookup = Fault::Fail;
    fixture.save_current()?;

    let result = fixture.controller().stop(&job);

    assert!(matches!(result, Err(Error::ZellijFailed { .. })));
    assert!(matches!(
        fixture.reload()?.jobs.get(&job),
        Some(JobRecord::PendingStart(_))
    ));
    Ok(())
}

#[test]
fn session_deletion_requires_owned_keeper_and_no_foreign_pane()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("old")?;
    fixture.registry.jobs.insert(
        job,
        JobRecord::PendingRemove(PendingRemove {
            job: fixture.active(&JobName::from_str("old")?, 1),
        }),
    );
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.panes = vec![pane(9, "foreign-pane", false)];
    fixture.save_current()?;

    let result = fixture.controller().list();

    assert!(matches!(result, Err(Error::ZellijFailed { .. })));
    assert_eq!(fixture.fake.state()?.killed_sessions, 0);
    Ok(())
}

#[test]
fn recovered_pending_removal_completes_without_force_authorization()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("old")?;
    fixture.registry.jobs.insert(
        job.clone(),
        JobRecord::PendingRemove(PendingRemove {
            job: fixture.active(&job, 1),
        }),
    );
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.panes = vec![
        fixture.keeper(),
        pane(1, "agent-terminal:old:0123456789ab", false),
    ];
    fixture.save_current()?;

    fixture.controller().stop(&job)?;

    assert!(!fixture.reload()?.jobs.contains_key(&job));
    assert!(
        fixture
            .fake
            .state()?
            .operations
            .contains(&Operation::ClosePane)
    );
    Ok(())
}

#[test]
fn submit_revalidates_identity_before_pressing_enter() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("prompt")?;
    fixture.install_active(&job, 1);
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.after_paste = AfterPaste::ReplaceIdentity;
    fixture.fake.state()?.panes = vec![
        fixture.keeper(),
        pane(1, "agent-terminal:prompt:0123456789ab", false),
    ];
    fixture.save_current()?;

    let result = fixture.controller().send(&job, "yes", true);

    assert!(matches!(result, Err(Error::JobNotRunning { .. })));
    assert!(
        !fixture
            .fake
            .state()?
            .operations
            .iter()
            .any(|operation| { matches!(operation, crate::support::Operation::SendKeys(_)) })
    );
    Ok(())
}

#[test]
fn semantically_invalid_registry_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    fixture.save_current()?;
    let mut value = serde_json::to_value(&fixture.registry)?;
    value["session"] = serde_json::Value::String("agent-terminal-foreign-session".to_owned());
    fs::write(
        fixture.store.paths().state_file(),
        serde_json::to_vec(&value)?,
    )?;

    let result = fixture.reload();

    assert!(matches!(result, Err(Error::StateCorrupt { .. })));
    Ok(())
}

#[test]
fn active_title_must_match_its_persisted_operation_nonce() -> Result<(), Box<dyn std::error::Error>>
{
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("api")?;
    fixture.registry.jobs.insert(
        job,
        JobRecord::Active(ActiveJob {
            operation_nonce: "0123456789abcdef0123456789abcdef".to_owned(),
            title: "agent-terminal:api:ffffffffffff".to_owned(),
            cwd: fixture.project.clone(),
            command: vec!["sh".to_owned()],
            pane_id: TerminalPaneId::new(1),
        }),
    );
    fixture.save_current()?;

    assert!(matches!(fixture.reload(), Err(Error::StateCorrupt { .. })));
    Ok(())
}
