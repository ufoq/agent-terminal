use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::error::Error;

const MAX_JOB_NAME_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobName(String);

impl JobName {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for JobName {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let valid_len = !value.is_empty() && value.len() <= MAX_JOB_NAME_LEN;
        let mut chars = value.chars();
        let valid_start = chars
            .next()
            .is_some_and(|character| character.is_ascii_lowercase() || character.is_ascii_digit());
        let valid_rest = chars.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_' | '.')
        });

        if valid_len && valid_start && valid_rest {
            Ok(Self(value.to_owned()))
        } else {
            Err(Error::InvalidInput {
                message: format!(
                    "invalid job name {value:?}; use 1-64 lowercase ASCII letters, digits, '.', '_', or '-', starting with a letter or digit"
                ),
            })
        }
    }
}

impl fmt::Display for JobName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionName(String);

impl SessionName {
    pub fn new(value: String) -> Result<Self, Error> {
        if value.is_empty()
            || !value.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            return Err(Error::InvalidInput {
                message: format!("invalid session name {value:?}"),
            });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TerminalPaneId(u32);

impl TerminalPaneId {
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for TerminalPaneId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "terminal_{}", self.0)
    }
}

impl FromStr for TerminalPaneId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let numeric = value
            .strip_prefix("terminal_")
            .ok_or_else(|| Error::InvalidInput {
                message: format!("invalid terminal pane id {value:?}"),
            })?;
        if numeric.is_empty() || !numeric.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Error::InvalidInput {
                message: format!("invalid terminal pane id {value:?}"),
            });
        }
        numeric
            .parse::<u32>()
            .map(Self)
            .map_err(|source| Error::InvalidInput {
                message: format!("invalid terminal pane id {value:?}: {source}"),
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Exited,
    Lost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    public_name: String,
    zellij_token: String,
}

impl Key {
    #[must_use]
    pub fn zellij_token(&self) -> &str {
        &self.zellij_token
    }

    #[must_use]
    pub fn public_name(&self) -> &str {
        &self.public_name
    }
}

impl FromStr for Key {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let named = [
            "Enter",
            "Tab",
            "Esc",
            "Backspace",
            "Delete",
            "Insert",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "Up",
            "Down",
            "Left",
            "Right",
        ];
        if named.contains(&value) {
            return Ok(Self {
                public_name: value.to_owned(),
                zellij_token: value.to_owned(),
            });
        }
        if let Some(number) = value
            .strip_prefix('F')
            .and_then(|rest| rest.parse::<u8>().ok())
            && (1..=12).contains(&number)
            && value == format!("F{number}")
        {
            return Ok(Self {
                public_name: value.to_owned(),
                zellij_token: value.to_owned(),
            });
        }
        if let Some(character) = single_character(value, "Ctrl+")
            && character.is_ascii_alphabetic()
        {
            return Ok(Self {
                public_name: format!("Ctrl+{}", character.to_ascii_uppercase()),
                zellij_token: format!("Ctrl {}", character.to_ascii_lowercase()),
            });
        }
        if let Some(character) = single_character(value, "Alt+")
            && character.is_ascii()
            && !character.is_ascii_control()
        {
            return Ok(Self {
                public_name: format!("Alt+{character}"),
                zellij_token: format!("Alt {character}"),
            });
        }
        Err(Error::InvalidInput {
            message: format!("unsupported key {value:?}"),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedScreen {
    pub screen: String,
    pub truncated: bool,
}

#[must_use]
pub fn bound_screen(source: &str, max_lines: usize, max_bytes: usize) -> BoundedScreen {
    let logical_lines = source.bytes().filter(|byte| *byte == b'\n').count()
        + usize::from(!source.is_empty() && !source.ends_with('\n'));
    let lines_to_skip = logical_lines.saturating_sub(max_lines);
    let line_start = if max_lines == 0 {
        source.len()
    } else if lines_to_skip == 0 {
        0
    } else {
        source
            .match_indices('\n')
            .nth(lines_to_skip - 1)
            .map_or(0, |(index, _)| index + 1)
    };
    let line_tail = &source[line_start..];
    let byte_floor = line_tail.len().saturating_sub(max_bytes);
    let mut byte_start = byte_floor;
    while byte_start < line_tail.len() && !line_tail.is_char_boundary(byte_start) {
        byte_start += 1;
    }
    BoundedScreen {
        screen: line_tail[byte_start..].to_owned(),
        truncated: line_start > 0 || byte_start > 0,
    }
}

fn single_character(value: &str, prefix: &str) -> Option<char> {
    let mut characters = value.strip_prefix(prefix)?.chars();
    let character = characters.next()?;
    characters.next().is_none().then_some(character)
}
