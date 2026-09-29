//! The two halves every command produces: a payload that is the `--format json`
//! contract, and the human rendering of the same facts.

use serde::Serialize;

/// Bumped only for a breaking change to a payload's shape.
pub const SCHEMA: u32 = 1;

pub trait Report: Serialize {
    /// Human-readable output. Carries no stability guarantee; empty means
    /// "print nothing".
    fn human(&self) -> String;
}

/// A report plus the process exit code that goes with it. Some commands
/// succeed in emitting a payload and still exit non-zero (an install that was
/// not confirmed, a skill that failed to fetch, a config that differs).
pub struct Done<R> {
    pub report: R,
    pub code: i32,
}

impl<R> Done<R> {
    pub fn ok(report: R) -> Self {
        Self { report, code: 0 }
    }
    pub fn code(report: R, code: i32) -> Self {
        Self { report, code }
    }
}

/// `{"schema": N, ...payload}` — the envelope every `--format json` output wears.
#[derive(Serialize)]
pub struct Envelope<'a, T: Serialize> {
    pub schema: u32,
    #[serde(flatten)]
    pub payload: &'a T,
}

#[derive(Serialize)]
pub struct ErrorPayload<'a> {
    pub schema: u32,
    pub error: &'a str,
}

pub fn json_of<T: Serialize>(payload: &T) -> String {
    crate::json::to_string_pretty(&Envelope { schema: SCHEMA, payload })
}

pub fn json_error(message: &str) -> String {
    crate::json::to_string_pretty(&ErrorPayload { schema: SCHEMA, error: message })
}
