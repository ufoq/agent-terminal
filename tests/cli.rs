#![allow(clippy::disallowed_methods)]

use std::{
    fmt::Write as _,
    fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;
use sha2::{Digest as _, Sha256};
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
        .env("RUST_LOG", "warn")
        .args(args)
        .output()
        .unwrap()
}

fn run_env(
    state: &Path,
    project: &Path,
    args: &[&str],
    envs: &[(&str, &std::ffi::OsStr)],
) -> Output {
    let mut command = Command::new(bin());
    command
        .arg("--state-dir")
        .arg(state)
        .arg("--project")
        .arg(project)
        .env("ZELLIJ_SOCKET_DIR", state.join("sockets"))
        .env("RUST_LOG", "warn")
        .args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().unwrap()
}

fn zellij_bin() -> PathBuf {
    let output = Command::new("sh")
        .arg("-c")
        .arg("command -v zellij")
        .output()
        .expect("sh");
    assert!(output.status.success(), "zellij is not on PATH");
    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
}

fn start_job(state: &Path, project: &Path, job: &str) {
    let start = run(
        state,
        project,
        &[
            "--global",
            "start",
            job,
            "--",
            "sh",
            "-c",
            "echo ready; exec sleep 60",
        ],
    );
    assert!(
        start.status.success(),
        "start {job}: {}",
        String::from_utf8_lossy(&start.stderr)
    );
    wait_for_job(state, project, job, true);
}

