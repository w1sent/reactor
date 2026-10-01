//! The REactor catalogue as a library: which reverse-engineering tools exist
//! on this machine, which are active, how to install the missing ones, which
//! skills go with them.
//!
//! No rig, no gpui, no agent concepts (ADR-0034): files and subprocesses only.
//! That is what keeps it testable without a model and consumable by a harness
//! that is not ours.

pub mod catalogue;
pub mod commands;
pub mod completion;
pub mod config;
pub mod error;
pub mod host;
pub mod install;
pub mod json;
pub mod model;
pub mod paths;
pub mod probe;
pub mod recipes;
pub mod render;
pub mod report;
pub mod skills;
pub mod state;
pub mod util;

pub use error::{ReactorError, Result};
pub use paths::Paths;
pub use report::{Done, Report, SCHEMA};
