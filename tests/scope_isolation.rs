#![cfg(unix)]

//! Integration test proving that two concurrent scopes never collide.
//!
//! Two `agent-terminal start server` processes run concurrently in separate
//! scopes (`PI_SESSION_ID=A` / `PI_SESSION_ID=B`, `AGENT_TERMINAL_SCOPE`
//! unset) against the same project, state root, socket namespace, and job
//! name. This is the exact scope-resolution path the pi and omp host adapters
//! rely on: pi injects `PI_SESSION_ID` natively and omp injects
//! `AGENT_TERMINAL_SCOPE`, and the CLI must keep the two sessions fully
//! isolated.
//!
//! Both starts must report a running job, each scope's `list` must see only
//! its own job, an unrelated scope must see nothing, and stopping each job
//! must leave no job and no owned `zellij` session behind.

use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use agent_terminal::paths::{project_digest, scope_digest};
use serde_json::Value;
use tempfile::TempDir;

/// Shell loop for the started job: exits cleanly on the interrupt signal so
/// `stop` returns promptly without needing the pane-close fallback.
const LOOP: &str = "trap \"exit 0\" INT; while :; do sleep 1; done";

/// Locates the pinned `zellij` binary: `$ZELLIJ_BIN`, then the repo cache
/// under `~/.cache/agent-terminal-zellij/zellij-0.44.3`, then `zellij` on
/// `PATH`. Returns `None` when no candidate resolves.
#[must_use]
fn locate_zellij() -> Option<PathBuf> {
    std::env::var_os("ZELLIJ_BIN")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".cache/agent-terminal-zellij/zellij-0.44.3"))
                .filter(|path| path.is_file())
                .or_else(which_zellij)
        })
}

#[must_use]
fn which_zellij() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("zellij"))
            .find(|candidate| candidate.is_file())
    })
}

/// Owns the shared project, state root, and socket directory that both scopes
/// use concurrently, plus a private bin directory exposing the located
/// `zellij` binary under its plain name.
struct Fixture {
    project: PathBuf,
    state_root: PathBuf,
    socket_dir: PathBuf,
    zellij_dir: PathBuf,
    host_path: String,
    _root: TempDir,
    _bin: TempDir,
}

