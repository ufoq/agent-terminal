use super::harness::{Harness, session_is_live};

#[test]
fn running_job_name_can_be_reused_after_stop() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("reuse", "printf 'first\n'; while :; do sleep 1; done")?;
    harness.read_until("reuse", |body| body["state"] == "running")?;

    let stopped = harness.run(&["stop", "reuse"])?;
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stdout)
    );

    harness.start_ok("reuse", "printf 'second\n'; exit 0")?;
    let second = harness.read_until("reuse", |body| {
        body["state"] == "exited"
            && body["screen"]
                .as_str()
                .is_some_and(|screen| screen.contains("second"))
    })?;
    assert_eq!(second["exit_code"], 0);
    let screen = second["screen"].as_str().unwrap_or_default();
    assert_eq!(screen.trim(), "second");
    harness.run_ok(&["stop", "reuse"])?;
    Ok(())
}

#[test]
fn exited_job_name_can_be_reused_after_cleanup() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("reuse", "exit 3")?;
    let first = harness.read_until("reuse", |body| body["state"] == "exited")?;
    assert_eq!(first["exit_code"], 3);
    harness.run_ok(&["stop", "reuse"])?;

    harness.start_ok("reuse", "exit 7")?;
    let second = harness.read_until("reuse", |body| body["state"] == "exited")?;
    assert_eq!(second["exit_code"], 7);
    harness.run_ok(&["stop", "reuse"])?;
    Ok(())
}

#[test]
fn externally_closed_job_name_can_be_reused_after_reconciliation()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("reuse", "while :; do sleep 1; done")?;
    harness.read_until("reuse", |body| body["state"] == "running")?;

    harness.kill_session_externally()?;
    let stale = harness.run(&["read", "reuse"])?;
    let stale_body: serde_json::Value = serde_json::from_slice(&stale.stdout)?;
    assert_eq!(stale.status.code(), Some(1));
    assert_eq!(stale_body["code"], "job_not_found");

    harness.start_ok("reuse", "printf 'restarted\n'; while :; do sleep 1; done")?;
    let restarted = harness.read_until("reuse", |body| {
        body["state"] == "running"
            && body["screen"]
                .as_str()
                .is_some_and(|screen| screen.contains("restarted"))
    })?;
    assert_eq!(restarted["state"], "running");
    harness.run_ok(&["stop", "reuse"])?;
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
            body["state"] == "exited"
                && body["screen"]
                    .as_str()
                    .is_some_and(|screen| screen.contains(&marker))
        })?;
        assert_eq!(read["exit_code"], 0);
        let screen = read["screen"].as_str().unwrap_or_default();
        assert_eq!(screen.trim(), marker);

        harness.run_ok(&["stop", "cycle"])?;
        for session in harness.sessions() {
            assert!(!session_is_live(&harness.socket_dir, &session)?);
        }
    }
    Ok(())
}
