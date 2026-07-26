use std::str::FromStr;

use agent_terminal::{
    domain::{JobName, JobState, Key},
    error::Error,
};

use crate::{
    fixture::Fixture,
    support::{AfterPaste, Fault, Operation},
};

fn running_fixture(name: &str) -> Result<(Fixture, JobName), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str(name)?;
    fixture.install_active(&job, 1);
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.panes = vec![fixture.owned_pane(&job, 1, false)];
    fixture.save_current()?;
    Ok((fixture, job))
}

#[test]
fn read_running_pane_returns_screen_and_no_exit_code() -> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = running_fixture("read")?;
    fixture.fake.state()?.screen = b"running\n".to_vec();

    let read = fixture.controller().read(job)?;

    assert_eq!(read.state, JobState::Running);
    assert_eq!(read.exit_code, None);
    assert!(read.screen_available);
    assert_eq!(read.screen.as_deref(), Some("running\n"));
    Ok(())
}

#[test]
fn read_exited_pane_returns_screen_and_exit_status() -> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = running_fixture("read")?;
    fixture.fake.state()?.panes[0].exited = true;
    fixture.fake.state()?.panes[0].exit_status = Some(23);
    fixture.fake.state()?.screen = b"done\n".to_vec();

    let read = fixture.controller().read(job)?;

    assert_eq!(read.state, JobState::Exited);
    assert_eq!(read.exit_code, Some(23));
    assert_eq!(read.screen.as_deref(), Some("done\n"));
    Ok(())
}

#[test]
fn dump_failure_propagates_from_read_and_removes_temporary_file()
-> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = running_fixture("read")?;
    fixture.fake.state()?.dump = Fault::Fail;

    let result = fixture.controller().read(job);

    assert!(matches!(result, Err(Error::ZellijFailed { .. })));
    let dump_paths = fixture.fake.state()?.dump_paths.clone();
    assert_eq!(dump_paths.len(), 1);
    assert!(!dump_paths[0].exists());
    Ok(())
}

#[test]
fn screen_capture_strips_ansi_and_replaces_invalid_utf8() -> Result<(), Box<dyn std::error::Error>>
{
    let (fixture, job) = running_fixture("read")?;
    fixture.fake.state()?.screen = b"\x1b[31mred\x1b[0m:\xff\n".to_vec();

    let read = fixture.controller().read(job)?;

    assert_eq!(read.screen.as_deref(), Some("red:\u{fffd}\n"));
    Ok(())
}

#[test]
fn send_without_submit_pastes_without_sending_keys() -> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = running_fixture("send")?;

    let sent = fixture.controller().send(job, "literal", false)?;

    assert!(!sent.submitted);
    assert_eq!(
        fixture.fake.state()?.operations,
        vec![
            Operation::SessionExists,
            Operation::ListPanes,
            Operation::Paste("literal".to_owned()),
        ]
    );
    Ok(())
}

#[test]
fn send_with_submit_orders_paste_revalidation_and_enter() -> Result<(), Box<dyn std::error::Error>>
{
    let (fixture, job) = running_fixture("send")?;

    let sent = fixture.controller().send(job, "literal", true)?;

    assert!(sent.submitted);
    assert_eq!(
        fixture.fake.state()?.operations,
        vec![
            Operation::SessionExists,
            Operation::ListPanes,
            Operation::Paste("literal".to_owned()),
            Operation::SessionExists,
            Operation::ListPanes,
            Operation::SendKeys(vec!["Enter".to_owned()]),
        ]
    );
    Ok(())
}

#[test]
fn paste_failure_never_sends_enter() -> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = running_fixture("send")?;
    fixture.fake.state()?.paste = Fault::Fail;

    let result = fixture.controller().send(job, "literal", true);

    assert!(matches!(result, Err(Error::ZellijFailed { .. })));
    assert!(
        !fixture
            .fake
            .state()?
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::SendKeys(_)))
    );
    Ok(())
}

#[test]
fn send_revalidates_identity_before_paste() -> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = running_fixture("send")?;
    fixture.fake.state()?.panes[0].title = "foreign-pane".to_owned();

    let result = fixture.controller().send(job, "literal", false);

    assert!(matches!(result, Err(Error::JobNotRunning { .. })));
    assert!(
        !fixture
            .fake
            .state()?
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::Paste(_)))
    );
    Ok(())
}

#[test]
fn post_paste_lookup_failure_never_sends_enter() -> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = running_fixture("send")?;
    fixture.fake.state()?.after_paste = AfterPaste::FailSessionLookup;

    let result = fixture.controller().send(job, "literal", true);

    assert!(matches!(result, Err(Error::ZellijFailed { .. })));
    assert!(
        !fixture
            .fake
            .state()?
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::SendKeys(_)))
    );
    Ok(())
}

#[test]
fn press_maps_public_keys_to_ordered_zellij_tokens() -> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = running_fixture("press")?;
    let keys = [
        Key::from_str("Enter")?,
        Key::from_str("Ctrl+C")?,
        Key::from_str("Alt+x")?,
        Key::from_str("F12")?,
    ];

    let pressed = fixture.controller().press(job, &keys)?;

    assert_eq!(pressed.keys, ["Enter", "Ctrl+C", "Alt+x", "F12"]);
    assert!(
        fixture
            .fake
            .state()?
            .operations
            .contains(&Operation::SendKeys(vec![
                "Enter".to_owned(),
                "Ctrl c".to_owned(),
                "Alt x".to_owned(),
                "F12".to_owned(),
            ]))
    );
    Ok(())
}

#[test]
fn exited_job_rejects_send_and_press_without_backend_input()
-> Result<(), Box<dyn std::error::Error>> {
    let (fixture, job) = running_fixture("input")?;
    fixture.fake.state()?.panes[0].exited = true;

    let send_result = fixture.controller().send(job.clone(), "literal", true);
    let press_result = fixture.controller().press(job, &[Key::from_str("Enter")?]);

    assert!(matches!(send_result, Err(Error::JobNotRunning { .. })));
    assert!(matches!(press_result, Err(Error::JobNotRunning { .. })));
    assert!(
        !fixture
            .fake
            .state()?
            .operations
            .iter()
            .any(|operation| { matches!(operation, Operation::Paste(_) | Operation::SendKeys(_)) })
    );
    Ok(())
}
