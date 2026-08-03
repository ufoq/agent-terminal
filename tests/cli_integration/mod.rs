mod args;
mod cwd;
mod errors;
mod output;
mod paths;
mod state;

use std::{error::Error, fs, io};

use agent_terminal::paths::ProjectPaths;

use crate::support::cli_harness::DeterministicHarness;

type TestResult = Result<(), Box<dyn Error>>;

fn project_paths(
    harness: &DeterministicHarness,
) -> Result<ProjectPaths, agent_terminal::error::Error> {
    ProjectPaths::new(&harness.project, Some(&harness.state_dir), "standalone")
}

fn registry_value(harness: &DeterministicHarness) -> Result<serde_json::Value, Box<dyn Error>> {
    let paths = project_paths(harness)?;
    Ok(serde_json::from_slice(&fs::read(paths.state_file())?)?)
}

fn persisted_command(harness: &DeterministicHarness) -> Result<Vec<String>, Box<dyn Error>> {
    let registry = registry_value(harness)?;
    let command = registry
        .pointer("/jobs/job/command")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| io::Error::other("persisted command was not an array"))?;
    command
        .iter()
        .map(|argument| {
            argument
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| io::Error::other("persisted argument was not a string"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}