fn kill_current_session(state: &Path) {
    let socket = state.join("sockets");
    let list = Command::new(zellij_bin())
        .args(["list-sessions", "--short", "--no-formatting"])
        .env("ZELLIJ_SOCKET_DIR", &socket)
        .output()
        .expect("zellij list-sessions");
    assert!(
        list.status.success(),
        "zellij list-sessions: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    let list_stdout = String::from_utf8_lossy(&list.stdout);
    let names: Vec<&str> = list_stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(names.len(), 1, "expected one session: {names:?}");
    let killed = Command::new(zellij_bin())
        .args(["kill-session", names[0]])
        .env("ZELLIJ_SOCKET_DIR", &socket)
        .output()
        .expect("zellij kill-session");
    assert!(
        killed.status.success(),
        "zellij kill-session: {}",
        String::from_utf8_lossy(&killed.stderr)
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let list = Command::new(zellij_bin())
            .args(["list-sessions", "--short", "--no-formatting"])
            .env("ZELLIJ_SOCKET_DIR", &socket)
            .output()
            .expect("zellij list-sessions");
        let stdout = String::from_utf8_lossy(&list.stdout);
        let stderr = String::from_utf8_lossy(&list.stderr);
        if list.status.success() && stdout.lines().all(|line| line.trim().is_empty()) {
            return;
        }
        if !list.status.success()
            && (stdout.contains("No active zellij sessions")
                || stderr.contains("No active zellij sessions"))
        {
            return;
        }
        assert!(
            list.status.success(),
            "zellij list-sessions failed after kill: {stderr}"
        );
        thread::sleep(Duration::from_millis(50));
    }
    panic!("zellij session did not disappear after kill");
}

fn fake_zellij(state: &Path, fail_action: &str) -> PathBuf {
    let path = state.join("fake-zellij");
    let script = format!(
        "#!/bin/sh\nif [ -n \"$AGENT_TERMINAL_FAKE_LOG\" ]; then printf '%s\\n' \"$*\" >> \"$AGENT_TERMINAL_FAKE_LOG\"; fi\nif [ \"$3\" = \"action\" ] && [ \"$4\" = \"{fail_action}\" ]; then\n  printf '%s\\n' \"$AGENT_TERMINAL_FAKE_SECRET\" >&2\n  exit 1\nfi\nexec \"{}\" \"$@\"\n",
        zellij_bin().display()
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn fake_zellij_bad_json(state: &Path, action: &str) -> PathBuf {
    let path = state.join("fake-zellij-bad-json");
    let script = format!(
        "#!/bin/sh\nif [ -n \"$AGENT_TERMINAL_FAKE_LOG\" ]; then printf '%s\\n' \"$*\" >> \"$AGENT_TERMINAL_FAKE_LOG\"; fi\nif [ \"$1\" = \"list-sessions\" ] || [ \"$3\" = \"list-sessions\" ]; then\n  printf '%s\\n' \"$AGENT_TERMINAL_FAKE_SESSION\"\n  exit 0\nfi\nif {{ [ \"$3\" = \"action\" ] && [ \"$4\" = \"{action}\" ]; }} || {{ [ \"$1\" = \"action\" ] && [ \"$2\" = \"{action}\" ]; }}; then\n  printf '[{{\"id\":\"%s\",\"is_plugin\":false,\"title\":\"agent-terminal:job:x\",\"exited\":false}}]\\n' \"$AGENT_TERMINAL_FAKE_SECRET\"\n  printf '%s\\n' \"$AGENT_TERMINAL_FAKE_SECRET\" >&2\n  exit 0\nfi\nexit 2\n"
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn project_digest(project: &Path) -> String {
    let project = project.canonicalize().unwrap();
    let mut hasher = Sha256::new();
    for part in [
        b"project\0".as_slice(),
        project.as_os_str().as_encoded_bytes(),
    ] {
        hasher.update(part);
        hasher.update([0]);
    }
    let hash = hasher.finalize();
    hash[..32].iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn create_global_project_state(state: &Path, project: &Path) -> String {
    let digest = project_digest(project);
    let dir = state
        .join("scopes")
        .join("global")
        .join("projects")
        .join(&digest);
    fs::create_dir_all(dir).unwrap();
    format!("at-global-{}", &digest[..12])
}

fn fake_zellij_list_sessions(state: &Path, stderr: &str, code: i32) -> PathBuf {
    let path = state.join(format!("fake-zellij-list-{code}"));
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"list-sessions\" ] || [ \"$3\" = \"list-sessions\" ]; then\n  printf '%s\\n' \"{stderr}\" >&2\n  exit {code}\nfi\nexit 2\n"
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
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

#[test]
fn owner_session_loss_is_diagnosed_and_not_durable() {
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
        &["start", "owner-lost", "--", "sh", "-c", "exec sleep 60"],
    );
    assert!(
        start.status.success(),
        "{}",
        String::from_utf8_lossy(&start.stderr)
    );
    kill_current_session(state_path);

    let read = run(state_path, project_path, &["read", "owner-lost"]);
    assert_eq!(json(&read)["code"], "session_lost");
    let repeat = run(state_path, project_path, &["read", "owner-lost"]);
    assert_eq!(json(&repeat)["code"], "job_not_found");
    let stop = run(state_path, project_path, &["stop", "owner-lost"]);
    assert!(stop.status.success());
}

#[test]
fn session_loss_is_observable_and_not_durable() {
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

    let fresh = run(state_path, project_path, &["--global", "list"]);
    assert!(fresh.status.success());
    assert!(json(&fresh)["data"].get("session").is_none());
    assert!(json(&fresh)["data"]["jobs"].as_array().unwrap().is_empty());

    start_job(state_path, project_path, "lost-list");
    kill_current_session(state_path);
    let list = run(state_path, project_path, &["--global", "list"]);
    assert!(list.status.success());
    assert_eq!(json(&list)["data"]["session"], "lost");
    assert!(json(&list)["data"]["jobs"].as_array().unwrap().is_empty());

    let read = run(state_path, project_path, &["--global", "read", "lost-list"]);
    assert!(!read.status.success());
    assert_eq!(json(&read)["code"], "session_lost");

    let next = run(state_path, project_path, &["--global", "list"]);
    assert!(next.status.success());
    assert!(json(&next)["data"].get("session").is_none());
}

#[test]
fn session_loss_replaces_job_not_found_for_read_send_and_press() {
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

    for (job, args) in [
        ("lost-read", vec!["--global", "read", "lost-read"]),
        ("lost-send", vec!["--global", "send", "lost-send", "hello"]),
        (
            "lost-press",
            vec!["--global", "press", "lost-press", "Enter"],
        ),
    ] {
        start_job(state_path, project_path, job);
        kill_current_session(state_path);
        let output = run(state_path, project_path, &args);
        assert!(!output.status.success(), "{job} unexpectedly succeeded");
        assert_eq!(json(&output)["code"], "session_lost", "{job}");
        let repeat = run(state_path, project_path, &args);
        assert_eq!(json(&repeat)["code"], "job_not_found", "{job} repeat");
    }
}

#[test]
fn stop_and_start_recover_after_session_loss() {
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

    start_job(state_path, project_path, "lost-stop");
    kill_current_session(state_path);
    let stop = run(state_path, project_path, &["--global", "stop", "lost-stop"]);
    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stderr)
    );

    start_job(state_path, project_path, "lost-restart");
    kill_current_session(state_path);
    let restart = run(
        state_path,
        project_path,
        &["--global", "start", "lost-restart", "--", "sleep", "60"],
    );
    assert!(
        restart.status.success(),
        "{}",
        String::from_utf8_lossy(&restart.stderr)
    );
    let stop = run(
        state_path,
        project_path,
        &["--global", "stop", "lost-restart"],
    );
    assert!(stop.status.success());
}

#[test]
fn live_session_wrong_job_is_job_not_found() {
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

    let first = run(state_path, project_path, &["--global", "list"]);
    assert!(first.status.success());
    assert!(json(&first)["data"].get("session").is_none());
    assert!(json(&first)["data"]["jobs"].as_array().unwrap().is_empty());

    start_job(state_path, project_path, "live-job");
    let list = run(state_path, project_path, &["--global", "list"]);
    assert!(json(&list)["data"].get("session").is_none());
    let read = run(state_path, project_path, &["--global", "read", "missing"]);
    assert_eq!(json(&read)["code"], "job_not_found");
    let stop = run(state_path, project_path, &["--global", "stop", "live-job"]);
    assert!(stop.status.success());
}

#[test]
fn version_matches_release_json() {
    let release = serde_json::from_str::<Value>(
        &fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/release.json")).unwrap(),
    )
    .unwrap();
    let output = Command::new(bin()).arg("--version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("agent-terminal {}", release["version"].as_str().unwrap())
    );
}

#[test]
fn status_is_invalid_without_a_mutating_suggestion() {
    let output = Command::new(bin()).arg("status").output().unwrap();
    assert!(!output.status.success());
    let response = json(&output);
    assert_eq!(response["code"], "invalid_input");
    let message = response["message"].as_str().unwrap();
    assert!(message.contains("status"));
    assert!(!message.contains("start"), "misleading tip: {message}");
}

#[test]
fn invalid_pane_json_does_not_log_the_backend_output() {
    let state = TempDir::new().unwrap();
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let project = TempDir::new().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    let state_path = state.path();

    let session = create_global_project_state(state_path, &project_path);

    let secret = std::ffi::OsStr::new("UNIQUE_JSON_SECRET_2b7d");
    let fake = fake_zellij_bad_json(state_path, "list-panes");
    let fake_os = fake.as_os_str();
    let log = state_path.join("parser-calls.log");
    let output = run_env(
        state_path,
        &project_path,
        &["--global", "list"],
        &[
            ("AGENT_TERMINAL_ZELLIJ", fake_os),
            ("AGENT_TERMINAL_FAKE_SECRET", secret),
            ("AGENT_TERMINAL_FAKE_SESSION", session.as_ref()),
            ("AGENT_TERMINAL_FAKE_LOG", log.as_os_str()),
        ],
    );
    assert_eq!(json(&output)["code"], "zellij_failed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid list-panes JSON"), "{stderr}");
    assert!(
        !stderr.contains("UNIQUE_JSON_SECRET_2b7d"),
        "leaked: {stderr}"
    );
    let calls = fs::read_to_string(&log).unwrap();
    assert!(calls.contains("action list-panes"), "{calls}");
}

#[test]
fn keeper_schema_error_does_not_log_the_backend_output() {
    let state = TempDir::new().unwrap();
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let fake = fake_zellij_bad_json(state.path(), "list-panes");
    let log = state.path().join("keeper-calls.log");
    let mut child = Command::new(bin())
        .args(["__keeper", "-", "-"])
        .arg(&fake)
        .arg(state.path().join("lock"))
        .env("ZELLIJ_SESSION_NAME", "at-test")
        .env("RUST_LOG", "warn")
        .env("AGENT_TERMINAL_FAKE_SECRET", "UNIQUE_JSON_SECRET_2b7d")
        .env("AGENT_TERMINAL_FAKE_LOG", &log)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_millis(500));
    child.kill().unwrap();
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("keeper saw invalid pane JSON"), "{stderr}");
    assert!(
        !stderr.contains("UNIQUE_JSON_SECRET_2b7d"),
        "leaked: {stderr}"
    );
    let calls = fs::read_to_string(&log).unwrap();
    assert!(calls.contains("action list-panes"), "{calls}");
}

#[test]
fn backend_failures_do_not_log_arguments_or_stderr() {
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

    start_job(state_path, project_path, "secret-send");
    let secret = std::ffi::OsStr::new("UNIQUE_SEND_SECRET_7f3a");
    let fake = fake_zellij(state_path, "paste");
    let fake_os = fake.as_os_str();
    let send = run_env(
        state_path,
        project_path,
        &["--global", "send", "secret-send", "UNIQUE_SEND_SECRET_7f3a"],
        &[
            ("AGENT_TERMINAL_ZELLIJ", fake_os),
            ("AGENT_TERMINAL_FAKE_SECRET", secret),
        ],
    );
    assert_eq!(json(&send)["code"], "zellij_failed");
    let stderr = String::from_utf8_lossy(&send.stderr);
    assert!(
        !stderr.contains("UNIQUE_SEND_SECRET_7f3a"),
        "leaked: {stderr}"
    );
    let stop = run(
        state_path,
        project_path,
        &["--global", "stop", "secret-send"],
    );
    assert!(stop.status.success());

    let fake = fake_zellij(state_path, "new-pane");
    let fake_os = fake.as_os_str();
    let start = run_env(
        state_path,
        project_path,
        &[
            "--global",
            "start",
            "secret-start",
            "--",
            "sh",
            "-c",
            "echo UNIQUE_COMMAND_SECRET_91c2",
        ],
        &[
            ("AGENT_TERMINAL_ZELLIJ", fake_os),
            ("AGENT_TERMINAL_FAKE_SECRET", secret),
        ],
    );
    assert_eq!(json(&start)["code"], "zellij_failed");
    let stderr = String::from_utf8_lossy(&start.stderr);
    assert!(
        !stderr.contains("UNIQUE_COMMAND_SECRET_91c2"),
        "leaked: {stderr}"
    );
    assert!(
        !stderr.contains("UNIQUE_SEND_SECRET_7f3a"),
        "leaked: {stderr}"
    );
}

#[test]
fn send_paste_success_enter_failure_is_not_replayed() {
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
    start_job(state_path, project_path, "partial-send");

    let fake = fake_zellij(state_path, "send-keys");
    let log = state_path.join("calls.log");
    let output = run_env(
        state_path,
        project_path,
        &["--global", "send", "partial-send", "hello"],
        &[
            ("AGENT_TERMINAL_ZELLIJ", fake.as_os_str()),
            ("AGENT_TERMINAL_FAKE_LOG", log.as_os_str()),
        ],
    );
    assert_eq!(json(&output)["code"], "zellij_failed");
    let calls = fs::read_to_string(&log).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.contains("action paste"))
            .count(),
        1,
        "paste was replayed: {calls}"
    );
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.contains("action send-keys"))
            .count(),
        1,
        "enter was replayed: {calls}"
    );

    let stop = run(
        state_path,
        project_path,
        &["--global", "stop", "partial-send"],
    );
    assert!(stop.status.success());
}

