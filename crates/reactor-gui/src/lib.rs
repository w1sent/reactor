//! reactor-gui — the native REactor frontend (SPEC.md).
//!
//! Library target so the pure pieces (theme, session) test
//! without a window; `main.rs` is the thin binary shell.

pub mod app;
pub mod backend;
pub mod chrome;
pub mod console;
pub mod docs_panel;
pub mod files;
pub mod graph;
pub mod headless;
pub mod hints;
pub mod import;
pub mod inspect_ui;
pub mod layout;
pub mod notices_ui;
pub mod notifications;
pub mod palette;
pub mod panels;
pub mod prompt_history;
pub mod quick_filter;
pub mod session;
pub mod session_list;
pub mod settings;
pub mod settings_window;
pub mod start;
pub mod theme;
