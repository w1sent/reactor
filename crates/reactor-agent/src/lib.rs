//! REactor's agent: the session store, the loop over rig, the context budget
//! manager and the tools ([ADR-0033](../../docs/adr/0033-reactor-is-a-rust-project-on-rig.md)).
//!
//! Layering: `reactor-core` (machine facts) ← `reactor-context` (session state and
//! the blocks it renders) ← this crate, which owns the message list. Owning it is the
//! point: reduction, continuation and the session tree stop being fights with a host.

pub mod agent;
pub mod budget;
pub mod context;
pub mod entry;
pub mod error;
pub mod history;
pub mod llm;
pub mod prompt;
pub mod provider;
pub mod skills;
pub mod store;
pub mod tools;
pub mod truncate;

pub use error::{Error, Result};
