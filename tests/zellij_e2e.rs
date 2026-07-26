#![cfg(unix)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use serde_json::Value;
use tempfile::TempDir;

static E2E_LOCK: Mutex<()> = Mutex::new(());

struct Harness {
    _temp: TempDir,
    project: PathBuf,
    state_root: PathBuf,
}

impl Harness {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_state_root(None)
    }

    fn with_state_root(state_root: Option<&Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let project = temp.path().join("project");
        fs::create_dir_all(&project)?;
        Ok(Self {
            project: project.canonicalize()?,
            state_root: state_root.map_or_else(|| temp.path().join("state"), Path::to_path_buf),
            _temp: temp,
        })
    }

    fn run(&self, arguments: &[&str]) -> Result<Output, std::io::Error> {
        let mut command = Command::new(assert_cmd::cargo::cargo_bin!("agent-terminal"));
        command
            .current_dir(&self.project)
            .arg("--state-dir")
            .arg(&self.state_root)
            .args(arguments)
            .output()
    }

    fn start(&self, job: &str, shell_command: &str) -> Result<Output, std::io::Error> {
        self.run(&["start", job, "--", "sh", "-c", shell_command])
    }

    fn read_until(
        &self,
        job: &str,
        predicate: impl Fn(&Value) -> bool,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let output = self.run(&["read", job])?;
            if output.status.success() {
                let body: Value = serde_json::from_slice(&output.stdout)?;
                if predicate(&body) {
                    return Ok(body);
                }
            }
            if Instant::now() >= deadline {
                return Err(format!("timed out reading job {job:?}").into());
            }
            std::thread::yield_now();
        }
    }

    fn sessions(&self) -> Vec<String> {
        state_sessions(&self.state_root, &self.project)
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        for session in self.sessions() {
            let _result = Command::new("zellij")
                .args(["kill-session", session.as_str()])
                .output();
        }
    }
}

#[test]
fn start_read_stop_when_job_is_running() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = serial_guard();
    let harness = Harness::new()?;
    let started = harness.start(
        "server",
        "printf 'ready\\n'; trap 'exit 0' INT; while :; do sleep 1; done",
    )?;
    let start_body: Value = serde_json::from_slice(&started.stdout)?;
    assert!(started.status.success(), "{start_body}");
    assert_eq!(start_body["data"]["state"], "running");

    let read = harness.read_until("server", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("ready"))
    })?;
    assert_eq!(read["data"]["state"], "running");

    let duplicate = harness.start("server", "sleep 30")?;
    assert_eq!(duplicate.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&duplicate.stdout)?["error"]["code"],
        "job_exists"
    );

    let stopped = harness.run(&["stop", "server"])?;
    let stop_body: Value = serde_json::from_slice(&stopped.stdout)?;
    assert!(stopped.status.success(), "{stop_body}");
    assert_eq!(stop_body["data"]["cleaned_up"], true);
    assert_eq!(stop_body["data"]["forced"], false);
    for session in harness.sessions() {
        assert!(!session_is_live(&session)?);
    }
    Ok(())
}

#[test]
fn send_and_press_when_job_is_interactive() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = serial_guard();
    let harness = Harness::new()?;
    let started = harness.start(
        "prompt",
        "IFS= read -r first; printf 'text:%s\\n' \"$first\"; IFS= read -r second; printf 'key:%s\\n' \"$second\"",
    )?;
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stdout)
    );

    assert!(
        harness
            .run(&["send", "prompt", "--", "hello world"])?
            .status
            .success()
    );
    assert!(
        harness
            .run(&["press", "prompt", "--", "Enter"])?
            .status
            .success()
    );
    let read = harness.read_until("prompt", |body| {
        body["data"]["state"] == "exited"
            && body["data"]["screen"].as_str().is_some_and(|screen| {
                screen.contains("text:hello world") && screen.contains("key:")
            })
    })?;
    assert_eq!(read["data"]["exit_code"], 0);
    assert!(harness.run(&["stop", "prompt"])?.status.success());
    Ok(())
}

#[test]
fn fast_failure_when_command_exits_immediately() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = serial_guard();
    let harness = Harness::new()?;
    let started = harness.start("tests", "printf 'boom\\n'; exit 7")?;
    let start_body: Value = serde_json::from_slice(&started.stdout)?;
    assert!(started.status.success(), "{start_body}");
    assert_eq!(start_body["data"]["state"], "exited");
    assert_eq!(start_body["data"]["exit_code"], 7);

    let read = harness.read_until("tests", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("boom"))
    })?;
    assert_eq!(read["data"]["state"], "exited");
    assert_eq!(read["data"]["exit_code"], 7);
    assert!(harness.run(&["stop", "tests"])?.status.success());
    Ok(())
}

#[test]
fn same_name_when_projects_differ() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = serial_guard();
    let shared = TempDir::new()?;
    let state_root = shared.path().join("state");
    let first = Harness::with_state_root(Some(&state_root))?;
    let second = Harness::with_state_root(Some(&state_root))?;
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
fn concurrent_starts_share_the_bootstrap_lock() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = serial_guard();
    let shared = TempDir::new()?;
    let state_root = shared.path().join("state");
    let first = Harness::with_state_root(Some(&state_root))?;
    let second = Harness::with_state_root(Some(&state_root))?;

    let (first_output, second_output) = std::thread::scope(|scope| {
        let first_start = scope.spawn(|| first.start("first", "sleep 30"));
        let second_start = scope.spawn(|| second.start("second", "sleep 30"));
        let first_output = first_start
            .join()
            .map_err(|_| std::io::Error::other("first start thread panicked"))?;
        let second_output = second_start
            .join()
            .map_err(|_| std::io::Error::other("second start thread panicked"))?;
        Ok::<_, std::io::Error>((first_output?, second_output?))
    })?;
    assert!(
        first_output.status.success(),
        "{}",
        String::from_utf8_lossy(&first_output.stdout)
    );
    assert!(
        second_output.status.success(),
        "{}",
        String::from_utf8_lossy(&second_output.stdout)
    );
    assert!(first.run(&["stop", "first", "--force"])?.status.success());
    assert!(second.run(&["stop", "second", "--force"])?.status.success());
    Ok(())
}

fn serial_guard() -> MutexGuard<'static, ()> {
    match E2E_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn state_sessions(state_root: &Path, project: &Path) -> Vec<String> {
    let projects = state_root.join("projects");
    let Ok(entries) = fs::read_dir(projects) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| fs::read(entry.path().join("state.json")).ok())
        .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(|value| value["project_root"].as_str() == project.to_str())
        .filter_map(|value| value["session"].as_str().map(str::to_owned))
        .collect()
}

fn session_is_live(session: &str) -> Result<bool, std::io::Error> {
    let output = Command::new("zellij")
        .args(["list-sessions", "--short", "--no-formatting"])
        .output()?;
    Ok(output.status.success()
        && String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line == session))
}
