use serde_json::Value;
use tempfile::TempDir;

use super::harness::{Harness, serial_guard};

#[test]
fn same_name_when_projects_differ() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = serial_guard();
    let shared = TempDir::new()?;
    let state_root = shared.path().join("state");
    let socket_dir = shared.path().join("socket");
    let first = Harness::with_state_root(Some(&state_root), Some(&socket_dir))?;
    let second = Harness::with_state_root(Some(&state_root), Some(&socket_dir))?;
    assert!(first.start("server", "sleep 30")?.status.success());
    assert!(second.start("server", "sleep 30")?.status.success());

    let first_sessions = first.sessions();
    let second_sessions = second.sessions();
    assert_eq!(first_sessions.len(), 1);
    assert_eq!(second_sessions.len(), 1);
    assert_ne!(first_sessions, second_sessions);
    assert!(first.run(&["stop", "server", "--force"])?.status.success());
    assert!(second.run(&["stop", "server", "--force"])?.status.success());
    Ok(())
}

#[test]
fn multiple_jobs_share_session() -> Result<(), Box<dyn std::error::Error>> {
    let _lock = serial_guard();
    let harness = Harness::new()?;
    assert!(
        harness
            .start("a", "while :; do sleep 1; done")?
            .status
            .success()
    );
    assert!(
        harness
            .start("b", "while :; do sleep 1; done")?
            .status
            .success()
    );
    assert_eq!(harness.sessions().len(), 1);

    let stopped = harness.run(&["stop", "a", "--force"])?;
    let stop_body: Value = serde_json::from_slice(&stopped.stdout)?;
    assert!(stopped.status.success(), "{stop_body}");
    assert_eq!(stop_body["data"]["cleaned_up"], true);

    let read_b = harness.run(&["read", "b"])?;
    let read_b_body: Value = serde_json::from_slice(&read_b.stdout)?;
    assert!(read_b.status.success(), "{read_b_body}");
    assert_eq!(read_b_body["data"]["state"], "running");

    let stopped = harness.run(&["stop", "b", "--force"])?;
    let stop_body: Value = serde_json::from_slice(&stopped.stdout)?;
    assert!(stopped.status.success(), "{stop_body}");
    assert_eq!(stop_body["data"]["cleaned_up"], true);

    let listed = harness.run(&["list"])?;
    let list_body: Value = serde_json::from_slice(&listed.stdout)?;
    assert!(listed.status.success(), "{list_body}");
    assert_eq!(list_body["data"]["jobs"], serde_json::json!([]));
    Ok(())
}
