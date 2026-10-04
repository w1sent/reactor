//! One error type for the database.

use std::fmt;
use std::path::Path;

#[derive(Debug)]
pub enum Error {
    /// A file could not be read or written.
    Io(String),
    /// SQLite refused, or returned something unexpected.
    Sqlite(String),
    /// This build's SQLite lacks something REactor needs.
    Build(String),
    /// The request breaks a rule of the model: it was refused and nothing was written.
    Invalid(String),
}

impl Error {
    pub fn io(path: &Path, e: std::io::Error) -> Error {
        Error::Io(format!("{}: {}", path.display(), e))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(m) | Error::Sqlite(m) | Error::Build(m) | Error::Invalid(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Error {
        Error::Sqlite(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
