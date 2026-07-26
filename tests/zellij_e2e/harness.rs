use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{Arc, LazyLock, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use serde_json::Value;
use tempfile::TempDir;

static SOCKET_LOCKS: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub struct Harness {
    _temp: TempDir,
    pub project: PathBuf,
    pub socket_dir: PathBuf,
    state_root: PathBuf,
}

impl Harness {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_state_root(None, None)
    }

    pub fn with_state_root(
        state_root: Option<&Path>,
        socket_dir: Option<&Path>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let project = temp.path().join("project");
        let socket_dir = socket_dir.map_or_else(|| temp.path().join("socket"), Path::to_path_buf);
        fs::create_dir_all(&project)?;
        fs::create_dir_all(&socket_dir)?;
        Ok(Self {
            project: project.canonicalize()?,
            socket_dir: socket_dir.canonicalize()?,
            state_root: state_root.map_or_else(|| temp.path().join("state"), Path::to_path_buf),
            _temp: temp,
        })
    }

    pub fn run(&self, arguments: &[&str]) -> Result<Output, std::io::Error> {
        let mut command = Command::new(assert_cmd::cargo::cargo_bin!("agent-terminal"));
        command
            .current_dir(&self.project)
            .env("ZELLIJ_SOCKET_DIR", &self.socket_dir)
            .arg("--state-dir")
            .arg(&self.state_root)
            .args(arguments)
            .output()
    }

    pub fn run_ok(&self, arguments: &[&str]) -> Result<Value, Box<dyn std::error::Error>> {
        success_body(&self.run(arguments)?)
    }

    pub fn run_retrying_lock(&self, arguments: &[&str]) -> Result<Output, std::io::Error> {
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

    pub fn start(&self, job: &str, shell_command: &str) -> Result<Output, std::io::Error> {
        self.run(&["start", job, "--", "sh", "-c", shell_command])
    }

    pub fn start_ok(
        &self,
        job: &str,
        shell_command: &str,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        success_body(&self.start(job, shell_command)?)
    }

    pub fn read_until(
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

    pub fn sessions(&self) -> Vec<String> {
        state_sessions(&self.state_root, &self.project)
    }

    fn session_name(&self) -> Result<String, Box<dyn std::error::Error>> {
        match self.sessions().as_slice() {
            [session] => Ok(session.clone()),
            sessions => Err(format!("expected one Zellij session, found {sessions:?}").into()),
        }
    }

    pub fn kill_session_externally(&self) -> Result<(), Box<dyn std::error::Error>> {
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

pub fn socket_guard(socket_dir: &Path) -> MutexGuard<'static, ()> {
    let canonical_socket_dir = socket_dir
        .canonicalize()
        .unwrap_or_else(|_| socket_dir.to_path_buf());
    let socket_lock = {
        let mut socket_locks = match SOCKET_LOCKS.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        Arc::clone(
            socket_locks
                .entry(canonical_socket_dir)
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    };
    let static_socket_lock = Box::leak(Box::new(socket_lock));
    match static_socket_lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub fn session_is_live(socket_dir: &Path, session: &str) -> Result<bool, std::io::Error> {
    let output = Command::new("zellij")
        .env("ZELLIJ_SOCKET_DIR", socket_dir)
        .args(["list-sessions", "--short", "--no-formatting"])
        .output()?;
    Ok(output.status.success()
        && String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line == session))
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

fn success_body(output: &Output) -> Result<Value, Box<dyn std::error::Error>> {
    let body = serde_json::from_slice(&output.stdout)?;
    assert!(output.status.success(), "{body}");
    Ok(body)
}