#[test]
fn press_has_one_mutation() {
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
    start_job(state_path, project_path, "one-press");

    let fake = fake_zellij(state_path, "__never__");
    let log = state_path.join("calls.log");
    let output = run_env(
        state_path,
        project_path,
        &["--global", "press", "one-press", "Enter"],
        &[
            ("AGENT_TERMINAL_ZELLIJ", fake.as_os_str()),
            ("AGENT_TERMINAL_FAKE_LOG", log.as_os_str()),
        ],
    );
    assert!(output.status.success());
    let calls = fs::read_to_string(&log).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.contains("action send-keys"))
            .count(),
        1,
        "press mutated more than once: {calls}"
    );
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.contains("action paste"))
            .count(),
        0,
        "press used paste: {calls}"
    );

    let stop = run(state_path, project_path, &["--global", "stop", "one-press"]);
    assert!(stop.status.success());
}

#[test]
fn probe_errors_are_not_absence() {
    let state = TempDir::new().unwrap();
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let project = TempDir::new().unwrap();
    let state_path = state.path();
    let project_path = project.path();
    let session = create_global_project_state(state_path, project_path);

    let broken = fake_zellij_list_sessions(state_path, "backend exploded", 7);
    let output = run_env(
        state_path,
        project_path,
        &["--global", "list"],
        &[
            ("AGENT_TERMINAL_ZELLIJ", broken.as_os_str()),
            ("AGENT_TERMINAL_FAKE_SESSION", session.as_ref()),
        ],
    );
    assert_eq!(json(&output)["code"], "zellij_failed");
    assert_ne!(json(&output)["code"], "session_lost");

    let absent = fake_zellij_list_sessions(state_path, "No active zellij sessions", 1);
    let output = run_env(
        state_path,
        project_path,
        &["--global", "list"],
        &[
            ("AGENT_TERMINAL_ZELLIJ", absent.as_os_str()),
            ("AGENT_TERMINAL_FAKE_SESSION", session.as_ref()),
        ],
    );
    assert!(output.status.success());
    assert_eq!(json(&output)["data"]["session"], "lost");
}

