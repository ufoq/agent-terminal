#![cfg(unix)]
// SIZE_OK: one serialized real-Zellij scenario suite

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{Barrier, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use serde_json::Value;
use tempfile::TempDir;

static E2E_LOCK: Mutex<()> = Mutex::new(());

struct Harness {
    _temp: TempDir,
    project: PathBuf,
    socket_dir: PathBuf,
    state_root: PathBuf,
}

impl Harness {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_state_root(None)
    }

    fn with_state_root(state_root: Option<&Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let project = temp.path().join("project");
        let socket_dir = temp.path().join("socket");
        fs::create_dir_all(&project)?;
        fs::create_dir_all(&socket_dir)?;
        Ok(Self {
            project: project.canonicalize()?,
            socket_dir: socket_dir.canonicalize()?,
            state_root: state_root.map_or_else(|| temp.path().join("state"), Path::to_path_buf),
            _temp: temp,
        })
    }

    fn run(&self, arguments: &[&str]) -> Result<Output, std::io::Error> {
        let mut command = Command::new(assert_cmd::cargo::cargo_bin!("agent-terminal"));
        command
            .current_dir(&self.project)
            .env("ZELLIJ_SOCKET_DIR", &self.socket_dir)
            .arg("--state-dir")
            .arg(&self.state_root)
            .args(arguments)
            .output()
    }

    fn run_retrying_lock(&self, arguments: &[&str]) -> Result<Output, std::io::Error> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let output = self.run(arguments)?;
            let body: Value = serde_json::from_slice(&output.stdout)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            if body["error"]["code"] != "lock_busy" || Instant::now() >= deadline {
                return Ok(output);
            }
            std::thread::yield_now();
        }
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

    fn session_name(&self) -> Result<String, Box<dyn std::error::Error>> {
        match self.sessions().as_slice() {
            [session] => Ok(session.clone()),
            sessions => Err(format!("expected one Zellij session, found {sessions:?}").into()),
        }
    }

    fn kill_session_externally(&self) -> Result<(), Box<dyn std::error::Error>> {
        let session = self.session_name()?;
        let output = Command::new("zellij")
            .env("ZELLIJ_SOCKET_DIR", &self.socket_dir)
            .args(["kill-session", session.as_str()])
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "could not kill Zellij session {session:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into())
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        for session in self.sessions() {
            let _result = Command::new("zellij")
                .env("ZELLIJ_SOCKET_DIR", &self.socket_dir)
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
        assert!(!session_is_live(&harness.socket_dir, &session)?);
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

#[test]
fn concurrent_starts_to_same_job_serialize() -> Result<(), Box<dyn std::error::Error>> {
    let _lock = serial_guard();
    let harness = Harness::new()?;
    let project = harness.project.to_string_lossy();
    let barrier = Barrier::new(3);
    let (first_output, second_output) = std::thread::scope(|scope| {
        let first_start = scope.spawn(|| {
            barrier.wait();
            harness.run(&[
                "--project",
                project.as_ref(),
                "start",
                "race",
                "--",
                "/bin/sh",
                "-c",
                "while :; do sleep 1; done",
            ])
        });
        let second_start = scope.spawn(|| {
            barrier.wait();
            harness.run(&[
                "--project",
                project.as_ref(),
                "start",
                "race",
                "--",
                "/bin/sh",
                "-c",
                "while :; do sleep 1; done",
            ])
        });
        barrier.wait();
        let first_output = first_start
            .join()
            .map_err(|_| std::io::Error::other("first start thread panicked"))?;
        let second_output = second_start
            .join()
            .map_err(|_| std::io::Error::other("second start thread panicked"))?;
        Ok::<_, std::io::Error>((first_output?, second_output?))
    })?;
    let first_body: Value = serde_json::from_slice(&first_output.stdout)?;
    let second_body: Value = serde_json::from_slice(&second_output.stdout)?;
    let starts = [(&first_output, &first_body), (&second_output, &second_body)];
    assert_eq!(
        starts
            .iter()
            .filter(|(output, _body)| output.status.success())
            .count(),
        1,
        "first={first_body}, second={second_body}"
    );
    for (output, body) in starts {
        if output.status.success() {
            assert_eq!(body["data"]["state"], "running");
        } else {
            assert!(
                matches!(
                    body["error"]["code"].as_str(),
                    Some("lock_busy" | "job_exists")
                ),
                "{body}"
            );
        }
    }

    let listed = harness.run(&["list"])?;
    let list_body: Value = serde_json::from_slice(&listed.stdout)?;
    assert!(listed.status.success(), "{list_body}");
    assert_eq!(list_body["data"]["jobs"].as_array().map(Vec::len), Some(1));
    assert_eq!(list_body["data"]["jobs"][0]["job"], "race");
    assert!(harness.run(&["stop", "race", "--force"])?.status.success());
    Ok(())
}

#[test]
fn concurrent_stop_and_read_do_not_corrupt() -> Result<(), Box<dyn std::error::Error>> {
    let _lock = serial_guard();
    let harness = Harness::new()?;
    let started = harness.start("race", "while :; do sleep 1; done")?;
    let start_body: Value = serde_json::from_slice(&started.stdout)?;
    assert!(started.status.success(), "{start_body}");
    assert_eq!(start_body["data"]["state"], "running");

    let barrier = Barrier::new(3);
    let (stopped, read_bodies) = std::thread::scope(|scope| {
        let stop = scope.spawn(|| {
            barrier.wait();
            harness.run_retrying_lock(&["stop", "race", "--force"])
        });
        let read = scope.spawn(|| {
            barrier.wait();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut bodies = Vec::new();
            loop {
                let output = harness.run(&["read", "race"])?;
                let body: Value = serde_json::from_slice(&output.stdout)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
                let finished = if output.status.success() {
                    body["data"]["state"] != "running"
                } else {
                    body["status"] == "error" && body["error"]["code"].as_str().is_some()
                };
                bodies.push(body);
                if finished {
                    return Ok::<_, std::io::Error>(bodies);
                }
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "read remained running during concurrent stop",
                    ));
                }
                std::thread::yield_now();
            }
        });
        barrier.wait();
        let stopped = stop
            .join()
            .map_err(|_| std::io::Error::other("stop thread panicked"))?;
        let read_bodies = read
            .join()
            .map_err(|_| std::io::Error::other("read thread panicked"))?;
        Ok::<_, std::io::Error>((stopped?, read_bodies?))
    })?;
    let stop_body: Value = serde_json::from_slice(&stopped.stdout)?;
    assert!(stopped.status.success(), "{stop_body}");
    assert_eq!(stop_body["data"]["cleaned_up"], true);
    assert!(!read_bodies.is_empty());
    for body in &read_bodies {
        if body["status"] == "ok" {
            assert!(
                matches!(
                    body["data"]["state"].as_str(),
                    Some("running" | "exited" | "lost")
                ),
                "{body}"
            );
        } else {
            assert!(body["error"]["code"].as_str().is_some(), "{body}");
            assert_ne!(body["error"]["code"], "state_corrupt");
        }
    }

    let listed = harness.run(&["list"])?;
    let list_body: Value = serde_json::from_slice(&listed.stdout)?;
    assert!(listed.status.success(), "{list_body}");
    assert_eq!(list_body["data"]["jobs"], serde_json::json!([]));
    Ok(())
}

