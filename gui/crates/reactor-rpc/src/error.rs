//! Error type for the RPC client.
//!
//! One enum, `Display` by hand — the client's errors are few and the
//! messages are for the GUI's log view, not for matching.

use std::io;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// The child could not be spawned — `pi` missing from `PATH` is the
    /// common case, and the GUI surfaces it as its startup failure.
    Spawn(io::Error),
    /// A write to the child's stdin failed.
    Io(io::Error),
    /// A line was not valid JSON.
    Parse { line: String, reason: String },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Spawn(e) => write!(f, "failed to start pi: {e}"),
            Error::Io(e) => write!(f, "rpc transport: {e}"),
            Error::Parse { reason, .. } => write!(f, "malformed rpc line: {reason}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Spawn(e) | Error::Io(e) => Some(e),
            Error::Parse { .. } => None,
        }
    }
}
