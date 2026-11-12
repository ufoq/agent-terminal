#![allow(clippy::disallowed_methods)]

use std::{
    fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;
use tempfile::TempDir;

const fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agent-terminal")
}

fn run(state: &Path, project: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .arg("--state-dir")
        .arg(state)
        .arg("--project")
        .arg(project)
        .env("ZELLIJ_SOCKET_DIR", state.join("sockets"))
        .args(args)
        .output()
        .unwrap()
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON: {error}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn wait_for_job(state: &Path, project: &Path, job: &str, present: bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let output = run(state, project, &["--global", "list"]);
        if !output.status.success() {
            thread::sleep(Duration::from_millis(100));
            continue;
        }
        let value = json(&output);
        assert_eq!(
            value["status"],
            "ok",
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let jobs = value["data"]["jobs"].as_array().expect("jobs array");
        let found = jobs.iter().any(|entry| entry["name"] == job);
        if found == present {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    panic!("job {job} did not reach present={present}");
}

fn zellij_available() -> bool {
    Command::new("zellij")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn global_job_lifecycle_and_cleanup() {
    if !zellij_available() {
        eprintln!("skipping: zellij is not on PATH");
        return;
    }
    let state = TempDir::new().unwrap();
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let project = TempDir::new().unwrap();
    fs::create_dir(state.path().join("sockets")).unwrap();
    let state_path = state.path();
    let project_path = project.path();

    let empty = run(state_path, project_path, &["--global", "list"]);
    assert!(empty.status.success());
    assert_eq!(json(&empty)["data"]["jobs"].as_array().unwrap().len(), 0);

    let absent = run(state_path, project_path, &["--global", "stop", "missing"]);
    assert!(
        absent.status.success(),
        "{}",
        String::from_utf8_lossy(&absent.stderr)
    );

    let start = run(
        state_path,
        project_path,
        &[
            "--global",
            "start",
            "echoer",
            "--",
            "sh",
            "-c",
            "echo ready; IFS= read -r line; echo \"got:$line\"; exec sleep 60",
        ],
    );
    assert!(
        start.status.success(),
        "{}",
        String::from_utf8_lossy(&start.stderr)
    );
    wait_for_job(state_path, project_path, "echoer", true);

    let read = run(state_path, project_path, &["--global", "read", "echoer"]);
    assert!(read.status.success());
    assert!(
        json(&read)["data"]["screen"]
            .as_str()
            .unwrap()
            .contains("ready")
    );

    let send = run(
        state_path,
        project_path,
        &["--global", "send", "echoer", "hello"],
    );
    assert!(send.status.success());

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let read = run(state_path, project_path, &["--global", "read", "echoer"]);
        if json(&read)["data"]["screen"]
            .as_str()
            .unwrap()
            .contains("got:hello")
        {
            break;
        }
        assert!(Instant::now() < deadline, "job did not receive input");
        thread::sleep(Duration::from_millis(100));
    }

    let stop = run(state_path, project_path, &["--global", "stop", "echoer"]);
    assert!(stop.status.success());
    wait_for_job(state_path, project_path, "echoer", false);
    let sessions = run(state_path, project_path, &["--global", "list"]);
    assert!(sessions.status.success());

    let lock = fs::read_dir(state_path.join("locks"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(lock.is_file(), "project lock must survive cleanup");
}

#[test]
fn job_names_reject_unsafe_values() {
    let state = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let output = run(
        state.path(),
        project.path(),
        &["--global", "start", "../escape", "--", "true"],
    );
    assert!(!output.status.success());
    assert_eq!(json(&output)["code"], "invalid_input");
}

#[test]
fn state_directory_must_be_private() {
    let parent = TempDir::new().unwrap();
    let state = parent.path().join("state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();
    let project = TempDir::new().unwrap();
    let output = run(&state, project.path(), &["--global", "list"]);
    assert!(output.status.success());
    let mode = fs::metadata(&state).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

#[test]
fn help_documents_six_commands() {
    let output = Command::new(bin()).arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    for command in ["list", "start", "read", "send", "press", "stop"] {
        assert!(help.contains(command), "missing {command} in help");
    }
}

#[test]
fn missing_pi_is_global_only() {
    let state = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let output = run(state.path(), project.path(), &["list"]);
    assert!(output.status.success() || json(&output)["code"] == "no_pi_ancestor");
}

#[test]
fn lock_is_reused_across_runs() {
    let state = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let first = run(state.path(), project.path(), &["--global", "list"]);
    assert!(first.status.success());
    let locks: Vec<PathBuf> = fs::read_dir(state.path().join("locks"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(locks.len(), 1);
    let before = fs::metadata(&locks[0]).unwrap().ino();
    let second = run(state.path(), project.path(), &["--global", "list"]);
    assert!(second.status.success());
    let after = fs::metadata(&locks[0]).unwrap().ino();
    assert_eq!(before, after);
}
#[test]
fn owner_job_survives_repeated_commands() {
    if !zellij_available() {
        eprintln!("skipping: zellij is not on PATH");
        return;
    }
    let state = TempDir::new().unwrap();
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let project = TempDir::new().unwrap();
    fs::create_dir(state.path().join("sockets")).unwrap();
    let state_path = state.path();
    let project_path = project.path();

    let probe = run(state_path, project_path, &["list"]);
    if !probe.status.success() && json(&probe)["code"] == "no_pi_ancestor" {
        eprintln!("skipping: test process has no pi ancestor");
        return;
    }

    let start = run(
        state_path,
        project_path,
        &[
            "start",
            "owner-job",
            "--",
            "sh",
            "-c",
            "echo ready; exec sleep 60",
        ],
    );
    assert!(
        start.status.success(),
        "{}",
        String::from_utf8_lossy(&start.stderr)
    );

    for _ in 0..3 {
        let list = run(state_path, project_path, &["list"]);
        assert!(list.status.success());
        let jobs = json(&list)["data"]["jobs"].as_array().unwrap().clone();
        assert_eq!(jobs.len(), 1, "owner job disappeared: {list:?}");
        assert_eq!(jobs[0]["name"], "owner-job");
    }

    let read = run(state_path, project_path, &["read", "owner-job"]);
    assert!(read.status.success());
    assert!(
        json(&read)["data"]["screen"]
            .as_str()
            .unwrap()
            .contains("ready")
    );

    let stop = run(state_path, project_path, &["stop", "owner-job"]);
    assert!(stop.status.success());
    let list = run(state_path, project_path, &["list"]);
    assert_eq!(json(&list)["data"]["jobs"].as_array().unwrap().len(), 0);
}