impl Fixture {
    fn new(zellij: &Path) -> Result<Self, Box<dyn Error>> {
        let root = TempDir::new()?;
        let project = root.path().join("project");
        let state_root = root.path().join("state");
        let socket_dir = root.path().join("socket");
        fs::create_dir_all(&project)?;
        fs::create_dir_all(&state_root)?;
        fs::create_dir_all(&socket_dir)?;
        // The CLI resolves `zellij` from PATH, so expose the located binary
        // under its plain name in a private directory, then prepend that
        // directory to every child command's PATH — the same convention the
        // e2e gates use for their bundled `bin/zellij/zellij`.
        let bin = TempDir::new()?;
        let zellij_dir = bin.path().canonicalize()?;
        let link = zellij_dir.join("zellij");
        std::os::unix::fs::symlink(zellij, &link)?;
        let host_path = format!(
            "{}:{}",
            zellij_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Ok(Self {
            project: project.canonicalize()?,
            state_root: state_root.canonicalize()?,
            socket_dir: socket_dir.canonicalize()?,
            zellij_dir,
            host_path,
            _root: root,
            _bin: bin,
        })
    }

    /// Builds an `agent-terminal` invocation with a per-command environment:
    /// the shared state root, the shared socket namespace, `PATH` extended
    /// with the `zellij` directory, and the scope variables chosen by the
    /// caller. `AGENT_TERMINAL_SCOPE` and `PI_SESSION_ID` are never inherited
    /// from the test process, so each command sees exactly the scope the
    /// caller selects.
    #[must_use]
    fn command(&self, scope: Option<&str>, pi_session: Option<&str>, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-terminal"));
        command
            .current_dir(&self.project)
            .arg("--project")
            .arg(&self.project)
            .env("AGENT_TERMINAL_STATE", &self.state_root)
            .env("ZELLIJ_SOCKET_DIR", &self.socket_dir)
            .env("PATH", &self.host_path)
            .env("TERM", "xterm-256color")
            .env_remove("AGENT_TERMINAL_SCOPE")
            .env_remove("PI_SESSION_ID")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(scope) = scope {
            command.env("AGENT_TERMINAL_SCOPE", scope);
        }
        if let Some(pi_session) = pi_session {
            command.env("PI_SESSION_ID", pi_session);
        }
        command.args(args);
        command
    }
}

/// Parses a successful JSON response body, asserting a zero exit code and an
/// `ok` status, with the operation name in the failure message.
#[must_use]
fn parse_ok(output: &Output, context: &str) -> Value {
    assert!(
        output.status.success(),
        "{context}: exit={:?} stdout={} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let parsed: Result<Value, serde_json::Error> = serde_json::from_slice(&output.stdout);
    assert!(
        parsed.is_ok(),
        "{context}: stdout was not valid JSON; stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );
    let body = parsed.unwrap_or(Value::Null);
    assert_eq!(
        body["status"], "ok",
        "{context}: unexpected response {body}"
    );
    body
}

/// Runs the CLI under an explicit scope and returns the parsed `ok` body.
fn run_ok(
    fixture: &Fixture,
    scope: &str,
    pi_session: Option<&str>,
    args: &[&str],
) -> Result<Value, Box<dyn Error>> {
    let output = fixture.command(Some(scope), pi_session, args).output()?;
    let context = format!("scope {scope:?}: agent-terminal {}", args.join(" "));
    Ok(parse_ok(&output, &context))
}

/// True when no `zellij` session owned by `project` — same session-name
/// prefix the CLI derives — is listed in `socket_dir`.
fn no_sessions_for_project(
    zellij_dir: &Path,
    socket_dir: &Path,
    project: &Path,
) -> Result<bool, Box<dyn Error>> {
    let digest = project_digest(project);
    let prefix = format!("agent-terminal-{}-", &digest[..12]);
    let host_path = format!(
        "{}:{}",
        zellij_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new("zellij")
        .env("ZELLIJ_SOCKET_DIR", socket_dir)
        .env("PATH", host_path)
        .env("TERM", "xterm-256color")
        .args(["list-sessions", "--short", "--no-formatting"])
        .output()?;
    if output.status.success() {
        Ok(!String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.starts_with(&prefix)))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("No active zellij sessions"),
            "zellij list-sessions failed: exit={:?} stderr={stderr}",
            output.status.code(),
        );
        Ok(true)
    }
}

#[test]
fn two_concurrent_scopes_never_collide() -> Result<(), Box<dyn Error>> {
    let zellij = locate_zellij().unwrap_or_default();
    assert!(
        zellij.is_file(),
        "no Zellij binary found: set ZELLIJ_BIN, install the pinned binary at \
         ~/.cache/agent-terminal-zellij/zellij-0.44.3, or add zellij to PATH"
    );
    let fixture = Fixture::new(&zellij)?;

    // Two concurrent `start` processes in different scopes share the state
    // root, socket namespace, project, and job name. The scope comes from
    // `PI_SESSION_ID` alone (`AGENT_TERMINAL_SCOPE` unset) — the exact
    // fallback the pi host adapter relies on.
    let first_start = fixture
        .command(
            None,
            Some("A"),
            &["start", "server", "--", "/bin/sh", "-c", LOOP],
        )
        .spawn()?;
    let second_start = fixture
        .command(
            None,
            Some("B"),
            &["start", "server", "--", "/bin/sh", "-c", LOOP],
        )
        .spawn()?;
    let first_body = parse_ok(&first_start.wait_with_output()?, "start server (scope A)");
    let second_body = parse_ok(&second_start.wait_with_output()?, "start server (scope B)");
    assert_eq!(first_body["state"], "running", "scope A start body");
    assert_eq!(second_body["state"], "running", "scope B start body");

    // Each scope sees exactly its own running job; an unrelated scope sees
    // nothing.
    let first_list = run_ok(&fixture, "A", Some("A"), &["list"])?;
    assert_eq!(
        first_list["jobs"],
        Value::Array(vec![Value::Object(
            [("job", "server"), ("state", "running")]
                .into_iter()
                .map(|(key, value)| (key.to_owned(), Value::String(value.to_owned())))
                .collect()
        )]),
        "scope A list"
    );
    let second_list = run_ok(&fixture, "B", Some("B"), &["list"])?;
    assert_eq!(
        second_list["jobs"],
        Value::Array(vec![Value::Object(
            [("job", "server"), ("state", "running")]
                .into_iter()
                .map(|(key, value)| (key.to_owned(), Value::String(value.to_owned())))
                .collect()
        )]),
        "scope B list"
    );
    let other_list = run_ok(&fixture, "other", Some("other"), &["list"])?;
    assert_eq!(other_list["jobs"], Value::Array(vec![]), "scope other list");

    // Scope A can read its own job's running screen.
    let first_read = run_ok(&fixture, "A", Some("A"), &["read", "server"])?;
    assert_eq!(first_read["state"], "running", "scope A read");
    assert!(
        first_read["screen"].is_string(),
        "scope A read produced no screen string: {first_read}"
    );

    // Stopping the job in each scope clears that scope's state.
    run_ok(&fixture, "A", Some("A"), &["stop", "server"])?;
    run_ok(&fixture, "B", Some("B"), &["stop", "server"])?;
    let first_final = run_ok(&fixture, "A", Some("A"), &["list"])?;
    assert_eq!(
        first_final["jobs"],
        Value::Array(vec![]),
        "scope A final list"
    );
    let second_final = run_ok(&fixture, "B", Some("B"), &["list"])?;
    assert_eq!(
        second_final["jobs"],
        Value::Array(vec![]),
        "scope B final list"
    );

    // No `zellij` session owned by this project survives in either scope's
    // derived socket namespace.
    for scope in ["A", "B"] {
        let socket_dir =
            std::env::temp_dir().join(format!("agent-terminal-{}", scope_digest(scope)));
        assert!(
            no_sessions_for_project(&fixture.zellij_dir, &socket_dir, &fixture.project)?,
            "scope {scope}: owned Zellij session was left behind"
        );
    }
    Ok(())
}
