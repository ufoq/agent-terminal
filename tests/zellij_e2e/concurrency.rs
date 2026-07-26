use std::{
    sync::Barrier,
    time::{Duration, Instant},
};

use serde_json::Value;
use tempfile::TempDir;

use super::harness::{Harness, session_is_live, socket_guard};

#[test]
fn concurrent_starts_share_the_bootstrap_lock() -> Result<(), Box<dyn std::error::Error>> {
    let shared = TempDir::new()?;
    let state_root = shared.path().join("state");
    let socket_dir = shared.path().join("socket");
    let first = Harness::with_state_root(Some(&state_root), Some(&socket_dir))?;
    let _guard = socket_guard(&socket_dir);
    let second = Harness::with_state_root(Some(&state_root), Some(&socket_dir))?;
    let barrier = Barrier::new(3);

    let (first_output, second_output) = std::thread::scope(|scope| {
        let first_start = scope.spawn(|| {
            barrier.wait();
            first.start("first", "sleep 30")
        });
        let second_start = scope.spawn(|| {
            barrier.wait();
            second.start("second", "sleep 30")
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
    assert!(first_output.status.success());
    assert!(second_output.status.success());
    first.run_ok(&["stop", "first", "--force"])?;
    second.run_ok(&["stop", "second", "--force"])?;
    Ok(())
}

#[test]
fn concurrent_starts_to_same_job_serialize() -> Result<(), Box<dyn std::error::Error>> {
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

    let list_body = harness.run_ok(&["list"])?;
    assert_eq!(list_body["data"]["jobs"].as_array().map(Vec::len), Some(1));
    assert_eq!(list_body["data"]["jobs"][0]["job"], "race");
    harness.run_ok(&["stop", "race", "--force"])?;
    Ok(())
}

#[test]
fn concurrent_stop_and_read_do_not_corrupt() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    let start_body = harness.start_ok("race", "while :; do sleep 1; done")?;
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
                } else if matches!(
                    body["error"]["code"].as_str(),
                    Some("lock_busy" | "job_not_found")
                ) {
                    true
                } else {
                    return Err(std::io::Error::other(format!(
                        "unexpected concurrent read response: {body}"
                    )));
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
            assert!(
                matches!(
                    body["error"]["code"].as_str(),
                    Some("lock_busy" | "job_not_found")
                ),
                "{body}"
            );
        }
    }

    let list_body = harness.run_ok(&["list"])?;
    assert_eq!(list_body["data"]["jobs"], serde_json::json!([]));
    Ok(())
}

#[test]
fn concurrent_sends_to_same_job_serialize() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.run_ok(&[
        "start",
        "race",
        "--",
        "/bin/sh",
        "-c",
        "printf 'ready\n'; IFS= read -r first; printf 'accepted=<%s>\n' \"$first\"; IFS= read -r second; printf 'accepted=<%s>\n' \"$second\"; while :; do sleep 1; done",
    ])?;
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
    harness.run_ok(&["stop", "race", "--force"])?;
    Ok(())
}

#[test]
fn concurrent_start_and_read_are_linearizable() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    let barrier = Barrier::new(3);
    let (start_output, read_output) = std::thread::scope(|scope| {
        let start = scope.spawn(|| {
            barrier.wait();
            harness.run_retrying_lock(&[
                "start",
                "race",
                "--",
                "sh",
                "-c",
                "while :; do sleep 1; done",
            ])
        });
        let read = scope.spawn(|| {
            barrier.wait();
            harness.run(&["read", "race"])
        });
        barrier.wait();
        let start_output = start
            .join()
            .map_err(|_| std::io::Error::other("start thread panicked"))?;
        let read_output = read
            .join()
            .map_err(|_| std::io::Error::other("read thread panicked"))?;
        Ok::<_, std::io::Error>((start_output?, read_output?))
    })?;

    let start_body: Value = serde_json::from_slice(&start_output.stdout)?;
    assert!(start_output.status.success(), "{start_body}");
    assert_eq!(start_body["data"]["state"], "running");

    let read_body: Value = serde_json::from_slice(&read_output.stdout)?;
    if read_output.status.success() {
        assert_eq!(read_body["data"]["state"], "running", "{read_body}");
    } else {
        assert!(
            matches!(
                read_body["error"]["code"].as_str(),
                Some("job_not_found" | "lock_busy")
            ),
            "{read_body}"
        );
    }

    let final_read = harness.read_until("race", |body| body["data"]["state"] == "running")?;
    assert_eq!(final_read["data"]["state"], "running");
    harness.run_ok(&["stop", "race", "--force"])?;
    Ok(())
}

