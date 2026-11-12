use std::{
    fs::{self, File},
    io::{Read as _, Seek as _, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use wait_timeout::ChildExt as _;

use crate::{
    error::Error,
    owner::{any_dead, read_owners},
    zellij::{PaneSnapshot, job_panes, keeper_pane, unknown_panes},
};

const POLL: Duration = Duration::from_secs(1);
const IDLE_AFTER: Duration = Duration::from_secs(60);

pub fn run(owners: &Path, ready: &Path, zellij: &Path, lock: &Path) -> Result<(), Error> {
    let session = std::env::var("ZELLIJ_SESSION_NAME").map_err(|_| Error::InvalidInput {
        message: "keeper is not running inside a Zellij session".to_owned(),
    })?;
    let owner_chain = if owners == Path::new("-") {
        None
    } else {
        Some(match read_owners(owners) {
            Ok(chain)
                if !chain.is_empty() && !any_dead(Path::new("/proc"), &chain).unwrap_or(true) =>
            {
                chain
            }
            Ok(_) | Err(_) => {
                while !kill(zellij, &session) {
                    thread::sleep(POLL);
                }
                return Ok(());
            }
        })
    };
    if ready != Path::new("-") {
        write_ready(ready)?;
    }
    let mut last_job = Instant::now();
    loop {
        if let Some(owners) = &owner_chain
            && any_dead(Path::new("/proc"), owners).unwrap_or(true)
        {
            if kill(zellij, &session) {
                return Ok(());
            }
            thread::sleep(POLL);
            continue;
        }
        let panes = match list_panes(zellij) {
            Ok(panes) => panes,
            Err(error) => {
                tracing::warn!(%error, "keeper pane poll failed; retrying");
                thread::sleep(POLL);
                continue;
            }
        };
        if job_panes(&panes).is_empty() {
            if last_job.elapsed() >= IDLE_AFTER
                && let Ok(Some(_lock)) = try_lock(lock)
                && let Ok(panes) = list_panes(zellij)
                && job_panes(&panes).is_empty()
                && unknown_panes(&panes).is_empty()
                && keeper_pane(&panes).is_some()
                && kill(zellij, &session)
            {
                return Ok(());
            }
        } else {
            last_job = Instant::now();
        }
        thread::sleep(POLL);
    }
}

fn list_panes(zellij: &Path) -> Result<Vec<PaneSnapshot>, Error> {
    let output = run_bounded(zellij, &["action", "list-panes", "--json"])?;
    if !output.status.success() {
        return Err(Error::ZellijFailed {
            message: "keeper could not list panes".to_owned(),
        });
    }
    serde_json::from_slice(&output.stdout).map_err(|source| {
        tracing::warn!(
            line = source.line(),
            column = source.column(),
            stdout_captured_raw_bytes = output.stdout.len(),
            stderr_captured_raw_bytes = output.stderr.len(),
            "keeper saw invalid pane JSON"
        );
        Error::ZellijFailed {
            message: "keeper saw invalid pane data".to_owned(),
        }
    })
}

fn kill(zellij: &Path, session: &str) -> bool {
    for _ in 0..5 {
        let killed = run_bounded(zellij, &["kill-session", session])
            .is_ok_and(|output| output.status.success());
        if killed && session_exists(zellij, session).is_ok_and(|exists| !exists) {
            return true;
        }
        thread::sleep(Duration::from_millis(200));
    }
    false
}

fn session_exists(zellij: &Path, session: &str) -> Result<bool, ()> {
    let output =
        run_bounded(zellij, &["list-sessions", "--short", "--no-formatting"]).map_err(|_| ())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stdout.contains("No active zellij sessions found")
        || stderr.contains("No active zellij sessions found")
    {
        return Ok(false);
    }
    if !output.status.success() {
        return Err(());
    }
    Ok(stdout.lines().any(|line| line == session))
}

fn run_bounded(executable: &Path, arguments: &[&str]) -> Result<std::process::Output, Error> {
    let stdout = Capture::new()?;
    let stderr = Capture::new()?;
    let mut child = Command::new(executable)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(stdout.handle()?)
        .stderr(stderr.handle()?)
        .spawn()
        .map_err(|source| Error::state_io("run zellij from keeper", executable, source))?;
    let status = match child.wait_timeout(Duration::from_secs(5)) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _kill = child.kill();
            let _reap = child.wait();
            return Err(Error::ZellijTimeout);
        }
        Err(source) => {
            let _kill = child.kill();
            let _reap = child.wait();
            return Err(Error::state_io(
                "wait for zellij from keeper",
                executable,
                source,
            ));
        }
    };
    Ok(std::process::Output {
        status,
        stdout: stdout.read()?,
        stderr: stderr.read()?,
    })
}

struct Capture(File);

impl Capture {
    fn new() -> Result<Self, Error> {
        let file = tempfile::tempfile().map_err(|source| {
            Error::state_io("create keeper capture file", std::env::temp_dir(), source)
        })?;
        Ok(Self(file))
    }

    fn handle(&self) -> Result<Stdio, Error> {
        self.0.try_clone().map(Stdio::from).map_err(|source| {
            Error::state_io("clone keeper capture handle", std::env::temp_dir(), source)
        })
    }

    fn read(mut self) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        self.0
            .seek(SeekFrom::Start(0))
            .and_then(|_| self.0.take(1024 * 1024).read_to_end(&mut bytes))
            .map_err(|source| {
                Error::state_io("read keeper capture", std::env::temp_dir(), source)
            })?;
        Ok(bytes)
    }
}

fn write_ready(path: &Path) -> Result<(), Error> {
    let pane = std::env::var("ZELLIJ_PANE_ID").map_err(|_| Error::InvalidInput {
        message: "keeper is not running inside a Zellij pane".to_owned(),
    })?;
    let temporary = path.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&temporary, pane)
        .map_err(|source| Error::state_io("write keeper ready file", &temporary, source))?;
    fs::rename(&temporary, path)
        .map_err(|source| Error::state_io("publish keeper ready file", path, source))
}

fn try_lock(path: &Path) -> Result<Option<fs::File>, Error> {
    use fs2::FileExt as _;
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(|source| Error::state_io("open project lock", path, source))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(_) => Ok(None),
    }
}

#[must_use]
pub fn lock_path(paths: &crate::paths::ProjectPaths) -> PathBuf {
    paths.lock_file()
}
