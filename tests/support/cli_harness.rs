use std::{
    ffi::OsString,
    fmt::Display,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use fs2::FileExt as _;
use serde_json::Value;
use tempfile::TempDir;

/// Returns a command that runs the Cargo-built `agent-terminal` binary.
#[must_use]
pub fn binary() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("agent-terminal"))
}

/// Owns isolated project and state directories for deterministic CLI calls.
#[derive(Debug)]
pub struct DeterministicHarness {
    /// Canonical project directory passed to the CLI.
    pub project: PathBuf,
    /// Canonical state root passed to the CLI.
    pub state_dir: PathBuf,
    /// Temporary root that keeps all harness paths alive.
    pub root: TempDir,
    environment: Vec<(OsString, OsString)>,
    current_dir: Option<PathBuf>,
    explicit_project: bool,
}

impl DeterministicHarness {
    /// Creates an isolated project and state root.
    #[must_use]
    pub fn new() -> Self {
        let root = must(TempDir::new(), "create harness temporary directory");
        let project = root.path().join("project");
        let state_dir = root.path().join("state");
        must(
            fs::create_dir_all(&project),
            "create harness project directory",
        );
        must(
            fs::create_dir_all(&state_dir),
            "create harness state directory",
        );
        let project = must(project.canonicalize(), "canonicalize harness project");
        let state_dir = must(state_dir.canonicalize(), "canonicalize harness state root");
        Self {
            project,
            state_dir,
            root,
            environment: Vec::new(),
            current_dir: None,
            explicit_project: true,
        }
    }

    /// Adds an environment override for subsequent CLI calls.
    pub fn env(&mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> &mut Self {
        self.environment.push((key.into(), value.into()));
        self
    }

    /// Sets the working directory for subsequent CLI calls.
    pub fn current_dir(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.current_dir = Some(path.into());
        self
    }

    /// Lets the CLI discover project scope from the invocation directory.
    pub const fn use_implicit_project_scope(&mut self) -> &mut Self {
        self.explicit_project = false;
        self
    }

    /// Runs the CLI in the harness scope without a discoverable Zellij binary.
    #[must_use]
    pub fn run(&self, args: &[&str]) -> Output {
        let mut command = binary();
        command
            .envs(self.environment.iter().map(|(key, value)| (key, value)))
            .env("PATH", "/nonexistent");
        if self.explicit_project {
            command.arg("--project").arg(&self.project);
        }
        command.arg("--state-dir").arg(&self.state_dir).args(args);
        if let Some(current_dir) = &self.current_dir {
            command.current_dir(current_dir);
        }
        must(command.output(), "run agent-terminal")
    }
}

impl Default for DeterministicHarness {
    fn default() -> Self {
        Self::new()
    }
}

/// Parses a successful JSON response and returns its complete value.
#[must_use]
pub fn assert_json_ok(output: &Output) -> Value {
    let context = output_context(output);
    let parsed = serde_json::from_slice::<Value>(&output.stdout);
    assert!(parsed.is_ok(), "stdout was not valid JSON; {context}");
    let value = parsed.unwrap_or(Value::Null);
    assert!(
        output.status.success(),
        "JSON response exited unsuccessfully; {context}"
    );
    assert_eq!(
        value.get("status").and_then(Value::as_str),
        Some("ok"),
        "JSON response status was not ok; {context}"
    );
    value
}

/// Asserts an error response's exit code, single JSON object, and error code.
pub fn assert_json_error(output: &Output, expected_code: &str, expected_exit: i32) {
    let context = output_context(output);
    assert_eq!(
        output.status.code(),
        Some(expected_exit),
        "unexpected process exit code; {context}"
    );
    let parsed = serde_json::from_slice::<Value>(&output.stdout);
    assert!(
        parsed.is_ok(),
        "stdout did not contain exactly one JSON value; {context}"
    );
    let value = parsed.unwrap_or(Value::Null);
    assert!(
        value.is_object(),
        "stdout JSON value was not an object; {context}"
    );
    assert_eq!(
        value.get("status").and_then(Value::as_str),
        Some("error"),
        "JSON response status was not error; {context}"
    );
    assert_eq!(
        value.pointer("/error/code").and_then(Value::as_str),
        Some(expected_code),
        "unexpected JSON error code; {context}"
    );
}

/// Holds an exclusive state-file lock until it is dropped.
#[derive(Debug)]
pub struct StateLockGuard {
    lock_file: File,
}

impl Drop for StateLockGuard {
    fn drop(&mut self) {
        drop(fs2::FileExt::unlock(&self.lock_file));
    }
}

/// Acquires the harness project's exclusive state lock.
#[must_use]
pub fn hold_state_lock(harness: &DeterministicHarness) -> StateLockGuard {
    let lock_path = project_state_dir(harness).join("state.lock");
    let lock_file = must(
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path),
        "open harness state lock",
    );
    must(lock_file.lock_exclusive(), "acquire harness state lock");
    StateLockGuard { lock_file }
}