#[test]
fn concurrent_start_and_stop_leave_valid_reconcilable_state()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    let barrier = Barrier::new(3);
    let (start_output, stop_output) = std::thread::scope(|scope| {
        let start = scope.spawn(|| {
            barrier.wait();
            harness.run_retrying_lock(&[
                "start",
                "race",
                "--",
                "sh",
                "-c",
                "while :; do sleep 1; done",
            ])
        });
        let stop = scope.spawn(|| {
            barrier.wait();
            harness.run_retrying_lock(&["stop", "race", "--force"])
        });
        barrier.wait();
        let start_output = start
            .join()
            .map_err(|_| std::io::Error::other("start thread panicked"))?;
        let stop_output = stop
            .join()
            .map_err(|_| std::io::Error::other("stop thread panicked"))?;
        Ok::<_, std::io::Error>((start_output?, stop_output?))
    })?;
    let start_body: Value = serde_json::from_slice(&start_output.stdout)?;
    assert!(start_output.status.success(), "{start_body}");
    assert_eq!(start_body["data"]["state"], "running");

    let stop_body: Value = serde_json::from_slice(&stop_output.stdout)?;
    if stop_output.status.success() {
        assert_eq!(stop_body["data"]["cleaned_up"], true, "{stop_body}");
    } else {
        assert_eq!(stop_body["error"]["code"], "job_not_found", "{stop_body}");
    }

    let list_body = harness.run_ok(&["list"])?;
    let jobs = list_body["data"]["jobs"]
        .as_array()
        .ok_or("list jobs was not an array")?;
    assert!(jobs.len() <= 1, "{list_body}");
    if let Some(job) = jobs.first() {
        assert_eq!(job["job"], "race", "{list_body}");
        assert_eq!(job["state"], "running", "{list_body}");
        harness.run_ok(&["stop", "race", "--force"])?;
    }
    Ok(())
}

#[test]
fn concurrent_send_and_force_stop_do_not_corrupt_state() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("race", "printf 'ready\n'; while :; do sleep 1; done")?;
    harness.read_until("race", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("ready"))
    })?;

    let barrier = Barrier::new(3);
    let (send_output, stop_output) = std::thread::scope(|scope| {
        let send = scope.spawn(|| {
            barrier.wait();
            harness.run(&["send", "race", "--", "payload"])
        });
        let stop = scope.spawn(|| {
            barrier.wait();
            harness.run_retrying_lock(&["stop", "race", "--force"])
        });
        barrier.wait();
        let send_output = send
            .join()
            .map_err(|_| std::io::Error::other("send thread panicked"))?;
        let stop_output = stop
            .join()
            .map_err(|_| std::io::Error::other("stop thread panicked"))?;
        Ok::<_, std::io::Error>((send_output?, stop_output?))
    })?;

    let send_body: Value = serde_json::from_slice(&send_output.stdout)?;
    if send_output.status.success() {
        assert_eq!(send_body["data"]["submitted"], true, "{send_body}");
    } else {
        assert!(
            matches!(
                send_body["error"]["code"].as_str(),
                Some("job_not_running" | "job_not_found" | "lock_busy")
            ),
            "{send_body}"
        );
    }
    let stop_body: Value = serde_json::from_slice(&stop_output.stdout)?;
    assert!(stop_output.status.success(), "{stop_body}");
    assert_eq!(stop_body["data"]["cleaned_up"], true);

    let list_body = harness.run_ok(&["list"])?;
    assert_eq!(list_body["data"]["jobs"], serde_json::json!([]));
    Ok(())
}

