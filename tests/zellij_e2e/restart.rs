use super::harness::{Harness, session_is_live};

#[test]
fn running_job_name_can_be_reused_after_force_stop() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("reuse", "printf 'first\n'; while :; do sleep 1; done")?;
    harness.read_until("reuse", |body| body["data"]["state"] == "running")?;

    let stopped = harness.run(&["stop", "reuse", "--force"])?;
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stdout)
    );

    harness.start_ok("reuse", "printf 'second\n'; exit 0")?;
    let second = harness.read_until("reuse", |body| {
        body["data"]["state"] == "exited"
            && body["data"]["screen"]
                .as_str()
                .is_some_and(|screen| screen.contains("second"))
    })?;
    assert_eq!(second["data"]["exit_code"], 0);
    let screen = second["data"]["screen"].as_str().unwrap_or_default();
    assert_eq!(screen.trim(), "second");
    harness.run_ok(&["stop", "reuse"])?;
    Ok(())
}

#[test]
fn exited_job_name_can_be_reused_after_cleanup() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("reuse", "exit 3")?;
    let first = harness.read_until("reuse", |body| body["data"]["state"] == "exited")?;
    assert_eq!(first["data"]["exit_code"], 3);
    harness.run_ok(&["stop", "reuse"])?;

    harness.start_ok("reuse", "exit 7")?;
    let second = harness.read_until("reuse", |body| body["data"]["state"] == "exited")?;
    assert_eq!(second["data"]["exit_code"], 7);
    harness.run_ok(&["stop", "reuse"])?;
    Ok(())
}

#[test]
fn lost_job_name_can_be_reused_after_cleanup() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("reuse", "while :; do sleep 1; done")?;
    harness.read_until("reuse", |body| body["data"]["state"] == "running")?;

    harness.kill_session_externally()?;
    let lost = harness.read_until("reuse", |body| body["data"]["state"] == "lost")?;
    assert_eq!(lost["data"]["screen_available"], false);
    harness.run_ok(&["stop", "reuse"])?;

    harness.start_ok("reuse", "printf 'restarted\n'; while :; do sleep 1; done")?;
    let restarted = harness.read_until("reuse", |body| {
        body["data"]["state"] == "running"
            && body["data"]["screen"]
                .as_str()
                .is_some_and(|screen| screen.contains("restarted"))
    })?;
    assert_eq!(restarted["data"]["state"], "running");
    harness.run_ok(&["stop", "reuse", "--force"])?;
    Ok(())
}

#[test]
fn session_is_recreated_across_repeated_empty_transitions() -> Result<(), Box<dyn std::error::Error>>
{
    let harness = Harness::new()?;

    for cycle in 1..=3 {
        let marker = format!("cycle-{cycle}");
        let command = format!("printf '{marker}\\n'; exit 0");
        harness.start_ok("cycle", &command)?;

        let read = harness.read_until("cycle", |body| {
            body["data"]["state"] == "exited"
                && body["data"]["screen"]
                    .as_str()
                    .is_some_and(|screen| screen.contains(&marker))
        })?;
        assert_eq!(read["data"]["exit_code"], 0);
        let screen = read["data"]["screen"].as_str().unwrap_or_default();
        assert_eq!(screen.trim(), marker);

        harness.run_ok(&["stop", "cycle"])?;
        for session in harness.sessions() {
            assert!(!session_is_live(&harness.socket_dir, &session)?);
        }
    }
    Ok(())
}
