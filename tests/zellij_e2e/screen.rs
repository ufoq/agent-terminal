use serde_json::Value;

use super::harness::Harness;

// Mirrors the private screen capture limit in src/controller.rs.
const BOUNDED_SCREEN_BYTES: usize = 32 * 1024;

#[test]
fn carriage_return_updates_rendered_screen() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("carriage-return", "printf 'abc\\rXYZ\\n'")?;

    let read = harness.read_until("carriage-return", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("XYZ"))
    })?;
    let screen = read["data"]["screen"].as_str().unwrap_or_default();
    assert!(screen.contains("XYZ"));
    assert!(!screen.as_bytes().contains(&b'\r'));
    assert!(!screen.as_bytes().contains(&0x1b));
    Ok(())
}

#[test]
fn erase_and_clear_sequences_do_not_leak_ansi() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("erase-clear", "printf 'obsolete\\033[1G\\033[2Kfinal\\n'")?;

    let read = harness.read_until("erase-clear", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("final"))
    })?;
    let screen = read["data"]["screen"].as_str().unwrap_or_default();
    assert!(screen.contains("final"));
    assert!(!screen.as_bytes().contains(&0x1b));
    Ok(())
}

#[test]
fn stdout_and_stderr_share_the_visible_pty_screen() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok(
        "stdout-stderr",
        "printf 'STDOUT-MARKER\\n'; printf 'STDERR-MARKER\\n' >&2",
    )?;

    let read = harness.read_until("stdout-stderr", |body| {
        body["data"]["screen"].as_str().is_some_and(|screen| {
            screen.contains("STDOUT-MARKER") && screen.contains("STDERR-MARKER")
        })
    })?;
    let screen = read["data"]["screen"].as_str().unwrap_or_default();
    assert!(screen.contains("STDOUT-MARKER"));
    assert!(screen.contains("STDERR-MARKER"));
    Ok(())
}

#[test]
fn unterminated_last_line_is_readable() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("unterminated", "printf 'no-newline'")?;

    let read = harness.read_until("unterminated", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("no-newline"))
    })?;
    let screen = read["data"]["screen"].as_str().unwrap_or_default();
    assert!(screen.contains("no-newline"));
    Ok(())
}

#[test]
fn long_single_line_is_byte_bounded_at_utf8_boundary() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("long-line", "printf '%40000sTAIL' '' | tr ' ' x")?;

    let read = harness.read_until("long-line", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("TAIL"))
    })?;
    let screen = read["data"]["screen"].as_str().unwrap_or_default();
    assert_eq!(read["data"]["truncated"], true);
    assert!(screen.len() <= BOUNDED_SCREEN_BYTES);
    assert!(screen.contains("TAIL"));
    Ok(())
}

#[test]
fn multibyte_output_near_byte_limit_remains_valid_utf8() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok(
        "multibyte",
        "i=0; while [ \"$i\" -lt 11000 ]; do printf '界'; i=$((i + 1)); done; printf '終'",
    )?;
    harness.read_until("multibyte", |body| {
        body["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains('終'))
    })?;

    let read = harness.run_ok(&["read", "multibyte"])?;
    let screen = read["data"]["screen"].as_str().unwrap_or_default();
    assert!(screen.len() <= BOUNDED_SCREEN_BYTES);
    assert!(screen.contains('終'));
    Ok(())
}

#[test]
fn invalid_utf8_output_is_lossily_readable() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("invalid-utf8", "/usr/bin/printf 'BEGIN-\\xffA\\xfe-END\\n'")?;
    harness.read_until("invalid-utf8", |body| {
        body["data"]["screen"].as_str().is_some_and(|screen| {
            screen.contains("BEGIN-") && screen.contains('A') && screen.contains("-END")
        })
    })?;

    let output = harness.run(&["read", "invalid-utf8"])?;
    let read: Value = serde_json::from_slice(&output.stdout)?;
    assert!(output.status.success(), "{read}");
    let screen = read["data"]["screen"].as_str().unwrap_or_default();
    assert!(screen.contains("BEGIN-"));
    assert!(screen.contains('A'));
    assert!(screen.contains("-END"));
    Ok(())
}

#[test]
fn osc_title_or_hyperlink_sequences_do_not_break_ownership()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    harness.start_ok("osc-title", "printf '\\033]2;spoof\\007PAYLOAD\\n'; exit 0")?;

    let read = harness.read_until("osc-title", |body| {
        body["data"]["state"] == "exited"
            && body["data"]["screen"]
                .as_str()
                .is_some_and(|screen| screen.contains("PAYLOAD"))
    })?;
    assert_eq!(read["data"]["state"], "exited");
    assert!(
        read["data"]["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains("PAYLOAD"))
    );
    Ok(())
}
