use std::str::FromStr;

use agent_terminal::{domain::JobName, error::Error, state::JobRecord};

use crate::{
    fixture::Fixture,
    support::{AfterCtrlC, DestructiveEffect, Fault, Operation},
};

fn stop_fixture(exited: bool) -> Result<(Fixture, JobName), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("stop")?;
    fixture.install_active(&job, 1);
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.panes = vec![fixture.keeper(), fixture.owned_pane(&job, 1, exited)];
    fixture.save_current()?;
    Ok((fixture, job))
}

#[test]
fn stop_running_pane_auto_escalates_to_close_after_grace_period()
-> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = stop_fixture(false)?;

    fixture.controller().stop(&job)?;

    let operations = &fixture.fake.state()?.operations;
    let ctrl_c = operations
        .iter()
        .position(|operation| operation == &Operation::SendKeys(vec!["Ctrl c".to_owned()]));
    let close = operations
        .iter()
        .position(|operation| operation == &Operation::ClosePane);
    assert!(matches!((ctrl_c, close), (Some(ctrl_c), Some(close)) if ctrl_c < close));
    assert!(!fixture.reload()?.jobs.contains_key(&job));
    Ok(())
}

#[test]
fn stop_exited_pane_closes_without_sending_ctrl_c() -> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = stop_fixture(true)?;

    fixture.controller().stop(&job)?;

    assert!(
        !fixture
            .fake
            .state()?
            .operations
            .contains(&Operation::SendKeys(vec!["Ctrl c".to_owned()]))
    );
    assert!(!fixture.reload()?.jobs.contains_key(&job));
    Ok(())
}

#[test]
fn graceful_stop_sends_ctrl_c_then_closes_after_exit() -> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = stop_fixture(false)?;
    fixture.fake.state()?.after_ctrl_c = AfterCtrlC::Exit;

    fixture.controller().stop(&job)?;

    let operations = &fixture.fake.state()?.operations;
    let ctrl_c = operations
        .iter()
        .position(|operation| operation == &Operation::SendKeys(vec!["Ctrl c".to_owned()]));
    let close = operations
        .iter()
        .position(|operation| operation == &Operation::ClosePane);
    assert!(matches!((ctrl_c, close), (Some(ctrl_c), Some(close)) if ctrl_c < close));
    assert!(!fixture.reload()?.jobs.contains_key(&job));
    Ok(())
}

#[test]
fn graceful_stop_lookup_failure_keeps_active_record() -> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = stop_fixture(false)?;
    fixture.fake.state()?.session_lookup = Fault::Fail;

    let result = fixture.controller().stop(&job);

    assert!(matches!(result, Err(Error::ZellijFailed { .. })));
    assert!(matches!(
        fixture.reload()?.jobs.get(&job),
        Some(JobRecord::Active(_))
    ));
    Ok(())
}

#[test]
fn close_error_is_tolerated_when_pane_disappeared() -> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = stop_fixture(true)?;
    fixture.fake.state()?.close = DestructiveEffect::FailAndDisappear;

    fixture.controller().stop(&job)?;

    assert!(!fixture.reload()?.jobs.contains_key(&job));
    Ok(())
}

#[test]
fn close_error_is_preserved_when_pane_remains() -> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = stop_fixture(true)?;
    fixture.fake.state()?.close = DestructiveEffect::FailAndRemain;

    let result = fixture.controller().stop(&job);

    assert!(matches!(result, Err(Error::ZellijFailed { .. })));
    assert!(matches!(
        fixture.reload()?.jobs.get(&job),
        Some(JobRecord::PendingRemove(_))
    ));
    Ok(())
}

#[test]
fn kill_error_is_tolerated_when_session_disappeared() -> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = stop_fixture(true)?;
    fixture.fake.state()?.kill = DestructiveEffect::FailAndDisappear;

    fixture.controller().stop(&job)?;

    assert!(!fixture.reload()?.jobs.contains_key(&job));
    Ok(())
}

#[test]
fn kill_error_is_preserved_when_session_remains() -> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = stop_fixture(true)?;
    fixture.fake.state()?.kill = DestructiveEffect::FailAndRemain;

    let result = fixture.controller().stop(&job);

    assert!(matches!(result, Err(Error::ZellijFailed { .. })));
    assert!(matches!(
        fixture.reload()?.jobs.get(&job),
        Some(JobRecord::PendingRemove(_))
    ));
    Ok(())
}

#[test]
fn stop_does_not_capture_a_final_screen() -> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = stop_fixture(true)?;
    fixture.fake.state()?.dump = Fault::Fail;

    fixture.controller().stop(&job)?;

    assert!(
        !fixture
            .fake
            .state()?
            .operations
            .contains(&Operation::DumpScreen)
    );
    assert!(!fixture.reload()?.jobs.contains_key(&job));
    Ok(())
}
