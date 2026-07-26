use std::{
    sync::Barrier,
    time::{Duration, Instant},
};

use serde_json::Value;
use tempfile::TempDir;

use super::harness::{Harness, serial_guard};

#[test]
fn concurrent_starts_share_the_bootstrap_lock() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = serial_guard();
    let shared = TempDir::new()?;
    let state_root = shared.path().join("state");
    let socket_dir = shared.path().join("socket");
    let first = Harness::with_state_root(Some(&state_root), Some(&socket_dir))?;
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

    let list_body = harness.run_ok(&["list"])?;
    assert_eq!(list_body["data"]["jobs"].as_array().map(Vec::len), Some(1));
    assert_eq!(list_body["data"]["jobs"][0]["job"], "race");
    harness.run_ok(&["stop", "race", "--force"])?;
    Ok(())
}

#[test]
fn concurrent_stop_and_read_do_not_corrupt() -> Result<(), Box<dyn std::error::Error>> {
    let _lock = serial_guard();
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
    let _lock = serial_guard();
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
