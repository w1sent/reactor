//! One error type for the agent.

use std::fmt;
use std::path::Path;

#[derive(Debug)]
pub enum Error {
    /// A file could not be read or written.
    Io(String),
    /// The session store is inconsistent or was misused.
    Store(String),
    /// The model failed, or could not be reached, in a way with no recovery of its own.
    Model(String),
    /// The provider refused the request as too long for the model's context window
    /// ([ADR-0057](../../docs/adr/0057-a-turn-recovers-from-a-rejected-or-cut-off-request.md)).
    Overflow(String),
    /// The provider's policy blocked the request or its reply. The same request would be
    /// blocked again, so it is never retried unchanged.
    Blocked(String),
    /// A failure worth trying again after a pause: a rate limit, an overloaded or
    /// unreachable server, a stream that went quiet.
    Transient(String),
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

    /// Whether the error came from the model or the connection to it (as opposed to the
    /// log, the files or a reduction).
    pub fn is_model(&self) -> bool {
        matches!(
            self,
            Error::Model(_) | Error::Overflow(_) | Error::Blocked(_) | Error::Transient(_)
        )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(m)
            | Error::Store(m)
            | Error::Model(m)
            | Error::Overflow(m)
            | Error::Blocked(m)
            | Error::Transient(m)
            | Error::Reduction(m) => f.write_str(m),
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

impl From<reactor_db::Error> for Error {
    fn from(e: reactor_db::Error) -> Error {
        match e {
            reactor_db::Error::Io(m) => Error::Io(m),
            other => Error::Store(other.to_string()),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
