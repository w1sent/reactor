//! reactor-gui — the native REactor frontend (SPEC.md).
//!
//! Library target so the pure pieces (theme, session) test
//! without a window; `main.rs` is the thin binary shell.

pub mod app;
pub mod backend;
pub mod chrome;
pub mod docs_panel;
pub mod files;
pub mod graph;
pub mod headless;
pub mod hints;
pub mod identity_editor;
pub mod import;
pub mod indicator;
pub mod inspect_ui;
pub mod layout;
pub mod manifest_ui;
pub mod mcp_editor;
pub mod model_cards;
pub mod notices_ui;
pub mod notifications;
pub mod palette;
pub mod pane;
pub mod panels;
pub mod plan_ui;
pub mod prompt_history;
pub mod quick_filter;
pub mod rail;
pub mod ref_index;
pub mod scenario_editor;
pub mod scenario_start;
pub mod scenario_ui;
pub mod session;
pub mod session_list;
pub mod settings;
pub mod settings_window;
pub mod shortcuts;
pub mod start;
pub mod terminal;
pub mod theme;
pub mod toolbox;
pub mod transcript_ui;
