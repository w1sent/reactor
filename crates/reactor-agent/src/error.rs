//! One error type for the agent.

use std::fmt;
use std::path::Path;

#[derive(Debug)]
pub enum Error {
    /// A file could not be read or written.
    Io(String),
    /// The session store is inconsistent or was misused.
    Store(String),
    /// The model failed, or could not be reached.
    Model(String),
    /// A context reduction could not be carried out. The context did not shrink, so
    /// the loop must not continue on it (ADR-0037).
    Reduction(String),
    /// The loop stopped because reduction is not getting the context to fit.
    ReductionBudget { attempts: usize },
    /// The turn was cancelled.
    Cancelled,
}

impl Error {
    pub fn io(path: &Path, e: std::io::Error) -> Error {
        Error::Io(format!("{}: {}", path.display(), e))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(m) | Error::Store(m) | Error::Model(m) | Error::Reduction(m) => f.write_str(m),
            Error::ReductionBudget { attempts } => write!(
                f,
                "context reduction ran {attempts} times in a row and the context still does not fit -- stopping"
            ),
            Error::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for Error {}

impl From<reactor_core::ReactorError> for Error {
    fn from(e: reactor_core::ReactorError) -> Error {
        Error::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
