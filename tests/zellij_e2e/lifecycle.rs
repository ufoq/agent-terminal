use std::fs;

use serde_json::Value;

use super::harness::{Harness, session_is_live};

#[test]
fn start_read_stop_when_job_is_running() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    let start_body = harness.start_ok(
        "server",
        "printf 'ready\\n'; trap 'exit 0' INT; while :; do sleep 1; done",
    )?;
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

    let stop_body = harness.run_ok(&["stop", "server"])?;
    assert_eq!(stop_body["data"]["cleaned_up"], true);
    assert_eq!(stop_body["data"]["forced"], false);
    for session in harness.sessions() {
        assert!(!session_is_live(&harness.socket_dir, &session)?);
    }
    Ok(())
}

#[test]
fn send_and_press_when_job_is_interactive() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok(
        "prompt",
        "IFS= read -r first; printf 'text:%s\\n' \"$first\"; IFS= read -r second; printf 'key:%s\\n' \"$second\"",
    )?;

    harness.run_ok(&["send", "prompt", "--", "hello world"])?;
    harness.run_ok(&["press", "prompt", "--", "Enter"])?;
    let read = harness.read_until("prompt", |body| {
        body["data"]["state"] == "exited"
            && body["data"]["screen"].as_str().is_some_and(|screen| {
                screen.contains("text:hello world") && screen.contains("key:")
            })
    })?;
    assert_eq!(read["data"]["exit_code"], 0);
    harness.run_ok(&["stop", "prompt"])?;
    Ok(())
}

#[test]
fn fast_failure_when_command_exits_immediately() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    let start_body = harness.start_ok("tests", "printf 'boom\\n'; exit 7")?;
    assert_eq!(start_body["data"]["state"], "exited");
    assert_eq!(start_body["data"]["exit_code"], 7);

    let read = harness.read_until("tests", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("boom"))
    })?;
    assert_eq!(read["data"]["state"], "exited");
    assert_eq!(read["data"]["exit_code"], 7);
    harness.run_ok(&["stop", "tests"])?;
    Ok(())
}

#[test]
fn cwd_and_argv_preserve_spaces_and_metacharacters() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    let subdir = harness.project.join("sub dir");
    fs::create_dir(&subdir)?;
    let canonical_subdir = subdir.canonicalize()?;
    let cwd = canonical_subdir.to_string_lossy();
    harness.run_ok(&[
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

    let read = harness.read_until("argv", |body| {
        body["data"]["screen"].as_str().is_some_and(|screen| {
            screen.contains(cwd.as_ref()) && screen.contains("arg=<a b;$HOME>")
        })
    })?;
    let screen = read["data"]["screen"].as_str().unwrap_or_default();
    assert!(screen.contains(&format!("cwd=<{}>", canonical_subdir.display())));
    assert!(screen.contains("arg=<a b;$HOME>"));
    harness.run_ok(&["stop", "argv"])?;
    Ok(())
}

#[test]
fn send_no_submit_then_press_submits() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.run_ok(&[
        "start",
        "job",
        "--",
        "/bin/sh",
        "-c",
        "printf 'armed\\n'; IFS= read -r line; printf 'accepted=<%s>\\n' \"$line\"",
    ])?;
    harness.read_until("job", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("armed"))
    })?;

    let first_send_body = harness.run_ok(&["send", "job", "--no-submit", "--", "abx"])?;
    assert_eq!(first_send_body["data"]["submitted"], false);

    let backspace_body = harness.run_ok(&["press", "job", "--", "Backspace"])?;
    assert_eq!(backspace_body["data"]["keys"][0], "Backspace");

    let second_send_body = harness.run_ok(&["send", "job", "--no-submit", "--", "c"])?;
    assert_eq!(second_send_body["data"]["submitted"], false);

    let enter_body = harness.run_ok(&["press", "job", "--", "Enter"])?;
    assert_eq!(enter_body["data"]["keys"][0], "Enter");

    harness.read_until("job", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("accepted=<abc>"))
    })?;
    harness.run_ok(&["stop", "job"])?;
    Ok(())
}