#[test]
fn concurrent_sends_to_same_job_serialize() -> Result<(), Box<dyn std::error::Error>> {
    let _lock = serial_guard();
    let harness = Harness::new()?;
    let started = harness.run(&[
        "start",
        "race",
        "--",
        "/bin/sh",
        "-c",
        "printf 'ready\n'; IFS= read -r first; printf 'accepted=<%s>\n' \"$first\"; IFS= read -r second; printf 'accepted=<%s>\n' \"$second\"; while :; do sleep 1; done",
    ])?;
    let start_body: Value = serde_json::from_slice(&started.stdout)?;
    assert!(started.status.success(), "{start_body}");
    harness.read_until("race", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("ready"))
    })?;

    let barrier = Barrier::new(3);
    let (first_send, second_send) = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            barrier.wait();
            harness.run_retrying_lock(&["send", "race", "--", "thread-A"])
        });
        let second = scope.spawn(|| {
            barrier.wait();
            harness.run_retrying_lock(&["send", "race", "--", "thread-B"])
        });
        barrier.wait();
        let first_send = first
            .join()
            .map_err(|_| std::io::Error::other("first send thread panicked"))?;
        let second_send = second
            .join()
            .map_err(|_| std::io::Error::other("second send thread panicked"))?;
        Ok::<_, std::io::Error>((first_send?, second_send?))
    })?;
    let first_body: Value = serde_json::from_slice(&first_send.stdout)?;
    let second_body: Value = serde_json::from_slice(&second_send.stdout)?;
    for (output, body) in [(&first_send, &first_body), (&second_send, &second_body)] {
        if output.status.success() {
            assert_eq!(body["data"]["submitted"], true);
        } else {
            assert_eq!(body["error"]["code"], "job_not_running", "{body}");
        }
    }

    let read = harness.read_until("race", |body| {
        body["data"]["screen"].as_str().is_some_and(|screen| {
            screen.contains("accepted=<thread-A>") && screen.contains("accepted=<thread-B>")
        })
    })?;
    let Some(screen) = read["data"]["screen"].as_str() else {
        return Err("race screen was unavailable after both sends".into());
    };
    assert_eq!(screen.matches("accepted=<thread-A>").count(), 1, "{screen}");
    assert_eq!(screen.matches("accepted=<thread-B>").count(), 1, "{screen}");
    assert_eq!(screen.matches("accepted=<").count(), 2, "{screen}");
    assert!(harness.run(&["stop", "race", "--force"])?.status.success());
    Ok(())
}

