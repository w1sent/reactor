//! What a command tells the person at the keyboard: a message and how loudly.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub message: String,
    pub level: Level,
}

impl Notice {
    pub fn info(message: impl Into<String>) -> Self {
        Self { message: message.into(), level: Level::Info }
    }
    pub fn warning(message: impl Into<String>) -> Self {
        Self { message: message.into(), level: Level::Warning }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self { message: message.into(), level: Level::Error }
    }
}
