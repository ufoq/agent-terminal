use std::str::FromStr;

use agent_terminal::{
    domain::{JobName, JobState, TerminalPaneId},
    error::Error,
    state::{JobRecord, PendingStart},
};

use crate::{
    fixture::Fixture,
    support::{Fault, Operation, pane},
};

#[test]
fn pending_start_is_adopted_by_unique_owned_title() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("pending")?;
    let pending = fixture.pending(&job, None, false);
    fixture
        .registry
        .jobs
        .insert(job.clone(), JobRecord::PendingStart(pending.clone()));
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.panes = vec![
        pane(4, "foreign-pane", false),
        pane(7, &pending.title, false),
    ];
    fixture.save_current()?;

    let listed = fixture.controller().list()?;

    assert_eq!(listed.jobs[0].job, job);
    assert_eq!(listed.jobs[0].state, JobState::Running);
    assert!(matches!(
        fixture.reload()?.jobs.get(&job),
        Some(JobRecord::Active(active)) if active.pane_id == TerminalPaneId::new(7)
    ));
    Ok(())
}

#[test]
fn pending_start_with_pane_id_adopts_only_the_exact_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("pending")?;
    let pending = fixture.pending(&job, Some(8), false);
    fixture
        .registry
        .jobs
        .insert(job.clone(), JobRecord::PendingStart(pending.clone()));
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.panes = vec![
        pane(7, &pending.title, false),
        pane(8, &pending.title, true),
    ];
    fixture.save_current()?;

    let read = fixture.controller().read(&job)?;

    assert_eq!(read.state, JobState::Exited);
    assert!(matches!(
        fixture.reload()?.jobs.get(&job),
        Some(JobRecord::Active(active)) if active.pane_id == TerminalPaneId::new(8)
    ));
    Ok(())
}

#[test]
fn stale_pending_start_reconciles_to_job_not_found() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("pending")?;
    fixture.registry.jobs.insert(
        job.clone(),
        JobRecord::PendingStart(fixture.pending(&job, Some(8), true)),
    );
    fixture.fake.state()?.session_exists = false;
    fixture.save_current()?;

    let result = fixture.controller().read(&job);

    assert!(matches!(result, Err(Error::JobNotFound { .. })));
    assert!(!fixture.reload()?.jobs.contains_key(&job));
    Ok(())
}

#[test]
fn plugin_with_pending_title_is_not_adopted() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("pending")?;
    let pending = fixture.pending(&job, None, true);
    let mut plugin = pane(7, &pending.title, false);
    plugin.is_plugin = true;
    fixture
        .registry
        .jobs
        .insert(job.clone(), JobRecord::PendingStart(pending));
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.panes = vec![plugin];
    fixture.save_current()?;

    let result = fixture.controller().list();

    assert!(matches!(result, Err(Error::PendingStartAbsent { .. })));
    assert!(matches!(
        fixture.reload()?.jobs.get(&job),
        Some(JobRecord::PendingStart(_))
    ));
    Ok(())
}

#[test]
fn expired_pending_start_without_id_is_preserved_on_list_failure()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("pending")?;
    fixture.registry.jobs.insert(
        job.clone(),
        JobRecord::PendingStart(fixture.pending(&job, None, true)),
    );
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.pane_listing = Fault::Fail;
    fixture.save_current()?;

    let result = fixture.controller().list();

    assert!(matches!(result, Err(Error::ZellijFailed { .. })));
    assert!(matches!(
        fixture.reload()?.jobs.get(&job),
        Some(JobRecord::PendingStart(_))
    ));
    Ok(())
}

#[test]
fn stop_cleans_expired_pending_start_without_owned_pane() -> Result<(), Box<dyn std::error::Error>>
{
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("pending")?;
    fixture.registry.jobs.insert(
        job.clone(),
        JobRecord::PendingStart(fixture.pending(&job, None, true)),
    );
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.panes = vec![fixture.keeper()];
    fixture.save_current()?;

    fixture.controller().stop(&job)?;

    assert!(!fixture.reload()?.jobs.contains_key(&job));
    assert_eq!(fixture.fake.state()?.killed_sessions, 1);
    Ok(())
}

