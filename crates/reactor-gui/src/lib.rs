//! reactor-gui — the native REactor frontend (SPEC.md).
//!
//! Library target so the pure pieces (theme, session) test
//! without a window; `main.rs` is the thin binary shell.

pub mod app;
pub mod backend;
pub mod chrome;
pub mod console;
pub mod layout;
pub mod panels;
pub mod session;
pub mod start;
pub mod theme;
