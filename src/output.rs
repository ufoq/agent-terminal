use std::io::{self, Write as _};

use serde::Serialize;
use thiserror::Error as ThisError;

use crate::{
    domain::{JobName, JobState},
    error::Error,
};

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok(CommandData),
    Error { code: &'static str, message: String },
}

impl Response {
    #[must_use]
    pub const fn ok(data: CommandData) -> Self {
        Self::Ok(data)
    }

    #[must_use]
    pub fn error(error: &Error) -> Self {
        Self::Error {
            code: error.kind(),
            message: error.public_message(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum CommandData {
    Start(StartData),
    Read(ReadData),
    Send(SendData),
    Press(PressData),
    Stop(StopData),
    List(ListData),
}

#[derive(Debug, Serialize)]
pub struct StartData {
    pub state: JobState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct ReadData {
    pub state: JobState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub screen: String,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct SendData;

#[derive(Debug, Serialize)]
pub struct PressData;

#[derive(Debug, Serialize)]
pub struct StopData;

#[derive(Debug, Serialize)]
pub struct JobSummary {
    pub job: JobName,
    pub state: JobState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct ListData {
    pub jobs: Vec<JobSummary>,
}

#[derive(Debug, ThisError)]
pub enum PrintError {
    #[error("could not serialize response: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("could not write response: {0}")]
    Write(#[from] io::Error),
}

pub fn print(response: &Response, pretty: bool) -> Result<(), PrintError> {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    if pretty {
        serde_json::to_writer_pretty(&mut writer, response)?;
    } else {
        serde_json::to_writer(&mut writer, response)?;
    }
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}