#[test]
fn cwd_and_argv_preserve_spaces_and_metacharacters() -> Result<(), Box<dyn std::error::Error>> {
    let _lock = serial_guard();
    let harness = Harness::new()?;
    let subdir = harness.project.join("sub dir");
    fs::create_dir(&subdir)?;
    let canonical_subdir = subdir.canonicalize()?;
    let cwd = canonical_subdir.to_string_lossy();
    let started = harness.run(&[
        "start",
        "argv",
        "--cwd",
        cwd.as_ref(),
        "--",
        "/bin/sh",
        "-c",
        "printf 'cwd=<%s>\\narg=<%s>\\n' \"$PWD\" \"$1\"",
        "sh",
        "a b;$HOME",
    ])?;
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stdout)
    );

    let read = harness.read_until("argv", |body| {
        body["data"]["screen"].as_str().is_some_and(|screen| {
            screen.contains(cwd.as_ref()) && screen.contains("arg=<a b;$HOME>")
        })
    })?;
    let screen = read["data"]["screen"].as_str().unwrap_or_default();
    assert!(screen.contains(&format!("cwd=<{}>", canonical_subdir.display())));
    assert!(screen.contains("arg=<a b;$HOME>"));
    assert!(harness.run(&["stop", "argv"])?.status.success());
    Ok(())
}

