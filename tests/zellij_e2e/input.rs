use serde_json::Value;

use super::harness::Harness;

const ONE_LINE_READER: &str =
    "printf 'ready\n'; IFS= read -r line; printf 'accepted=<%s>\n' \"$line\"";

fn screen(body: &Value) -> &str {
    body["data"]["screen"].as_str().unwrap_or_default()
}

fn screen_contains(body: &Value, expected: &str) -> bool {
    screen(body).contains(expected)
}

fn start_reader(harness: &Harness, job: &str) -> Result<(), Box<dyn std::error::Error>> {
    harness.start_ok(job, ONE_LINE_READER)?;
    harness.read_until(job, |body| screen_contains(body, "ready"))?;
    Ok(())
}

#[test]
fn send_preserves_shell_metacharacters_literally() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    let payload = "a b;$HOME|$(id)&*";
    start_reader(&harness, "metacharacters")?;

    harness.run_ok(&["send", "metacharacters", "--", payload])?;

    let read = harness.read_until("metacharacters", |body| {
        screen_contains(body, "accepted=<a b;$HOME|$(id)&*>")
    })?;
    assert!(screen(&read).contains("accepted=<a b;$HOME|$(id)&*>"));
    Ok(())
}

#[test]
fn empty_send_is_rejected_without_affecting_running_job() -> Result<(), Box<dyn std::error::Error>>
{
    let harness = Harness::new()?;
    start_reader(&harness, "empty")?;

    let rejected = harness.run(&["send", "empty", "--", ""])?;
    let rejected_body: Value = serde_json::from_slice(&rejected.stdout)?;
    assert_eq!(rejected.status.code(), Some(2));
    assert_eq!(rejected_body["error"]["code"], "invalid_input");

    let read = harness.run_ok(&["read", "empty"])?;
    assert_eq!(read["data"]["state"], "running");
    assert!(!screen(&read).contains("accepted=<"));
    harness.run_ok(&["stop", "empty", "--force"])?;
    Ok(())
}

#[test]
fn multiline_send_submits_each_embedded_line() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok(
        "multiline",
        "IFS= read -r first; IFS= read -r second; printf 'first=<%s> second=<%s>\n' \"$first\" \"$second\"",
    )?;

    harness.run_ok(&["send", "multiline", "--", "one\ntwo"])?;

    let read = harness.read_until("multiline", |body| {
        screen_contains(body, "first=<one> second=<two>")
    })?;
    assert!(screen(&read).contains("first=<one> second=<two>"));
    Ok(())
}

#[test]
fn multiline_no_submit_only_submits_embedded_newlines() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok(
        "multiline-no-submit",
        "IFS= read -r first; printf 'first=<%s>\n' \"$first\"; IFS= read -r second; printf 'second=<%s>\n' \"$second\"",
    )?;

    harness.run_ok(&[
        "send",
        "multiline-no-submit",
        "--no-submit",
        "--",
        "one\ntwo",
    ])?;
    let first = harness.read_until("multiline-no-submit", |body| {
        screen_contains(body, "first=<one>")
    })?;
    assert!(!screen(&first).contains("second=<two>"));

    harness.run_ok(&["press", "multiline-no-submit", "--", "Enter"])?;
    let second = harness.read_until("multiline-no-submit", |body| {
        screen_contains(body, "second=<two>")
    })?;
    assert!(screen(&second).contains("second=<two>"));
    Ok(())
}

#[test]
fn unicode_paste_round_trips_through_pty() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    let payload = "한글 café 🦀 é";
    let expected = "accepted=<한글 café 🦀 é>";
    start_reader(&harness, "unicode")?;

    harness.run_ok(&["send", "unicode", "--", payload])?;

    let read = harness.read_until("unicode", |body| body["data"]["state"] == "exited")?;
    assert_eq!(read["data"]["exit_code"], 0);
    assert!(
        screen(&read).contains(expected),
        "screen={:?}",
        screen(&read)
    );
    Ok(())
}

