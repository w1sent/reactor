//! REactor's analysis database
//! ([ADR-0046](../../docs/adr/0046-the-analysis-database-is-sqlite-inside-the-case.md)):
//! SQLite with FTS5 and the sqlite-vec extension, all compiled into the binary, and the
//! case directory it lives in
//! ([ADR-0045](../../docs/adr/0045-a-case-is-the-working-directory.md)).
//!
//! Layering: this crate knows SQLite and files. No rig, no tokio, no gpui — the agent
//! and the GUI sit above it.

pub mod artifact;
pub mod blob;
pub mod browse;
pub mod case;
pub mod corpus;
pub mod db;
pub mod error;
pub mod export;
pub mod graph;
pub mod interchange;
pub mod manifest;
pub mod migrate;
pub mod reference;
pub mod selfcheck;
pub mod sessions;
pub mod sqlite;
pub mod time;

pub use case::Case;
pub use db::Db;
pub use error::{Error, Result};