#[test]
fn concurrent_press_and_force_stop_do_not_corrupt_state() -> Result<(), Box<dyn std::error::Error>>
{
    let harness = Harness::new()?;
    harness.start_ok("race", "printf 'ready\n'; while :; do sleep 1; done")?;
    harness.read_until("race", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("ready"))
    })?;

    let barrier = Barrier::new(3);
    let (press_output, stop_output) = std::thread::scope(|scope| {
        let press = scope.spawn(|| {
            barrier.wait();
            harness.run(&["press", "race", "--", "Enter"])
        });
        let stop = scope.spawn(|| {
            barrier.wait();
            harness.run_retrying_lock(&["stop", "race", "--force"])
        });
        barrier.wait();
        let press_output = press
            .join()
            .map_err(|_| std::io::Error::other("press thread panicked"))?;
        let stop_output = stop
            .join()
            .map_err(|_| std::io::Error::other("stop thread panicked"))?;
        Ok::<_, std::io::Error>((press_output?, stop_output?))
    })?;

    let press_body: Value = serde_json::from_slice(&press_output.stdout)?;
    if press_output.status.success() {
        assert_eq!(press_body["data"]["keys"][0], "Enter", "{press_body}");
    } else {
        assert!(
            matches!(
                press_body["error"]["code"].as_str(),
                Some("job_not_running" | "job_not_found" | "lock_busy")
            ),
            "{press_body}"
        );
    }
    let stop_body: Value = serde_json::from_slice(&stop_output.stdout)?;
    assert!(stop_output.status.success(), "{stop_body}");
    assert_eq!(stop_body["data"]["cleaned_up"], true);

    let list_body = harness.run_ok(&["list"])?;
    assert_eq!(list_body["data"]["jobs"], serde_json::json!([]));
    Ok(())
}

#[test]
fn concurrent_stops_have_exactly_one_cleanup_winner() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("race", "printf 'ready\n'; while :; do sleep 1; done")?;
    harness.read_until("race", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("ready"))
    })?;
    let sessions = harness.sessions();
    assert_eq!(sessions.len(), 1);

    let barrier = Barrier::new(3);
    let (first_stop, second_stop) = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            barrier.wait();
            harness.run(&["stop", "race", "--force"])
        });
        let second = scope.spawn(|| {
            barrier.wait();
            harness.run(&["stop", "race", "--force"])
        });
        barrier.wait();
        let first_stop = first
            .join()
            .map_err(|_| std::io::Error::other("first stop thread panicked"))?;
        let second_stop = second
            .join()
            .map_err(|_| std::io::Error::other("second stop thread panicked"))?;
        Ok::<_, std::io::Error>((first_stop?, second_stop?))
    })?;
    let first_body: Value = serde_json::from_slice(&first_stop.stdout)?;
    let second_body: Value = serde_json::from_slice(&second_stop.stdout)?;
    let stops = [(&first_stop, &first_body), (&second_stop, &second_body)];
    assert_eq!(
        stops
            .iter()
            .filter(|(output, _body)| output.status.success())
            .count(),
        1,
        "first={first_body}, second={second_body}"
    );
    for (output, body) in stops {
        if output.status.success() {
            assert_eq!(body["data"]["cleaned_up"], true, "{body}");
        } else {
            assert!(
                matches!(
                    body["error"]["code"].as_str(),
                    Some("job_not_found" | "lock_busy")
                ),
                "{body}"
            );
        }
    }

    let list_body = harness.run_ok(&["list"])?;
    assert_eq!(list_body["data"]["jobs"], serde_json::json!([]));
    for session in sessions {
        assert!(!session_is_live(&harness.socket_dir, &session)?);
    }
    Ok(())
}

