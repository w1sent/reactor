//! reactor-gui — the native REactor frontend (gui/SPEC.md).
//!
//! Library target so the pure pieces (theme, contract, session) test
//! without a window; `main.rs` is the thin binary shell.

pub mod app;
pub mod chrome;
pub mod console;
pub mod contract;
pub mod layout;
pub mod panels;
pub mod session;
pub mod start;
pub mod theme;
pub mod views;