#[test]
#[allow(clippy::too_many_lines)]
fn release_script_is_idempotent_and_rejects_ambiguous_cargo() {
    let node = Command::new("node").arg("--version").output();
    if node.is_err() {
        eprintln!("skipping: node is not on PATH");
        return;
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = TempDir::new().unwrap();
    let root = fixture.path();
    for path in [
        "scripts/release-version.mjs",
        "release.json",
        "Cargo.toml",
        "pi/npm/packages/pi-agent-terminal/package.json",
        "pi/npm/packages/pi-agent-terminal-bundle-zellij/package.json",
    ] {
        let destination = root.join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(source.join(path), destination).unwrap();
    }
    let script = root.join("scripts/release-version.mjs");
    let run_release = |args: &[&str]| {
        Command::new("node")
            .arg(&script)
            .args(args)
            .current_dir(root)
            .output()
            .unwrap()
    };
    let snapshot = || {
        [
            "release.json",
            "Cargo.toml",
            "pi/npm/packages/pi-agent-terminal/package.json",
            "pi/npm/packages/pi-agent-terminal-bundle-zellij/package.json",
        ]
        .map(|path| (path, fs::read(root.join(path)).unwrap()))
    };

    let first = run_release(&["sync", "0.1.6"]);
    assert!(
        first.status.success(),
        "same-version sync: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let before_second = snapshot();
    let second = run_release(&["sync", "0.1.6"]);
    assert!(
        second.status.success(),
        "same-version sync: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        snapshot(),
        before_second,
        "same-version sync was not idempotent"
    );

    let output = run_release(&["sync", "0.1.7"]);
    assert!(
        output.status.success(),
        "drift sync: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fs::read_to_string(root.join("release.json"))
            .unwrap()
            .contains("\"0.1.7\"")
    );
    let cargo_after = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(cargo_after.contains("version = \"0.1.7\""));
    assert!(cargo_after.contains("sha2 = \"0.10\""));
    assert!(
        run_release(&["check"]).status.success(),
        "check failed after sync"
    );
    fs::write(
        root.join("release.json"),
        "{\n  \"version\": \"0.1.5\"\n}\n",
    )
    .unwrap();
    assert!(
        !run_release(&["check"]).status.success(),
        "check accepted drift"
    );

    for path in [
        "release.json",
        "Cargo.toml",
        "pi/npm/packages/pi-agent-terminal/package.json",
        "pi/npm/packages/pi-agent-terminal-bundle-zellij/package.json",
    ] {
        fs::copy(source.join(path), root.join(path)).unwrap();
    }
    let cargo_path = root.join("Cargo.toml");
    let canonical = fs::read_to_string(&cargo_path).unwrap();
    fs::write(
        &cargo_path,
        canonical.replace(
            "version = \"0.1.6\"",
            "version = \"0.1.6\"\nversion = '0.9.9'",
        ),
    )
    .unwrap();
    let before = snapshot();
    let output = run_release(&["sync", "0.1.7"]);
    assert!(!output.status.success(), "duplicate version was accepted");
    assert_eq!(snapshot(), before, "duplicate version caused writes");

    fs::write(
        &cargo_path,
        canonical.replace("version = \"0.1.6\"\n", "")
            + "\n [dependencies.serde]\nversion = \"1\"\n",
    )
    .unwrap();
    let before = snapshot();
    let output = run_release(&["sync", "0.1.7"]);
    assert!(!output.status.success(), "dependency version was accepted");
    assert_eq!(snapshot(), before, "dependency version caused writes");

    fs::write(
        &cargo_path,
        canonical.replace("name = \"agent-terminal\"", "name = \"other\""),
    )
    .unwrap();
    let before = snapshot();
    let output = run_release(&["sync", "0.1.7"]);
    assert!(!output.status.success(), "wrong package name was accepted");
    assert_eq!(snapshot(), before, "wrong package name caused writes");

    let malformed = [
        format!("{canonical}\n [package]\nname = \"other\"\nversion = \"9.9.9\"\n"),
        canonical.replace("[package]", "[package] # comment"),
        canonical.replace(
            "version = \"0.1.6\"",
            "version = \"0.1.6\"\n\"version\" = \"9.9.9\"",
        ),
        format!("{canonical}\n[\"package\"]\nversion = \"9.9.9\"\n"),
        format!("{canonical}\n['package']\nversion = \"9.9.9\"\n"),
        format!(
            "{canonical}\ndescription = \"\"\"\n[package]\nname = \"agent-terminal\"\nversion = \"0.1.6\"\n\"\"\"\n"
        ),
        format!(
            "{canonical}\ndescription = \"\"\"\nname = \"agent-terminal\"\nversion = \"0.1.6\"\n[dependencies]\n\"\"\"\n"
        ),
        format!("{canonical}\n[\"pack\\u0061ge\"]\nversion = \"9.9.9\"\n"),
        format!("{canonical}\n\n[[package]]\nname = \"other\"\nversion = \"9.9.9\"\n"),
        format!("{canonical}\n[package\nname = \"other\"\nversion = \"9.9.9\"\n"),
        canonical.replace("[dependencies]", "[dependencies]\u{000b}"),
        canonical.replace("[dependencies]", "\u{00a0}[dependencies]"),
        canonical.replace("[package]", "#\u{2028}[package]"),
        format!("{canonical}\n[package.version]\nvalue = \"9.9.9\"\n"),
        format!("{canonical}\n[package.name]\nvalue = \"other\"\n"),
        format!("{canonical}\npackage = {{ version = \"9.9.9\" }}\n"),
        format!("{canonical}\npackage.version = \"9.9.9\"\n"),
        canonical.replace(
            "version = \"0.1.6\"",
            "version = \"0.1.6\"\n\"vers\\u0069on\" = \"9.9.9\"",
        ),
    ];
    for cargo in malformed {
        fs::write(&cargo_path, cargo).unwrap();
        let before = snapshot();
        let output = run_release(&["sync", "0.1.7"]);
        assert!(
            !output.status.success(),
            "ambiguous package structure was accepted"
        );
        assert_eq!(
            snapshot(),
            before,
            "ambiguous package structure caused writes"
        );
    }
}
