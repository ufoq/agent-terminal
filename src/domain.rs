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

    #[must_use]
    pub fn pane_title(&self) -> String {
        format!("agent-terminal:job:{}", self.0)
    }

    #[must_use]
    pub fn from_pane_title(title: &str) -> Option<Self> {
        title
            .strip_prefix("agent-terminal:job:")
            .and_then(|value| Self::from_str(value).ok())
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedScreen {
    pub screen: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key(String);

impl Key {
    #[must_use]
    pub fn zellij_token(&self) -> &str {
        &self.0
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
            return Ok(Self(value.to_owned()));
        }
        if let Some(number) = value
            .strip_prefix('F')
            .and_then(|rest| rest.parse::<u8>().ok())
            && (1..=12).contains(&number)
            && value == format!("F{number}")
        {
            return Ok(Self(value.to_owned()));
        }
        if let Some(character) = single_character(value, "Ctrl+")
            && character.is_ascii_alphabetic()
        {
            return Ok(Self(format!("Ctrl {}", character.to_ascii_lowercase())));
        }
        if let Some(character) = single_character(value, "Alt+")
            && character.is_ascii()
            && !character.is_ascii_control()
        {
            let name = if character == ' ' {
                "Space".to_owned()
            } else {
                character.to_string()
            };
            return Ok(Self(format!("Alt {name}")));
        }
        Err(Error::InvalidInput {
            message: format!("unsupported key {value:?}"),
        })
    }
}

fn single_character(value: &str, prefix: &str) -> Option<char> {
    let rest = value.strip_prefix(prefix)?;
    let mut characters = rest.chars();
    let character = characters.next()?;
    if characters.next().is_some() {
        return None;
    }
    Some(character)
}

#[must_use]
pub fn normalize_screen(input: &str) -> String {
    strip_ansi_escapes::strip_str(input)
}

#[must_use]
pub fn bound_screen(input: &str, max_lines: usize, max_bytes: usize) -> BoundedScreen {
    let mut lines: Vec<&str> = input.lines().collect();
    let mut truncated = lines.len() > max_lines;
    if truncated {
        lines = lines
            .into_iter()
            .rev()
            .take(max_lines)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
    }
    let mut screen = lines.join("\n");
    if screen.len() > max_bytes {
        let mut start = screen.len() - max_bytes;
        while !screen.is_char_boundary(start) {
            start += 1;
        }
        screen = screen[start..].to_owned();
        truncated = true;
    }
    BoundedScreen { screen, truncated }
}

#[cfg(test)]
mod tests {
    use super::{Key, bound_screen};

    #[test]
    fn truncation_keeps_utf8_boundaries() {
        let input = "界".repeat(10923);
        let bounded = bound_screen(&input, 1000, 32 * 1024);
        assert!(bounded.truncated);
        assert!(bounded.screen.len() <= 32 * 1024);
        assert!(bounded.screen.is_char_boundary(0));
    }

    #[test]
    fn lines_are_bounded() {
        let input = (0..300)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let bounded = bound_screen(&input, 200, 32 * 1024);
        assert!(bounded.truncated);
        assert_eq!(bounded.screen.lines().count(), 200);
    }

    #[test]
    fn alt_space_uses_named_token() {
        assert_eq!(
            "Alt+ ".parse::<Key>().ok().as_ref().map(Key::zellij_token),
            Some("Alt Space")
        );
    }
}
