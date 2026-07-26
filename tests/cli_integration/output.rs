use std::io;

use serde_json::Value;

use super::TestResult;
use crate::support::cli_harness::{DeterministicHarness, assert_json_ok};

#[test]
fn pretty_and_verbose_preserve_json_semantics() {
    let harness = DeterministicHarness::new();
    let compact = assert_json_ok(&harness.run(&["list"]));
    let pretty = assert_json_ok(&harness.run(&["--pretty", "list"]));
    let verbose = assert_json_ok(&harness.run(&["-vv", "list"]));
    assert_eq!(compact, pretty);
    assert_eq!(compact, verbose);
}

#[test]
fn compact_success_is_single_newline_terminated_json() -> TestResult {
    let harness = DeterministicHarness::new();

    let output = harness.run(&["list"]);

    let _response = assert_json_ok(&output);
    let (last, body) = output
        .stdout
        .split_last()
        .ok_or_else(|| io::Error::other("compact response was empty"))?;
    assert_eq!(*last, b'\n');
    assert!(!body.contains(&b'\n'));
    assert!(serde_json::from_slice::<Value>(&output.stdout)?.is_object());
    Ok(())
}
