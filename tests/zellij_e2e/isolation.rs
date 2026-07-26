use serde_json::Value;
use tempfile::TempDir;

use super::harness::{Harness, session_is_live, socket_guard};

const INTERACTIVE_READER: &str = "printf 'ready\n'; IFS= read -r line; printf 'accepted=<%s>\n' \"$line\"; while :; do sleep 1; done";
const LOOP: &str = "while :; do sleep 1; done";

fn screen(body: &Value) -> &str {
    body["data"]["screen"].as_str().unwrap_or_default()
}

#[test]
fn same_name_when_projects_differ() -> Result<(), Box<dyn std::error::Error>> {
    let shared = TempDir::new()?;
    let state_root = shared.path().join("state");
    let socket_dir = shared.path().join("socket");
    let first = Harness::with_state_root(Some(&state_root), Some(&socket_dir))?;
    let _guard = socket_guard(&socket_dir);
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

#[test]
fn input_to_one_job_never_reaches_sibling_pane() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("first-reader", INTERACTIVE_READER)?;
    harness.start_ok("second-reader", INTERACTIVE_READER)?;
    harness.read_until("first-reader", |body| screen(body).contains("ready"))?;
    harness.read_until("second-reader", |body| screen(body).contains("ready"))?;

    harness.run_ok(&["send", "first-reader", "--", "first-pane-token"])?;
    harness.run_ok(&["send", "second-reader", "--", "second-pane-token"])?;

    let first = harness.read_until("first-reader", |body| {
        screen(body).contains("accepted=<first-pane-token>")
    })?;
    let second = harness.read_until("second-reader", |body| {
        screen(body).contains("accepted=<second-pane-token>")
    })?;
    assert!(screen(&first).contains("first-pane-token"));
    assert!(!screen(&first).contains("second-pane-token"));
    assert!(screen(&second).contains("second-pane-token"));
    assert!(!screen(&second).contains("first-pane-token"));

    harness.run_ok(&["stop", "first-reader", "--force"])?;
    harness.run_ok(&["stop", "second-reader", "--force"])?;
    Ok(())
}

#[test]
fn same_named_interactive_jobs_are_isolated_across_projects()
-> Result<(), Box<dyn std::error::Error>> {
    let shared = TempDir::new()?;
    let state_root = shared.path().join("state");
    let socket_dir = shared.path().join("socket");
    let first = Harness::with_state_root(Some(&state_root), Some(&socket_dir))?;
    let _guard = socket_guard(&socket_dir);
    let second = Harness::with_state_root(Some(&state_root), Some(&socket_dir))?;
    first.start_ok("reader", INTERACTIVE_READER)?;
    second.start_ok("reader", INTERACTIVE_READER)?;
    first.read_until("reader", |body| screen(body).contains("ready"))?;
    second.read_until("reader", |body| screen(body).contains("ready"))?;
    let first_project = first.project.to_string_lossy();
    let second_project = second.project.to_string_lossy();

    let first_send = first.run_retrying_lock(&[
        "--project",
        first_project.as_ref(),
        "send",
        "reader",
        "--",
        "first-project-token",
    ])?;
    let first_send_body: Value = serde_json::from_slice(&first_send.stdout)?;
    assert!(first_send.status.success(), "{first_send_body}");
    let second_send = second.run_retrying_lock(&[
        "--project",
        second_project.as_ref(),
        "send",
        "reader",
        "--",
        "second-project-token",
    ])?;
    let second_send_body: Value = serde_json::from_slice(&second_send.stdout)?;
    assert!(second_send.status.success(), "{second_send_body}");

    let first_read = first.read_until("reader", |body| {
        screen(body).contains("accepted=<first-project-token>")
    })?;
    let second_read = second.read_until("reader", |body| {
        screen(body).contains("accepted=<second-project-token>")
    })?;
    assert!(screen(&first_read).contains("first-project-token"));
    assert!(!screen(&first_read).contains("second-project-token"));
    assert!(screen(&second_read).contains("second-project-token"));
    assert!(!screen(&second_read).contains("first-project-token"));

    first.run_ok(&["stop", "reader", "--force"])?;
    second.run_ok(&["stop", "reader", "--force"])?;
    Ok(())
}

#[test]
fn many_jobs_share_one_session_and_survive_arbitrary_stop_order()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    let jobs = ["a", "b", "c", "d", "e", "f", "g", "h"];
    for job in jobs {
        harness.start_ok(job, LOOP)?;
    }
    let sessions = harness.sessions();
    assert_eq!(sessions.len(), 1, "sessions={sessions:?}");
    let session = &sessions[0];
    let stop_order = ["d", "a", "h", "c", "f", "b", "g", "e"];

    for (index, job) in stop_order.iter().enumerate() {
        harness.run_ok(&["stop", *job, "--force"])?;
        for remaining in &stop_order[index + 1..] {
            let read = harness.run_ok(&["read", *remaining])?;
            assert_eq!(read["data"]["state"], "running", "job={remaining}");
        }
        assert_eq!(
            session_is_live(&harness.socket_dir, session)?,
            index + 1 < stop_order.len(),
            "session={session}, stopped={job}"
        );
    }
    Ok(())
}

#[test]
fn mixed_running_and_exited_siblings_can_be_cleaned_independently()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("runner-a", LOOP)?;
    harness.start_ok("runner-b", LOOP)?;
    harness.start_ok("exit-a", "printf 'exit-a-finished\n'; exit 11")?;
    harness.start_ok("exit-b", "printf 'exit-b-finished\n'; exit 12")?;
    let jobs = [
        ("runner-a", "running"),
        ("runner-b", "running"),
        ("exit-a", "exited"),
        ("exit-b", "exited"),
    ];
    for (job, state) in jobs {
        harness.read_until(job, |body| body["data"]["state"] == state)?;
    }
    let sessions = harness.sessions();
    assert_eq!(sessions.len(), 1, "sessions={sessions:?}");
    let session = &sessions[0];
    let cleanup_order = ["exit-a", "runner-b", "exit-b", "runner-a"];

    for (index, job) in cleanup_order.iter().enumerate() {
        let stopped = harness.run_ok(&["stop", *job, "--force"])?;
        assert_eq!(stopped["data"]["cleaned_up"], true);
        for (sibling, expected_state) in jobs {
            if cleanup_order[..=index].contains(&sibling) {
                continue;
            }
            let read = harness.run_ok(&["read", sibling])?;
            assert_eq!(
                read["data"]["state"], expected_state,
                "stopped={job}, sibling={sibling}"
            );
        }
        assert_eq!(
            session_is_live(&harness.socket_dir, session)?,
            index + 1 < cleanup_order.len(),
            "session={session}, stopped={job}"
        );
    }
    Ok(())
}