#[test]
fn send_no_submit_then_press_submits() -> Result<(), Box<dyn std::error::Error>> {
    let _lock = serial_guard();
    let harness = Harness::new()?;
    let started = harness.run(&[
        "start",
        "job",
        "--",
        "/bin/sh",
        "-c",
        "printf 'armed\\n'; IFS= read -r line; printf 'accepted=<%s>\\n' \"$line\"",
    ])?;
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stdout)
    );
    harness.read_until("job", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("armed"))
    })?;

    let first_send = harness.run(&["send", "job", "--no-submit", "--", "abx"])?;
    let first_send_body: Value = serde_json::from_slice(&first_send.stdout)?;
    assert!(first_send.status.success(), "{first_send_body}");
    assert_eq!(first_send_body["data"]["submitted"], false);

    let backspace = harness.run(&["press", "job", "--", "Backspace"])?;
    let backspace_body: Value = serde_json::from_slice(&backspace.stdout)?;
    assert!(backspace.status.success(), "{backspace_body}");
    assert_eq!(backspace_body["data"]["keys"][0], "Backspace");

    let second_send = harness.run(&["send", "job", "--no-submit", "--", "c"])?;
    let second_send_body: Value = serde_json::from_slice(&second_send.stdout)?;
    assert!(second_send.status.success(), "{second_send_body}");
    assert_eq!(second_send_body["data"]["submitted"], false);

    let enter = harness.run(&["press", "job", "--", "Enter"])?;
    let enter_body: Value = serde_json::from_slice(&enter.stdout)?;
    assert!(enter.status.success(), "{enter_body}");
    assert_eq!(enter_body["data"]["keys"][0], "Enter");

    harness.read_until("job", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("accepted=<abc>"))
    })?;
    assert!(harness.run(&["stop", "job"])?.status.success());
    Ok(())
}

#[test]
fn input_to_exited_job_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let _lock = serial_guard();
    let harness = Harness::new()?;
    let started = harness.run(&[
        "start",
        "done",
        "--",
        "/bin/sh",
        "-c",
        "printf done; exit 0",
    ])?;
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stdout)
    );
    harness.read_until("done", |body| body["data"]["state"] == "exited")?;

    let send = harness.run(&["send", "done", "--", "text"])?;
    let send_body: Value = serde_json::from_slice(&send.stdout)?;
    assert_eq!(send.status.code(), Some(1));
    assert_eq!(send_body["error"]["code"], "job_not_running");

    let press = harness.run(&["press", "done", "--", "Enter"])?;
    let press_body: Value = serde_json::from_slice(&press.stdout)?;
    assert_eq!(press.status.code(), Some(1));
    assert_eq!(press_body["error"]["code"], "job_not_running");

    let stopped = harness.run(&["stop", "done"])?;
    let stop_body: Value = serde_json::from_slice(&stopped.stdout)?;
    assert!(stopped.status.success(), "{stop_body}");
    assert_eq!(stop_body["data"]["cleaned_up"], true);
    Ok(())
}

#[test]
fn graceful_stop_refuses_then_force_succeeds() -> Result<(), Box<dyn std::error::Error>> {
    let _lock = serial_guard();
    let harness = Harness::new()?;
    let started = harness.run(&[
        "start",
        "stubborn",
        "--",
        "/bin/sh",
        "-c",
        "trap '' INT; printf ready; while :; do sleep 1; done",
    ])?;
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stdout)
    );
    harness.read_until("stubborn", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("ready"))
    })?;

    let graceful = harness.run(&["stop", "stubborn"])?;
    let graceful_body: Value = serde_json::from_slice(&graceful.stdout)?;
    assert_eq!(graceful.status.code(), Some(1), "{graceful_body}");
    assert_eq!(graceful_body["error"]["code"], "job_still_running");

    let read = harness.run(&["read", "stubborn"])?;
    let read_body: Value = serde_json::from_slice(&read.stdout)?;
    assert!(read.status.success(), "{read_body}");
    assert_eq!(read_body["data"]["state"], "running");

    let forced = harness.run(&["stop", "stubborn", "--force"])?;
    let forced_body: Value = serde_json::from_slice(&forced.stdout)?;
    assert!(forced.status.success(), "{forced_body}");
    assert_eq!(forced_body["data"]["cleaned_up"], true);
    assert_eq!(forced_body["data"]["forced"], true);
    Ok(())
}