#[test]
fn concurrent_no_submit_pastes_are_individually_atomic() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok(
        "race",
        "printf 'ready\n'; IFS= read -r line; printf 'accepted=<%s>\n' \"$line\"",
    )?;
    harness.read_until("race", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("ready"))
    })?;

    let barrier = Barrier::new(3);
    let (first_send, second_send) = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            barrier.wait();
            harness.run_retrying_lock(&["send", "race", "--no-submit", "--", "AAA"])
        });
        let second = scope.spawn(|| {
            barrier.wait();
            harness.run_retrying_lock(&["send", "race", "--no-submit", "--", "BBB"])
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
        assert!(output.status.success(), "{body}");
        assert_eq!(body["data"]["submitted"], false, "{body}");
    }

    harness.run_ok(&["press", "race", "--", "Enter"])?;
    let read = harness.read_until("race", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("accepted=<"))
    })?;
    let Some(screen) = read["data"]["screen"].as_str() else {
        return Err("race screen was unavailable after concurrent pastes".into());
    };
    assert!(
        screen
            .lines()
            .any(|line| matches!(line, "accepted=<AAABBB>" | "accepted=<BBBAAA>")),
        "{screen}"
    );
    assert_eq!(screen.matches("accepted=<").count(), 1, "{screen}");
    harness.run_ok(&["stop", "race", "--force"])?;
    Ok(())
}

#[test]
fn concurrent_sends_to_different_jobs_in_shared_session_are_isolated()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    let _guard = socket_guard(&harness.socket_dir);
    harness.start_ok(
        "a",
        "stty -echo; printf 'ready-a\n'; IFS= read -r line; stty echo; printf 'accepted-a=<%s>\n' \"$line\"; while :; do sleep 1; done",
    )?;
    harness.start_ok(
        "b",
        "stty -echo; printf 'ready-b\n'; IFS= read -r line; stty echo; printf 'accepted-b=<%s>\n' \"$line\"; while :; do sleep 1; done",
    )?;
    harness.read_until("a", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("ready-a"))
    })?;
    harness.read_until("b", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("ready-b"))
    })?;
    assert_eq!(harness.sessions().len(), 1);

    let barrier = Barrier::new(3);
    let (send_a, send_b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            harness.run_retrying_lock(&["send", "a", "--", "TOKEN-A"])
        });
        let b = scope.spawn(|| {
            barrier.wait();
            harness.run_retrying_lock(&["send", "b", "--", "TOKEN-B"])
        });
        barrier.wait();
        let send_a = a
            .join()
            .map_err(|_| std::io::Error::other("job a send thread panicked"))?;
        let send_b = b
            .join()
            .map_err(|_| std::io::Error::other("job b send thread panicked"))?;
        Ok::<_, std::io::Error>((send_a?, send_b?))
    })?;
    let alpha_response: Value = serde_json::from_slice(&send_a.stdout)?;
    let bravo_response: Value = serde_json::from_slice(&send_b.stdout)?;
    assert!(send_a.status.success(), "{alpha_response}");
    assert!(send_b.status.success(), "{bravo_response}");

    let read_a = harness.read_until("a", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("accepted-a=<TOKEN-A>"))
    })?;
    let read_b = harness.read_until("b", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("accepted-b=<TOKEN-B>"))
    })?;
    let Some(screen_a) = read_a["data"]["screen"].as_str() else {
        return Err("job a screen was unavailable after send".into());
    };
    let Some(screen_b) = read_b["data"]["screen"].as_str() else {
        return Err("job b screen was unavailable after send".into());
    };
    assert_eq!(screen_a.matches("TOKEN-A").count(), 1, "{screen_a}");
    assert_eq!(screen_a.matches("TOKEN-B").count(), 0, "{screen_a}");
    assert_eq!(screen_b.matches("TOKEN-B").count(), 1, "{screen_b}");
    assert_eq!(screen_b.matches("TOKEN-A").count(), 0, "{screen_b}");

    harness.run_ok(&["stop", "a", "--force"])?;
    harness.run_ok(&["stop", "b", "--force"])?;
    Ok(())
}