/// Selects the registry corruption written by [`write_corrupt_state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorruptKind {
    /// Writes bytes that cannot be parsed as JSON.
    Syntax,
    /// Writes valid JSON that violates registry ownership invariants.
    Semantic,
}

/// Writes a corrupt registry to the harness project's state file.
pub fn write_corrupt_state(harness: &DeterministicHarness, kind: CorruptKind) {
    let contents = match kind {
        CorruptKind::Syntax => b"{not-valid-json".to_vec(),
        CorruptKind::Semantic => {
            let project_root = must(
                serde_json::to_string(&harness.project),
                "serialize corrupt registry project path",
            );
            format!(
                r#"{{"version":1,"project_root":{project_root},"owner_nonce":"0123456789abcdef0123456789abcdef","session":"agent-terminal-mismatched-session","jobs":{{}}}}"#
            )
            .into_bytes()
        }
    };
    must(
        fs::write(project_state_dir(harness).join("state.json"), contents),
        "write corrupt harness state",
    );
}

/// Creates an empty Git root and nested `sub` directory.
#[must_use]
pub fn init_git_project() -> (TempDir, PathBuf, PathBuf) {
    let root = must(TempDir::new(), "create Git fixture temporary directory");
    let git_root = root.path().to_path_buf();
    let subdir = git_root.join("sub");
    must(
        fs::create_dir(git_root.join(".git")),
        "create .git directory",
    );
    must(fs::create_dir(&subdir), "create Git fixture subdirectory");
    (root, git_root, subdir)
}

fn project_state_dir(harness: &DeterministicHarness) -> PathBuf {
    let projects_dir = harness.state_dir.join("projects");
    let mut project_dirs = child_directories(&projects_dir);
    if project_dirs.is_empty() {
        let output = harness.run(&["list"]);
        assert!(
            output.status.success(),
            "could not initialize harness state; {}",
            output_context(&output)
        );
        project_dirs = child_directories(&projects_dir);
    }
    assert_eq!(
        project_dirs.len(),
        1,
        "expected exactly one project state directory under {}",
        projects_dir.display()
    );
    project_dirs.pop().unwrap_or(projects_dir)
}

fn child_directories(parent: &Path) -> Vec<PathBuf> {
    if !parent.is_dir() {
        return Vec::new();
    }
    let entries = must(
        must(fs::read_dir(parent), "read project state directory").collect::<Result<Vec<_>, _>>(),
        "read project state directory entry",
    );
    entries
        .into_iter()
        .filter_map(|entry| {
            must(entry.file_type(), "read project state entry type")
                .is_dir()
                .then(|| entry.path())
        })
        .collect()
}

fn output_context(output: &Output) -> String {
    format!(
        "exit={:?}, stdout={:?}, stderr={:?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn must<T, E: Display>(result: Result<T, E>, action: &str) -> T {
    if let Err(error) = &result {
        assert!(result.is_ok(), "{action}: {error}");
    }
    result.unwrap_or_else(|_| std::process::abort())
}
