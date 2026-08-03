use std::{fs, process::Output};

use serde_json::Value;
use tempfile::TempDir;

use super::harness::{Harness, session_is_live};

const LOOP: &str = "while :; do sleep 1; done";

struct TrackedSession<'a> {
    harness: &'a Harness,
    name: String,
}

impl Drop for TrackedSession<'_> {
    fn drop(&mut self) {
        let _result = self.harness.zellij(&["kill-session", self.name.as_str()]);
    }
}

fn assert_exclusive_refusal(output: &Output) -> Result<(), Box<dyn std::error::Error>> {
    let body: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(output.status.code(), Some(2), "{body}");
    assert_eq!(body["code"], "zellij_failed");
    Ok(())
}

fn job_has_state(list: &Value, job: &str, state: &str) -> bool {
    list["jobs"].as_array().is_some_and(|jobs| {
        jobs.iter()
            .any(|item| item["job"] == job && item["state"] == state)
    })
}

#[test]
fn external_pane_close_removes_stale_job() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("closed", LOOP)?;
    let session = harness.sessions().pop().ok_or("job has no session")?;

    let pane_id = harness.pane_id("agent-terminal:closed:")?;
    harness.close_pane_externally(&pane_id)?;

    let read = harness.run(&["read", "closed"])?;
    let read_body: Value = serde_json::from_slice(&read.stdout)?;
    assert_eq!(read.status.code(), Some(1));
    assert_eq!(read_body["code"], "job_not_found");
    assert_eq!(harness.run_ok(&["list"])?["jobs"], serde_json::json!([]));
    assert!(!session_is_live(&harness.socket_dir, &session)?);
    Ok(())
}

#[test]
fn external_pane_rename_is_not_adopted_as_owned() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("renamed", LOOP)?;
    let session = harness.sessions().pop().ok_or("job has no session")?;
    let pane_id = harness.pane_id("agent-terminal:renamed:")?;

    harness.rename_pane_externally(&pane_id, "foreign-title")?;

    assert_exclusive_refusal(&harness.run(&["read", "renamed"])?)?;
    assert!(session_is_live(&harness.socket_dir, &session)?);
    harness.close_pane_externally(&pane_id)?;
    assert_eq!(harness.run_ok(&["list"])?["jobs"], serde_json::json!([]));
    assert!(!session_is_live(&harness.socket_dir, &session)?);
    Ok(())
}

#[test]
fn list_reconciles_external_session_kill_by_removing_stale_job()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("killed", LOOP)?;

    harness.kill_session_externally()?;

    let list = harness.run_ok(&["list"])?;
    assert_eq!(list["jobs"], serde_json::json!([]));
    Ok(())
}

#[test]
fn starting_second_job_recreates_killed_session_without_adopting_old_job()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("old", LOOP)?;
    harness.kill_session_externally()?;

    harness.start_ok("new", LOOP)?;

    let list = harness.run_ok(&["list"])?;
    assert!(job_has_state(&list, "new", "running"));
    harness.run_ok(&["stop", "new"])?;
    Ok(())
}

#[test]
fn foreign_pane_blocks_last_job_session_deletion() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("owned", LOOP)?;
    let session = harness.sessions().pop().ok_or("job has no session")?;
    harness.run_pane_externally("foreign-pane", &harness.project, &["--", "sh", "-c", LOOP])?;
    let foreign_id = harness.pane_id("foreign-pane")?;

    assert_exclusive_refusal(&harness.run(&["stop", "owned"])?)?;
    assert!(session_is_live(&harness.socket_dir, &session)?);
    harness.close_pane_externally(&foreign_id)?;
    harness.run_ok(&["stop", "owned"])?;
    assert!(!session_is_live(&harness.socket_dir, &session)?);
    Ok(())
}

#[test]
fn missing_keeper_blocks_last_job_session_deletion() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("keeperless", LOOP)?;
    let keeper_id = harness.pane_id("agent-terminal:keeper:")?;
    harness.close_pane_externally(&keeper_id)?;

    assert_exclusive_refusal(&harness.run(&["stop", "keeperless"])?)?;
    harness.kill_session_externally()?;
    harness.run_ok(&["stop", "keeperless"])?;
    assert_eq!(harness.run_ok(&["list"])?["jobs"], serde_json::json!([]));
    Ok(())
}

#[test]
fn externally_closed_job_does_not_break_sibling_job() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("a", LOOP)?;
    harness.start_ok("b", LOOP)?;
    let session = harness.sessions().pop().ok_or("jobs have no session")?;
    let pane_a = harness.pane_id("agent-terminal:a:")?;
    harness.close_pane_externally(&pane_a)?;

    let read_a = harness.run(&["read", "a"])?;
    let read_a_body: Value = serde_json::from_slice(&read_a.stdout)?;
    assert_eq!(read_a.status.code(), Some(1));
    assert_eq!(read_a_body["code"], "job_not_found");
    assert_eq!(harness.run_ok(&["read", "b"])?["state"], "running");
    assert!(session_is_live(&harness.socket_dir, &session)?);
    harness.run_ok(&["stop", "b"])?;
    assert!(!session_is_live(&harness.socket_dir, &session)?);
    Ok(())
}

#[test]
fn deleted_state_does_not_make_orphan_session_untrackable_by_test_cleanup()
-> Result<(), Box<dyn std::error::Error>> {
    let shared = TempDir::new()?;
    let state_root = shared.path().join("state");
    let harness = Harness::with_state_root(Some(&state_root))?;
    harness.start_ok("orphan", LOOP)?;
    let session = harness.sessions().pop().ok_or("job has no session")?;
    let cleanup = TrackedSession {
        harness: &harness,
        name: session.clone(),
    };
    let project_dirs = fs::read_dir(state_root.join("scopes"))?
        .filter_map(Result::ok)
        .flat_map(|scope| {
            fs::read_dir(scope.path().join("projects"))
                .into_iter()
                .flatten()
        })
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    let [project_dir] = project_dirs.as_slice() else {
        return Err(format!(
            "expected one project state directory, found {}",
            project_dirs.len()
        )
        .into());
    };

    fs::remove_dir_all(project_dir.path())?;
    assert_eq!(harness.run_ok(&["list"])?["jobs"], serde_json::json!([]));
    drop(cleanup);

    assert!(!session_is_live(&harness.socket_dir, &session)?);
    Ok(())
}