#[test]
fn tabs_and_surrounding_spaces_are_preserved() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok(
        "whitespace",
        "IFS= read -r line; printf '%s' \"$line\" | od -An -tx1",
    )?;

    harness.run_ok(&["send", "whitespace", "--", " a\tb "])?;

    let read = harness.read_until("whitespace", |body| screen_contains(body, "20 61 09 62 20"))?;
    assert!(screen(&read).contains("20 61 09 62 20"));
    Ok(())
}

#[test]
fn large_no_submit_paste_reaches_raw_pty_intact() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok(
        "large-paste",
        "printf 'ready\n'; stty raw -echo; bytes=$(dd bs=1 count=8192 2>/dev/null | wc -c); stty sane; printf 'bytes=<%d>\n' \"$bytes\"",
    )?;
    harness.read_until("large-paste", |body| screen_contains(body, "ready"))?;
    let payload = "x".repeat(8192);

    let sent = harness.run_retrying_lock(&[
        "send",
        "large-paste",
        "--no-submit",
        "--",
        payload.as_str(),
    ])?;
    let sent_body: Value = serde_json::from_slice(&sent.stdout)?;
    assert!(sent.status.success(), "{sent_body}");
    assert_eq!(sent_body["data"]["submitted"], false);

    let read = harness.read_until("large-paste", |body| screen_contains(body, "bytes=<8192>"))?;
    assert!(screen(&read).contains("bytes=<8192>"));
    Ok(())
}

#[test]
fn sequential_no_submit_fragments_form_one_line() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    start_reader(&harness, "fragments")?;

    harness.run_ok(&["send", "fragments", "--no-submit", "--", "alpha"])?;
    harness.run_ok(&["send", "fragments", "--no-submit", "--", "beta"])?;
    harness.run_ok(&["press", "fragments", "--", "Enter"])?;

    let read = harness.read_until("fragments", |body| {
        screen_contains(body, "accepted=<alphabeta>")
    })?;
    assert!(screen(&read).contains("accepted=<alphabeta>"));
    Ok(())
}

#[test]
fn no_submit_does_not_complete_canonical_read() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    start_reader(&harness, "canonical")?;

    harness.run_ok(&["send", "canonical", "--no-submit", "--", "pending"])?;
    let intermediate = harness.run_ok(&["read", "canonical"])?;
    assert_eq!(intermediate["data"]["state"], "running");
    assert!(!screen(&intermediate).contains("accepted=<pending>"));

    harness.run_ok(&["press", "canonical", "--", "Enter"])?;
    let completed = harness.read_until("canonical", |body| {
        screen_contains(body, "accepted=<pending>")
    })?;
    assert!(screen(&completed).contains("accepted=<pending>"));
    Ok(())
}

#[test]
fn ctrl_d_delivers_terminal_eof() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("eof", "printf 'ready\n'; cat; printf 'eof-seen\n'")?;
    harness.read_until("eof", |body| screen_contains(body, "ready"))?;

    harness.run_ok(&["press", "eof", "--", "Ctrl+d"])?;

    let read = harness.read_until("eof", |body| {
        body["data"]["state"] == "exited" && screen_contains(body, "eof-seen")
    })?;
    assert_eq!(read["data"]["exit_code"], 0);
    assert!(screen(&read).contains("eof-seen"));
    Ok(())
}

#[test]
fn ctrl_u_uses_canonical_line_kill_behavior() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    start_reader(&harness, "line-kill")?;

    harness.run_ok(&["send", "line-kill", "--no-submit", "--", "wrong"])?;
    harness.run_ok(&["press", "line-kill", "--", "Ctrl+u"])?;
    harness.run_ok(&["send", "line-kill", "--", "right"])?;

    let read = harness.read_until("line-kill", |body| {
        screen_contains(body, "accepted=<right>")
    })?;
    assert!(screen(&read).contains("accepted=<right>"));
    Ok(())
}