#[test]
fn input_to_exited_job_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.run_ok(&[
        "start",
        "done",
        "--",
        "/bin/sh",
        "-c",
        "printf done; exit 0",
    ])?;
    harness.read_until("done", |body| body["data"]["state"] == "exited")?;

    let send = harness.run(&["send", "done", "--", "text"])?;
    let send_body: Value = serde_json::from_slice(&send.stdout)?;
    assert_eq!(send.status.code(), Some(1));
    assert_eq!(send_body["error"]["code"], "job_not_running");

    let press = harness.run(&["press", "done", "--", "Enter"])?;
    let press_body: Value = serde_json::from_slice(&press.stdout)?;
    assert_eq!(press.status.code(), Some(1));
    assert_eq!(press_body["error"]["code"], "job_not_running");

    let stop_body = harness.run_ok(&["stop", "done"])?;
    assert_eq!(stop_body["data"]["cleaned_up"], true);
    Ok(())
}

#[test]
fn graceful_stop_refuses_then_force_succeeds() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.run_ok(&[
        "start",
        "stubborn",
        "--",
        "/bin/sh",
        "-c",
        "trap '' INT; printf ready; while :; do sleep 1; done",
    ])?;
    harness.read_until("stubborn", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("ready"))
    })?;

    let graceful = harness.run(&["stop", "stubborn"])?;
    let graceful_body: Value = serde_json::from_slice(&graceful.stdout)?;
    assert_eq!(graceful.status.code(), Some(1), "{graceful_body}");
    assert_eq!(graceful_body["error"]["code"], "job_still_running");

    let read_body = harness.run_ok(&["read", "stubborn"])?;
    assert_eq!(read_body["data"]["state"], "running");

    let forced_body = harness.run_ok(&["stop", "stubborn", "--force"])?;
    assert_eq!(forced_body["data"]["cleaned_up"], true);
    assert_eq!(forced_body["data"]["forced"], true);
    Ok(())
}

#[test]
fn stop_already_exited_job_is_not_forced() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.run_ok(&[
        "start",
        "done",
        "--",
        "/bin/sh",
        "-c",
        "printf final; exit 0",
    ])?;
    harness.read_until("done", |body| body["data"]["state"] == "exited")?;

    let stop_body = harness.run_ok(&["stop", "done", "--force"])?;
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
fn external_session_loss_reports_lost() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("lost-job", "while :; do sleep 1; done")?;
    let running_body = harness.run_ok(&["read", "lost-job"])?;
    assert_eq!(running_body["data"]["state"], "running");

    harness.kill_session_externally()?;

    let lost_body = harness.run_ok(&["read", "lost-job"])?;
    assert_eq!(lost_body["data"]["state"], "lost");
    assert_eq!(lost_body["data"]["screen_available"], false);

    let send = harness.run(&["send", "lost-job", "--", "text"])?;
    let send_body: Value = serde_json::from_slice(&send.stdout)?;
    assert_eq!(send.status.code(), Some(1));
    assert_eq!(send_body["error"]["code"], "job_not_running");

    let stop_body = harness.run_ok(&["stop", "lost-job"])?;
    assert_eq!(stop_body["data"]["cleaned_up"], true);
    assert_eq!(stop_body["data"]["forced"], false);
    Ok(())
}

#[test]
fn screen_is_ansi_stripped_utf8_safe_and_bounded() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.run_ok(&[
        "start",
        "screen",
        "--",
        "/bin/sh",
        "-c",
        "i=1; while [ \"$i\" -le 205 ]; do printf 'line-%03d\\n' \"$i\"; i=$((i + 1)); done; printf '\\033[31mRED\\033[0m\\n한글\\n'",
    ])?;

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
    harness.run_ok(&["stop", "screen"])?;
    Ok(())
}
