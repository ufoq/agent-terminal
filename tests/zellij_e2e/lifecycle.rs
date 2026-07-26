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

#[test]
fn immediate_zero_exit_has_readable_held_pane() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("zero", "exit 0")?;

    let read = harness.read_until("zero", |body| body["data"]["state"] == "exited")?;
    assert_eq!(read["data"]["state"], "exited");
    assert_eq!(read["data"]["exit_code"], 0);
    assert_eq!(read["data"]["screen_available"], true);
    Ok(())
}

#[test]
fn maximum_shell_exit_code_is_preserved() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("maximum", "exit 255")?;

    let read = harness.read_until("maximum", |body| body["data"]["state"] == "exited")?;
    assert_eq!(read["data"]["exit_code"], 255);
    Ok(())
}

#[test]
fn self_signal_trap_exit_is_observed() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok(
        "self-signal",
        "trap 'printf term-seen; exit 42' TERM; kill -TERM $$",
    )?;

    let read = harness.read_until("self-signal", |body| {
        body["data"]["state"] == "exited"
            && body["data"]["screen"]
                .as_str()
                .is_some_and(|screen| screen.contains("term-seen"))
    })?;
    assert_eq!(read["data"]["state"], "exited");
    assert_eq!(read["data"]["exit_code"], 42);
    Ok(())
}

#[test]
fn graceful_stop_captures_interrupt_trap_output() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok(
        "graceful",
        "trap 'printf interrupted; exit 0' INT; printf ready; while :; do sleep 1; done",
    )?;
    harness.read_until("graceful", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("ready"))
    })?;

    let stop = harness.run_ok(&["stop", "graceful"])?;
    assert_eq!(stop["data"]["forced"], false);
    assert!(
        stop["data"]["last_screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("interrupted"))
    );
    Ok(())
}

#[test]
fn forced_stop_captures_last_visible_screen() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok(
        "forced",
        "trap '' INT; printf before-force; while :; do sleep 1; done",
    )?;
    harness.read_until("forced", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("before-force"))
    })?;

    let stop = harness.run_ok(&["stop", "forced", "--force"])?;
    assert_eq!(stop["data"]["forced"], true);
    assert!(
        stop["data"]["last_screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("before-force"))
    );
    Ok(())
}

#[test]
fn list_reports_mixed_running_and_exited_jobs() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("live", "while :; do sleep 1; done")?;
    harness.start_ok("done", "exit 6")?;
    harness.read_until("done", |body| body["data"]["state"] == "exited")?;

    let list = harness.run_ok(&["list"])?;
    let jobs = list["data"]["jobs"]
        .as_array()
        .ok_or("list jobs are not an array")?;
    assert!(
        jobs.iter()
            .any(|job| job["job"] == "live" && job["state"] == "running")
    );
    assert!(
        jobs.iter().any(|job| {
            job["job"] == "done" && job["state"] == "exited" && job["exit_code"] == 6
        })
    );
    Ok(())
}

#[test]
fn command_stdin_and_stdout_are_ptys() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.run_ok(&[
        "start",
        "pty",
        "--",
        "/bin/sh",
        "-c",
        "if [ -t 0 ] && [ -t 1 ]; then printf pty=yes; exit 0; else printf pty=no; exit 1; fi",
    ])?;

    let read = harness.read_until("pty", |body| {
        body["data"]["state"] == "exited"
            && body["data"]["screen"]
                .as_str()
                .is_some_and(|screen| screen.contains("pty=yes"))
    })?;
    assert_eq!(read["data"]["exit_code"], 0);
    Ok(())
}

#[test]
fn held_exited_pane_screen_is_stable_across_reads() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("stable", "printf stable-marker; exit 4")?;

    let first = harness.read_until("stable", |body| {
        body["data"]["state"] == "exited"
            && body["data"]["screen"]
                .as_str()
                .is_some_and(|screen| screen.contains("stable-marker"))
    })?;
    let second = harness.run_ok(&["read", "stable"])?;
    assert_eq!(second["data"]["exit_code"], first["data"]["exit_code"]);
    assert_eq!(second["data"]["screen"], first["data"]["screen"]);
    Ok(())
}