#[test]
fn stop_already_exited_job_is_not_forced() -> Result<(), Box<dyn std::error::Error>> {
    let _lock = serial_guard();
    let harness = Harness::new()?;
    let started = harness.run(&[
        "start",
        "done",
        "--",
        "/bin/sh",
        "-c",
        "printf final; exit 0",
    ])?;
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stdout)
    );
    harness.read_until("done", |body| body["data"]["state"] == "exited")?;

    let stopped = harness.run(&["stop", "done", "--force"])?;
    let stop_body: Value = serde_json::from_slice(&stopped.stdout)?;
    assert!(stopped.status.success(), "{stop_body}");
    assert_eq!(stop_body["data"]["cleaned_up"], true);
    assert_eq!(stop_body["data"]["forced"], false);
    assert!(
        stop_body["data"]["last_screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("final"))
    );
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

#[test]
fn external_session_loss_reports_lost() -> Result<(), Box<dyn std::error::Error>> {
    let _lock = serial_guard();
    let harness = Harness::new()?;
    assert!(
        harness
            .start("lost-job", "while :; do sleep 1; done")?
            .status
            .success()
    );
    let running = harness.run(&["read", "lost-job"])?;
    let running_body: Value = serde_json::from_slice(&running.stdout)?;
    assert!(running.status.success(), "{running_body}");
    assert_eq!(running_body["data"]["state"], "running");

    harness.kill_session_externally()?;

    let lost = harness.run(&["read", "lost-job"])?;
    let lost_body: Value = serde_json::from_slice(&lost.stdout)?;
    assert!(lost.status.success(), "{lost_body}");
    assert_eq!(lost_body["data"]["state"], "lost");
    assert_eq!(lost_body["data"]["screen_available"], false);

    let send = harness.run(&["send", "lost-job", "--", "text"])?;
    let send_body: Value = serde_json::from_slice(&send.stdout)?;
    assert_eq!(send.status.code(), Some(1));
    assert_eq!(send_body["error"]["code"], "job_not_running");

    let stopped = harness.run(&["stop", "lost-job"])?;
    let stop_body: Value = serde_json::from_slice(&stopped.stdout)?;
    assert!(stopped.status.success(), "{stop_body}");
    assert_eq!(stop_body["data"]["cleaned_up"], true);
    assert_eq!(stop_body["data"]["forced"], false);
    Ok(())
}

#[test]
fn screen_is_ansi_stripped_utf8_safe_and_bounded() -> Result<(), Box<dyn std::error::Error>> {
    let _lock = serial_guard();
    let harness = Harness::new()?;
    let started = harness.run(&[
        "start",
        "screen",
        "--",
        "/bin/sh",
        "-c",
        "i=1; while [ \"$i\" -le 205 ]; do printf 'line-%03d\\n' \"$i\"; i=$((i + 1)); done; printf '\\033[31mRED\\033[0m\\n한글\\n'",
    ])?;
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stdout)
    );

    let read = harness.read_until("screen", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("한글"))
    })?;
    let screen = read["data"]["screen"].as_str().unwrap_or_default();
    assert_eq!(read["data"]["truncated"], true);
    assert!(!screen.as_bytes().contains(&0x1b));
    assert!(screen.contains("RED"));
    assert!(screen.contains("한글"));
    assert!(screen.lines().count() <= 200);
    assert!(screen.len() <= 32 * 1024);
    assert!(harness.run(&["stop", "screen"])?.status.success());
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

fn session_is_live(socket_dir: &Path, session: &str) -> Result<bool, std::io::Error> {
    let output = Command::new("zellij")
        .env("ZELLIJ_SOCKET_DIR", socket_dir)
        .args(["list-sessions", "--short", "--no-formatting"])
        .output()?;
    Ok(output.status.success()
        && String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line == session))
}
