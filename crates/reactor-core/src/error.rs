//! One error type: a clean, user-facing message. Printed without a backtrace,
//! and — in `--format json` — as the payload's `error` field.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReactorError(pub String);

impl ReactorError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ReactorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ReactorError {}

pub type Result<T> = std::result::Result<T, ReactorError>;

/// `err!("{path}: {why}")` → `Err(ReactorError)`.
#[macro_export]
macro_rules! err {
    ($($arg:tt)*) => {
        $crate::error::ReactorError::new(format!($($arg)*))
    };
}