#[test]
fn session_lookup_failure_preserves_pending_start() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("pending")?;
    fixture.registry.jobs.insert(
        job.clone(),
        JobRecord::PendingStart(fixture.pending(&job, None, true)),
    );
    fixture.fake.state()?.session_lookup = Fault::Fail;
    fixture.save_current()?;

    let result = fixture.controller().list();

    assert!(matches!(result, Err(Error::ZellijFailed { .. })));
    assert!(matches!(
        fixture.reload()?.jobs.get(&job),
        Some(JobRecord::PendingStart(_))
    ));
    Ok(())
}

#[test]
fn pane_listing_failure_preserves_pending_start() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("pending")?;
    fixture.registry.jobs.insert(
        job.clone(),
        JobRecord::PendingStart(fixture.pending(&job, None, true)),
    );
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.pane_listing = Fault::Fail;
    fixture.save_current()?;

    let result = fixture.controller().read(&job);

    assert!(matches!(result, Err(Error::ZellijFailed { .. })));
    assert!(matches!(
        fixture.reload()?.jobs.get(&job),
        Some(JobRecord::PendingStart(_))
    ));
    Ok(())
}

#[test]
fn create_background_failure_leaves_recoverable_pending_start()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let job = JobName::from_str("start")?;
    fixture.fake.state()?.create_background = Fault::Fail;

    let result = fixture
        .controller()
        .start(&job, None, vec!["sh".to_owned()], &fixture.project);

    assert!(matches!(result, Err(Error::ZellijFailed { .. })));
    assert!(matches!(
        fixture.reload()?.jobs.get(&job),
        Some(JobRecord::PendingStart(PendingStart { pane_id: None, .. }))
    ));
    Ok(())
}

#[test]
fn create_pane_failure_leaves_recoverable_pending_start() -> Result<(), Box<dyn std::error::Error>>
{
    let fixture = Fixture::new()?;
    let job = JobName::from_str("start")?;
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.panes = vec![fixture.keeper()];
    fixture.fake.state()?.create_pane = Fault::Fail;
    fixture.save_current()?;

    let result = fixture
        .controller()
        .start(&job, None, vec!["sh".to_owned()], &fixture.project);

    assert!(matches!(result, Err(Error::ZellijFailed { .. })));
    assert!(matches!(
        fixture.reload()?.jobs.get(&job),
        Some(JobRecord::PendingStart(PendingStart { pane_id: None, .. }))
    ));
    assert!(
        fixture
            .fake
            .state()?
            .operations
            .contains(&Operation::CreatePane)
    );
    Ok(())
}

#[test]
fn read_reconciles_an_absent_owned_session_to_job_not_found()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("read")?;
    fixture.install_active(&job, 1);
    fixture.save_current()?;

    let result = fixture.controller().read(&job);

    assert!(matches!(result, Err(Error::JobNotFound { .. })));
    assert!(!fixture.reload()?.jobs.contains_key(&job));
    Ok(())
}

#[test]
fn same_pane_id_with_foreign_title_reconciles_to_job_not_found()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("read")?;
    let peer = JobName::from_str("peer")?;
    fixture.install_active(&job, 1);
    fixture.install_active(&peer, 2);
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.panes = vec![
        pane(1, "foreign-pane", false),
        fixture.owned_pane(&peer, 2, false),
    ];
    fixture.save_current()?;

    let result = fixture.controller().read(&job);

    assert!(matches!(result, Err(Error::JobNotFound { .. })));
    assert!(!fixture.reload()?.jobs.contains_key(&job));
    assert!(fixture.reload()?.jobs.contains_key(&peer));
    assert!(
        !fixture
            .fake
            .state()?
            .operations
            .contains(&Operation::DumpScreen)
    );
    Ok(())
}
