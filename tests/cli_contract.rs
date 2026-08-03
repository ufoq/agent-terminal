#![allow(clippy::disallowed_methods)]

use std::{fs, process::Command};

use serde_json::{Value, json};
use tempfile::TempDir;

fn binary() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("agent-terminal"))
}

fn create_project(temp: &TempDir) -> Result<std::path::PathBuf, std::io::Error> {
    let project = temp.path().join("project");
    fs::create_dir_all(&project)?;
    project.canonicalize()
}

#[test]
fn help_is_plain_text_and_names_only_the_six_commands() -> Result<(), Box<dyn std::error::Error>> {
    let output = binary().arg("--help").output()?;
    let stdout = String::from_utf8(output.stdout)?;

    assert!(output.status.success());
    assert!(stdout.contains("Usage:"));
    for command in ["start", "read", "send", "press", "stop", "list"] {
        assert!(stdout.contains(command), "help omitted {command}");
    }
    assert!(!stdout.contains("wait"));
    assert!(!stdout.contains("cleanup"));
    Ok(())
}

#[test]
fn invalid_command_is_one_json_error_on_stdout() -> Result<(), Box<dyn std::error::Error>> {
    let output = binary().arg("unknown").output()?;
    let body: Value = serde_json::from_slice(&output.stdout)?;

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        body,
        json!({
            "status": "error",
            "code": "invalid_input",
            "message": body["message"]
        })
    );
    assert_eq!(String::from_utf8(output.stdout.clone())?.lines().count(), 1);
    assert!(output.stderr.is_empty());
    Ok(())
}

#[test]
fn empty_list_succeeds_without_starting_zellij() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let project = create_project(&temp)?;
    let output = binary()
        .args([
            "--state-dir",
            temp.path()
                .join("state")
                .to_str()
                .ok_or("non-UTF-8 state path")?,
            "--project",
            project.to_str().ok_or("non-UTF-8 project path")?,
            "list",
        ])
        .env("PATH", "/nonexistent")
        .output()?;

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout)?,
        json!({"status":"ok","jobs":[]})
    );
    Ok(())
}

#[test]
fn version_is_plain_text() -> Result<(), Box<dyn std::error::Error>> {
    let output = binary().arg("--version").output()?;
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout)?.starts_with("agent-terminal 0.1.0"));
    Ok(())
}

#[test]
fn press_requires_the_argument_separator() -> Result<(), Box<dyn std::error::Error>> {
    let without_separator = binary().args(["press", "job", "Enter"]).output()?;
    let with_separator = binary().args(["press", "job", "--", "Enter"]).output()?;

    assert_eq!(without_separator.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<Value>(&without_separator.stdout)?["code"],
        "invalid_input"
    );
    assert_ne!(with_separator.status.code(), Some(2));
    Ok(())
}
