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
    Ok { data: CommandData },
    Error { error: ErrorBody },
}

impl Response {
    #[must_use]
    pub const fn ok(data: CommandData) -> Self {
        Self::Ok { data }
    }

    #[must_use]
    pub fn error(error: &Error) -> Self {
        Self::Error {
            error: ErrorBody {
                code: error.kind(),
                message: error.to_string(),
                hint: error.hint(),
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<&'static str>,
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
    pub job: JobName,
    pub state: JobState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct ReadData {
    pub job: JobName,
    pub state: JobState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub screen_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct SendData {
    pub job: JobName,
    pub issued: Issued,
    pub submitted: bool,
}

#[derive(Debug, Serialize)]
pub struct PressData {
    pub job: JobName,
    pub issued: Issued,
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Issued {
    Text,
    Keys,
}

#[derive(Debug, Serialize)]
pub struct StopData {
    pub job: JobName,
    pub cleaned_up: bool,
    pub forced: bool,
    pub screen_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_screen: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

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
