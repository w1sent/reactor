//! REactor's session half: the state machines behind the manifest, the working
//! identity, reporting and scenarios, plus the deterministic text each one puts
//! in front of the model.
//!
//! Everything here is a pure function of its inputs (state, settings, a folder
//! on disk) — no model, no agent loop, no UI, no rig. That is what makes it
//! testable without any of them, and it is the rule that keeps prompt text out
//! of the agent ([ADR-0035](../../docs/adr/0035-portable-surface-is-machine-facts.md)):
//! the agent composes these blocks, it does not author them.
//!
//! Each module is a port of one pi extension's logic with the host removed:
//! the extension's command handlers become `command(&mut State, …) -> Effects`,
//! its `before_agent_start` block becomes `block(&State, …)`. What a handler did
//! *to the world* — append a session entry, enable a toolset, write a config
//! file — comes back as data in the effects for the caller to perform, which is
//! what keeps this crate ignorant of the session store that does not exist yet
//! ([ADR-0036](../../docs/adr/0036-reactor-owns-its-session-store-format.md)).
//!
//! The pi extensions are frozen and stay as they are (ADR-0035); they are the
//! *specification* here. `crates/reactor-context/tests/golden/` holds what they
//! actually said and did, captured by `tests/extensions/golden/capture.mjs`, and
//! these modules must reproduce it byte for byte.

pub mod identity;
pub mod manifest;
pub mod notice;
pub mod reporting;
pub mod scenario;
pub mod settings;
pub mod text;

pub use notice::{Level, Notice};
