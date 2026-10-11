//! The application window: one session, the agent running in this process, the docked
//! layout of SPEC.md §6.
//!
//! The GUI holds no facts. The transcript is the session store rendered
//! ([`crate::session::Session`]); the side panels render what `reactor-core` answers
//! ([`reactor_client::ReactorClient`]) and what the agent reports ([`crate::backend`]).
//! Panels are views over this one state object — stateless presentation, per the ADR-0029
//! discipline. There is no child process and no protocol: the agent is a library call
//! (ADR-0033, docs/adr/0042).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use gpui_kit::base::dock::PanelView;
use gpui_kit::base::h_flex;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::command::{Command as PaletteList, CommandItem, CommandState};
use gpui_kit::component::dock::{DockArea, DockPlacement, DockSkin, panel_handle};
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Root, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{App, Entity, KeyBinding, Render, SharedString, Window, div, px};
use serde::Deserialize;

use reactor_agent::entry::{EntryId, Mode};
use reactor_client::{Client, ReactorClient};
use reactor_context::settings::{ContextSettings, Settings};

use crate::backend::{Backend, ContextView, PlanView, StartOptions, TreeRow, UiEvent};
use crate::palette::{self, Args, Entry, Usage};
use crate::panels::{
    ConsolePanel, ContextPanel, ServicesPanel, ToolsPanel, ToolsetsPanel, TranscriptPanel,
    TreePanel,
};
use crate::session::{AgentPhase, ChatItem, Session, status_items};

/// How often the event pump drains the backend's channel (SPEC.md §2).
const PUMP_TICK: Duration = Duration::from_millis(50);
/// The indicator clock's step: a frame at about 60 a second for a pulse (a shimmer asks for its
/// own frames).
const INDICATOR_TICK: Duration = Duration::from_millis(16);
/// In the light rendering mode the indicators redraw every this many clock ticks (about 12 a
/// second): enough to look alive on a software rasteriser without keeping it busy.
const LIGHT_INDICATOR_TICKS: u32 = 5;
/// Popups are shown and taken down every this many clock ticks (about 100 ms).
const TOAST_TICKS: u32 = 6;
/// How much of a tool card's output the transcript renders before it sheds the tail
/// (the whole of it is in the session and `history_read` reaches it).
pub const TOOL_OUTPUT_MAX_CHARS: usize = 2000;

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

actions!(
    reactor_gui,
    [
        /// Esc: interrupt the running turn, restoring anything queued into the composer.
        ComposerEsc,
        /// Refresh the catalogue and services panels.
        RefreshReactor,
        /// Ctrl+P / Cmd+P, or the Commands menu: open the command palette.
        OpenPalette,
        /// Ctrl+W / Cmd+W: close the tab in front (the one with the focus).
        CloseTab,
        /// While the `/` completion popup is open: move the highlight.
        SlashUp,
        SlashDown,
        /// Take the highlighted completion (Enter, Tab).
        SlashAccept,
        /// Close the popup without choosing (Esc).
        SlashDismiss,
        /// Esc in the command palette: close it, whatever is typed.
        ClosePalette,
        /// Open the settings window (Cmd+, / Ctrl+,, or the Commands menu).
        OpenSettings,
        /// The Case menu: make this folder a case (ADR-0045).
        SaveCase,
        /// The Case menu: import context documents (ADR-0046).
        ImportDocuments,
        /// Esc in the notification history.
        CloseNotices,
        /// Up in the prompt, on its top line: the previous message you sent.
        HistoryPrev,
        /// Down in the prompt, on its bottom line: the next message.
        HistoryNext,
    ]
);

/// Model picked from the status bar's dropdown (`provider/name`).
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct SelectModelAction {
    pub spec: String,
}

/// A layout preset picked from the Layout menu.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct ApplyLayoutAction {
    pub preset: crate::layout::LayoutPreset,
}

/// Show or hide one dock, from the Layout menu.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct ToggleDockAction {
    pub side: crate::layout::DockSide,
}

/// Close or open one panel, from the Layout menu's Panels submenu.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct TogglePanelAction {
    pub panel: crate::layout::PanelKind,
}

// ---------------------------------------------------------------------------
// ReactorApp — the root view
// ---------------------------------------------------------------------------

/// What belongs to one open session and nothing else: kept here for the sessions that are
/// not the active one, while the active one's lives in `ReactorApp`'s own fields (which is
/// what every panel reads: the tree, the context, the status bar follow the active session).
#[derive(Clone)]
struct Bundle {
    backend: Option<Arc<Backend>>,
    session: Entity<Session>,
    composer: Entity<TextareaState>,
    tree: Vec<TreeRow>,
    leaf_id: Option<EntryId>,
    context: Option<ContextView>,
    preview: Option<PlanView>,
    context_busy: bool,
}

/// A session open in this window, with the transcript tab that shows it. Sessions are not
/// tied to the window: any number are open at once, each in its own tab.
struct OpenSession {
    id: String,
    /// The session's own state as of when it was last the active one.
    bundle: Bundle,
    transcript: Entity<crate::panels::TranscriptPanel>,
    /// How the dock holds the tab; the tab may be closed while the session stays open.
    handle: Arc<dyn PanelView>,
    /// This session's tab is the one the layouts place (the Transcript panel of the presets
    /// and the Panels menu). When it is closed another session takes its place.
    primary: bool,
}

/// The one window's state.
pub struct ReactorApp {
    pub cwd: PathBuf,
    /// The working directory as a filesystem — what the explorer shows and `/open` resolves
    /// relative paths against.
    pub workdir_fs: Arc<reactor_fs::LocalFs>,
    /// The whole host disk, for a file the transcript names outside the working directory.
    pub host_fs: Arc<reactor_fs::LocalFs>,
    /// `None` only when the agent could not start; the window opens anyway and says why.
    backend: Option<Arc<Backend>>,
    /// Vectors are being made for the case's documents: how far along.
    embedding: Option<reactor_db::corpus::Progress>,
    /// The case's graph, drawn. Told to read again whenever the agent may have written.
    graph_panel: Entity<crate::graph::panel::GraphPanel>,
    /// The documents, searched by hand.
    docs_panel: Entity<crate::docs_panel::DocsPanel>,
    /// The Session Tree, told to draw again when a session starts or stops working.
    tree_panel: Entity<TreePanel>,
    reactor: Client,
    pub session: Entity<Session>,
    pub composer: Entity<TextareaState>,
    /// Notifications: the popups, and the history behind the bell.
    pub notifier: crate::notifications::Notifier,
    /// Notifications not yet shown as popups, and the popups showing with when they go.
    pending_toasts: Vec<(u64, crate::notifications::Level, String)>,
    live_toasts: Vec<(u64, std::time::Instant)>,
    /// The history popup, while it is open.
    pub notices: Option<crate::notices_ui::NoticesUi>,
    /// The models the picker offers (`provider/name`).
    pub models: Vec<String>,
    /// Side-panel data, from `reactor-core`.
    pub catalogue: Option<reactor_client::ToolsPayload>,
    pub toolsets: Option<reactor_client::ToolsetsPayload>,
    pub services: Option<reactor_client::ServicesPayload>,
    /// Where the current activation comes from: `session`, `project`, `machine` or `default`.
    pub activation_scope: Option<String>,
    pub catalogue_loading: bool,
    pub services_loading: bool,
    /// The session tree, and where its tip is.
    pub tree: Vec<TreeRow>,
    pub leaf_id: Option<EntryId>,
    /// The case's sessions, for continuing an old one (read at start, after every turn and
    /// when the panel asks).
    pub sessions: Vec<crate::session_list::SessionRow>,
    /// Every session open in this window, and which one is the active one: the session
    /// whose transcript was last in focus (or opened). What a panel shows that belongs to
    /// a session — tree, context, status, the switches — is the active one's.
    open: Vec<OpenSession>,
    active: String,
    /// The transcript the layouts' *Transcript* panel is, which `Panels::remove` takes out of
    /// the dock: the primary session's, and changed when that session is closed.
    transcript_slot: Arc<std::sync::Mutex<Entity<TranscriptPanel>>>,

    /// The context panel's feed, and the preview last asked for.
    pub context: Option<ContextView>,
    pub preview: Option<PlanView>,
    /// A manual reduction or preview is in flight.
    pub context_busy: bool,
    scenarios_dir: PathBuf,
    panels: crate::layout::Panels,
    /// The preset last applied — what the Layout menu shows a tick beside.
    pub layout: crate::layout::LayoutPreset,
    dock_area: Entity<DockArea>,
    /// The in-window menu bar; `None` where the platform has a native one.
    menu_bar: Option<Entity<gpui_kit::component::menu::AppMenuBar>>,
    /// The command palette, while it is open.
    palette: Option<PaletteUi>,
    /// How often each command was run — what breaks ties in the ranking.
    usage: Usage,
    /// The `/` completion popup over the composer.
    pub slash: SlashUi,
    /// The working directory's files, for `#` in the composer.
    file_index: crate::ref_index::FileIndex,
    /// What the person has sent, for up/down in the prompt.
    prompt_history: crate::prompt_history::PromptHistory,
    /// Fonts and behaviour (`crate::settings`).
    pub ui: crate::settings::UiSettings,
    /// The context-window popup, while it is open.
    pub inspect: Option<crate::inspect_ui::InspectUi>,
    /// Font family fields as typed (half a name is not applied, but is kept on screen).
    family_drafts: std::collections::HashMap<crate::settings::Slot, String>,
    /// The model page's provider and name fields.
    pub model_draft: crate::settings_window::ModelDraft,
    /// The provider and name typed on the Documents page for the embedding model.
    pub embedding_draft: crate::settings_window::ModelDraft,
    /// "Re-embed everything" was pressed once and waits for its confirmation.
    pub reembed_armed: bool,
    /// An identity's Remove was pressed once and waits for its confirmation.
    pub identity_remove_armed: Option<String>,
    /// An MCP server's Remove was pressed once and waits for its confirmation.
    pub mcp_remove_armed: Option<String>,
    /// The Context panel's Scenario section: what can be started, or what is running.
    pub scenario: crate::scenario_ui::View,
    /// The scenario chosen to start (the first one until another is picked).
    pub scenario_pick: Option<String>,
    /// The Plan tab's goal, guidelines, steps and reporting switches.
    pub plan: crate::plan_ui::PlanView,
    /// The goal editor and the Frame popup, while they are open.
    pub goal_editor: Option<crate::manifest_ui::GoalEditor>,
    pub frame_popup: Option<crate::manifest_ui::FramePopup>,
    /// Which layer of the context budget the settings window edits.
    pub context_scope: crate::settings_window::ContextScope,
    /// The settings window, while it is open.
    pub settings_window: Option<gpui_kit::AnyWindowHandle>,
    /// The window may close without asking: the question about an unsaved state has been
    /// answered, or there was nothing to ask.
    closing: bool,
    /// Sessions that were working have been told to stop so that the window can close:
    /// the question about them is answered.
    interrupted: bool,
}

/// One identity, as the settings page lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentityRow {
    pub name: String,
    /// Built in: read, and copied; not changed or removed.
    pub builtin: bool,
    pub text: String,
    /// New sessions start with it.
    pub default: bool,
    /// This session uses it now.
    pub active: bool,
}

/// One MCP server, as the settings page lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct McpServerRow {
    pub server: reactor_core::mcp::Server,
    /// `None` when there is no agent, or the host has not heard of it yet.
    pub standing: Option<reactor_agent::mcp::Standing>,
    pub tools: usize,
    /// What all its tools would cost every request if every one were active (an estimate).
    pub tokens: u64,
}

/// The open command palette.
struct PaletteUi {
    list: Entity<CommandState>,
    /// What the commands' parameters are chosen from, as of when it opened.
    snapshot: palette::Snapshot,
    /// Every entry, built when the palette opened.
    entries: Vec<Entry>,
    /// What is shown now: `entries` ranked for the query.
    shown: Vec<Entry>,
}

/// The `/` completion popup: the ranked commands for what has been typed after the slash.
#[derive(Default)]
pub struct SlashUi {
    pub items: Vec<Entry>,
    pub selected: usize,
    /// The items complete a `#` reference: what kind it is, and the span of the composer's text
    /// that a completion replaces. Absent: they complete a `/` command.
    pub reference: Option<(crate::ref_index::Trigger, std::ops::Range<usize>)>,
    /// Esc closed it; it stays closed until the text changes.
    dismissed: bool,
}

impl SlashUi {
    pub fn is_open(&self) -> bool {
        !self.dismissed && !self.items.is_empty()
    }
}

/// The keys this module's actions answer to. Call once, after `gpui_kit::init`: the popup's
/// bindings must come *after* the input's own, which they override while the popup is open.
pub fn bind_keys(cx: &mut App) {
    crate::files::editor::bind_keys(cx);
    crate::files::hex::bind_keys(cx);
    crate::graph::panel::bind_keys(cx);
    cx.bind_keys(vec![
        // `secondary` is Cmd on macOS and Ctrl elsewhere.
        KeyBinding::new("secondary-p", OpenPalette, None),
        KeyBinding::new("secondary-w", CloseTab, None),
        KeyBinding::new("secondary-,", OpenSettings, None),
        KeyBinding::new("escape", CloseNotices, Some("ReactorNotices")),
        // The prompt: up on its top line and down on its bottom line walk through the messages
        // sent before; anywhere else the handlers decline and the input moves its cursor.
        // Defined before the popup's bindings, which win while the popup is open.
        KeyBinding::new("up", HistoryPrev, Some("PromptComposer > Input")),
        KeyBinding::new("down", HistoryNext, Some("PromptComposer > Input")),
        // `SlashPopup > Input`: the text input, while the popup is open above it. Deeper than
        // the popup alone, so these win over the input's own up/down/enter/tab/escape.
        KeyBinding::new("up", SlashUp, Some("SlashPopup > Input")),
        KeyBinding::new("down", SlashDown, Some("SlashPopup > Input")),
        KeyBinding::new("enter", SlashAccept, Some("SlashPopup > Input")),
        KeyBinding::new("tab", SlashAccept, Some("SlashPopup > Input")),
        KeyBinding::new("escape", SlashDismiss, Some("SlashPopup > Input")),
        // Esc always closes the palette — the list's own Esc only clears a typed query first.
        KeyBinding::new("escape", ClosePalette, Some("ReactorPalette > Input")),
    ]);
}

/// How many completions the popup lists.
pub const SLASH_ROWS: usize = 8;

/// Marks the popups this app pushes, so they can be taken down by id.
struct NoticeKind;

/// The window `ReactorApp` opened, and the app inside it.
///
/// Exists for the Layout menu's actions to reach a window from macOS's native menu bar,
/// which is application-wide: `App::on_action`'s global handlers get only `&mut App`,
/// with no `Window` to hand `DockArea::set_dock` and friends. The app has exactly one
/// window at a time (SPEC.md §3), so a plain global is enough.
#[derive(Clone)]
pub struct MainWindow {
    pub handle: gpui_kit::AnyWindowHandle,
    pub app: gpui_kit::WeakEntity<ReactorApp>,
}

impl gpui_kit::Global for MainWindow {}

/// Something the launcher wants said once the window is up (a save that failed).
static STARTUP_NOTE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn set_startup_note(note: Option<String>) {
    *STARTUP_NOTE.lock().unwrap() = note;
}

/// Start this program again, detached: it outlives the window that asked.
fn spawn_self(args: &[std::ffi::OsString]) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    Command::new(std::env::current_exe()?)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

/// What the person chose when closing an unsaved state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseChoice {
    Save,
    Delete,
    Cancel,
}

impl ReactorApp {
    // -- the case (ADR-0045) -----------------------------------------------------------------

    /// *Save as Case*: make this folder a case. The case can only be moved while nothing
    /// has it open, so the window closes and a new one opens on the same session, saving
    /// on its way in.
    pub fn save_case(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.backend.clone() else {
            self.push_note(
                "warning",
                "The agent did not start, so there is nothing to save.",
            );
            return;
        };
        if !b.is_unsaved() {
            self.push_note(
                "info",
                format!("{} is already a case.", b.case.root().display()),
            );
            return;
        }
        // The window is replaced, and every session open in it with it.
        if self
            .open
            .iter()
            .filter_map(|o| o.bundle.backend.as_ref())
            .chain(self.backend.as_ref())
            .any(|b| b.is_running())
        {
            self.push_note(
                "warning",
                "An agent is working. Interrupt it or let it finish, then save.",
            );
            return;
        }
        let mut args: Vec<std::ffi::OsString> = vec![
            b.case.root().as_os_str().to_owned(),
            "--session".into(),
            b.session_id.clone().into(),
            "--save-case".into(),
        ];
        let model = b.model();
        if !model.is_empty() {
            args.extend(["--model".into(), model.into()]);
        }
        match spawn_self(&args) {
            Ok(()) => {
                self.closing = true;
                cx.quit();
            }
            Err(e) => self.push_note("error", format!("could not reopen REactor to save: {e}")),
        }
    }

    // -- context documents (ADR-0046) ------------------------------------------------------------

    /// `/import [sources…]`: what was named, or — with nothing named — what the person picks
    /// in the system's file dialog (files and folders both).
    pub fn import_documents(&mut self, args: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else {
            self.push_note(
                "warning",
                "The agent did not start, so there is no case to import into.",
            );
            return;
        };
        let named = split_words(args);
        if !named.is_empty() {
            backend.import(named);
            return;
        }
        let receiver = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Import".into()),
        });
        window
            .spawn(cx, async move |_| {
                if let Ok(Ok(Some(paths))) = receiver.await {
                    backend.import(
                        paths
                            .into_iter()
                            .map(|p| p.to_string_lossy().into_owned())
                            .collect(),
                    );
                }
            })
            .detach();
    }

    /// The embedding model in `settings.json`, if one is chosen.
    pub fn embedding_model(&self) -> Option<String> {
        self.backend
            .as_ref()
            .and_then(|b| Settings::load(&b.paths).embedding.model)
    }

    /// The corpus and its vectors, in words (what `/embedding` says).
    pub fn corpus_status(&self) -> String {
        self.backend
            .as_ref()
            .map(|b| b.corpus_status())
            .unwrap_or_else(|| "The agent did not start, so there is no case.".into())
    }

    /// Choose the model typed on the Documents page.
    pub fn set_typed_embedding(&mut self, cx: &mut Context<Self>) {
        let name = self.embedding_draft.name.trim().to_string();
        if name.is_empty() {
            self.push_note("warning", "type an embedding model name first");
            return;
        }
        let spec = format!("{}/{}", self.embedding_draft.provider, name);
        self.embedding_command(&spec, cx);
    }

    /// The threshold for a match by meaning (0: none).
    pub fn min_similarity(&self) -> f32 {
        self.backend.as_ref().map_or(0.0, |b| b.min_similarity())
    }

    pub fn set_min_similarity(&mut self, value: f32, cx: &mut Context<Self>) {
        if let Some(b) = &self.backend
            && let Err(e) = b.set_min_similarity(value)
        {
            self.push_note("error", format!("embedding: {e}"));
        }
        cx.notify();
    }

    /// How far the vectors being made have got, while a run is going.
    pub fn embedding_progress(&self) -> Option<reactor_db::corpus::Progress> {
        self.embedding
    }

    /// First press of *Re-embed everything*: ask for a second, because it sends all the
    /// passages to the provider again and takes the meaning search away until it is done.
    pub fn reembed_arm(&mut self, cx: &mut Context<Self>) {
        self.reembed_armed = true;
        cx.notify();
    }

    pub fn reembed_cancel(&mut self, cx: &mut Context<Self>) {
        self.reembed_armed = false;
        cx.notify();
    }

    /// The second press.
    pub fn reembed_confirm(&mut self, cx: &mut Context<Self>) {
        self.reembed_armed = false;
        if let Some(backend) = &self.backend {
            backend.reembed_all();
        }
        cx.notify();
    }

    pub fn embedding_off(&mut self, cx: &mut Context<Self>) {
        self.embedding_command("off", cx);
    }

    pub fn embedding_now(&mut self, cx: &mut Context<Self>) {
        self.embedding_command("now", cx);
    }

    /// `/embedding`: show the state, choose the model, turn it off, or run it now.
    fn embedding_command(&mut self, args: &str, cx: &mut Context<Self>) {
        cx.notify();
        let Some(backend) = self.backend.clone() else {
            return;
        };
        match args.trim() {
            "" => self.push_note("info", backend.corpus_status()),
            "now" => backend.embed(),
            "off" => match backend.set_embedding_model(None) {
                Ok(()) => self.push_note("info", "Documents are searched by keyword only."),
                Err(e) => self.push_note("error", format!("embedding: {e}")),
            },
            spec => match backend.set_embedding_model(Some(spec)) {
                Ok(()) => self.push_note(
                    "info",
                    format!("Embedding model is {spec}. The text of the documents is being sent to its provider to be embedded; documents are searched by keyword until it has finished."),
                ),
                Err(e) => self.push_note("error", format!("embedding: {e}")),
            },
        }
    }

    /// Whether the window may close now. An unsaved state with something in it asks
    /// first; an empty one is simply dropped, so opening a folder by accident leaves
    /// nothing behind.
    pub fn should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.closing {
            return true;
        }
        // Sessions that are working would be cut off: ask first.
        if !self.interrupted && !self.working_ids().is_empty() {
            self.ask_before_interrupting(window, cx);
            return false;
        }
        let Some(b) = self.backend.clone() else {
            return true;
        };
        if !b.is_unsaved() {
            return true;
        }
        if !b.has_work() {
            self.finish_unsaved("discard", &b);
            return true;
        }
        self.ask_before_close(window, cx);
        false
    }

    /// The sessions open in this window that have a turn running now.
    pub fn working_ids(&self) -> Vec<String> {
        self.open
            .iter()
            .filter(|o| {
                // The active session's backend is the live one; the same Arc as its bundle's.
                o.bundle.backend.as_ref().is_some_and(|b| b.is_running())
            })
            .map(|o| o.id.clone())
            .collect()
    }

    /// Closing the window while a session works: say so, and let the person interrupt it or
    /// keep the window.
    fn ask_before_interrupting(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        let working = self.working_ids();
        let names = working
            .iter()
            .map(|id| crate::session_list::short_id(id))
            .collect::<Vec<_>>()
            .join(", ");
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let names = names.clone();
            let n = working.len();
            let (keep, stop) = (weak.clone(), weak.clone());
            dialog
                .w(px(560.))
                .close_button(false)
                .overlay_closable(false)
                .title(if n == 1 {
                    "A session is still working"
                } else {
                    "Sessions are still working"
                })
                .content(move |content, _window, cx| {
                    content.child(div().text_color(cx.theme().muted_foreground).child(format!(
                        "Working: {names}. Closing now interrupts {}. What it has done so far stays in the session log, and the session can be opened again.",
                        if n == 1 { "it" } else { "them" }
                    )))
                })
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(Button::new("interrupt-keep").label("Keep working").on_click(
                            move |_, window, cx| {
                                use gpui_kit::component::WindowExt as _;
                                window.close_dialog(cx);
                                let _ = keep.update(cx, |_, cx| cx.notify());
                            },
                        ))
                        .child(
                            Button::new("interrupt-stop")
                                .label("Interrupt and close")
                                .danger()
                                .on_click(move |_, window, cx| {
                                    let _ = stop.update(cx, |app, cx| {
                                        app.interrupt_and_close(window, cx)
                                    });
                                }),
                        ),
                )
        });
        cx.notify();
    }

    /// Stop every session that is working, wait for them to settle (a few seconds at most),
    /// and then close the window as it would have: the question about an unsaved state
    /// follows, if there is one.
    pub fn interrupt_and_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
        }
        for o in &self.open {
            if let Some(b) = &o.bundle.backend
                && b.is_running()
            {
                b.cancel();
            }
        }
        self.push_note("info", "interrupting before closing…");
        cx.spawn_in(window, async move |this, cx| {
            for _ in 0..50 {
                let still = this
                    .update(cx, |app, _| !app.working_ids().is_empty())
                    .unwrap_or(false);
                if !still {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
            let _ = this.update_in(cx, |app, window, cx| {
                app.interrupted = true;
                app.request_quit(window, cx);
            });
        })
        .detach();
    }

    /// Hand the unsaved state to a helper that acts once this window has let go of it.
    fn finish_unsaved(&mut self, verb: &str, b: &Backend) {
        let args: Vec<std::ffi::OsString> = vec![
            "case".into(),
            verb.into(),
            b.case.root().as_os_str().to_owned(),
            "--wait".into(),
        ];
        if let Err(e) = spawn_self(&args) {
            // Nothing is lost: the unsaved state stays where it is and is offered again
            // the next time this folder is opened.
            eprintln!("reactor-gui: could not {verb} the unsaved state: {e}");
        }
        self.closing = true;
    }

    fn ask_before_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        let Some(b) = self.backend.clone() else {
            return;
        };
        let folder = b.case.root().display().to_string();
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let folder = folder.clone();
            let button = |id: &'static str, label: &'static str, choice: CloseChoice| {
                let weak = weak.clone();
                Button::new(id).label(label).on_click(move |_, window, cx| {
                    let _ = weak.update(cx, |app, cx| app.close_with(choice, window, cx));
                })
            };
            dialog
                .w(px(560.))
                .close_button(false)
                .overlay_closable(false)
                .title("This folder is not a case yet")
                .content(move |content, _window, cx| {
                    content.child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "Save as a case to keep this session in {folder}/.reactor, or delete it. Files the agent wrote into the folder stay either way."
                            )),
                    )
                })
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(button("close-cancel", "Cancel", CloseChoice::Cancel))
                        .child(button("close-delete", "Delete", CloseChoice::Delete).danger())
                        .child(button("close-save", "Save as Case", CloseChoice::Save).primary()),
                )
        });
        cx.notify();
    }

    /// Carry out the answer to [`Self::ask_before_close`].
    pub fn close_with(&mut self, choice: CloseChoice, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
        }
        let Some(b) = self.backend.clone() else {
            return;
        };
        match choice {
            CloseChoice::Cancel => return,
            CloseChoice::Save => self.finish_unsaved("save", &b),
            CloseChoice::Delete => self.finish_unsaved("discard", &b),
        }
        cx.quit();
    }

    /// Quit, asking first when there is an unsaved state to decide about.
    pub fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.should_close(window, cx) {
            cx.quit();
        }
    }

    // -- the memory switches (ADR-0047) -------------------------------------------------------

    /// The switches in effect for this session: (other sessions' history, the case graph,
    /// the documents).
    pub fn memory(&self) -> (bool, bool, bool) {
        self.backend
            .as_ref()
            .map(|b| b.memory())
            .map(|m| (m.session_history(), m.case_graph(), m.documents()))
            .unwrap_or((true, true, true))
    }

    /// Set one of them for this session.
    pub fn set_memory(
        &mut self,
        change: reactor_context::settings::MemorySettings,
        cx: &mut Context<Self>,
    ) {
        if let Some(b) = self.backend.clone()
            && let Err(e) = b.set_session_memory(change)
        {
            self.push_note("error", format!("could not change the switch: {e}"));
        }
        cx.notify();
    }

    /// Open the main window: theme, dock, agent, event pump — in the order the spec's §6
    /// anatomy needs.
    pub fn open(cx: &mut App, args: &crate::start::ResolvedLaunch) -> anyhow::Result<()> {
        cx.bind_keys(vec![KeyBinding::new("escape", ComposerEsc, None)]);

        // Record the workdir as recent here: this is the one choke point every launch path
        // funnels through (a positional cwd, the chooser, the session picker).
        let mut config = crate::start::GuiConfig::load();
        config.push_workdir(&args.cwd);

        let bounds = gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds::centered(
            None,
            gpui_kit::size(px(1420.), px(920.)),
            cx,
        ));
        let mut app_entity: Option<Entity<ReactorApp>> = None;
        let _handle = cx.open_window(crate::chrome::window_options(bounds), |window, cx| {
            crate::theme::install_ayu_dark(cx);
            let app: Entity<ReactorApp> = cx.new(|cx| ReactorApp::new(window, cx, args));
            // Only a window knows its renderer: a software one switches the light mode on (Auto).
            crate::settings::detect_adapter(window, &crate::start::GuiConfig::load_ui(), cx);
            app_entity = Some(app.clone());
            cx.set_global(MainWindow {
                handle: window.window_handle(),
                app: app.downgrade(),
            });
            // Closing a folder that is not a case yet asks first: save it as one, or
            // delete what was said (ADR-0045).
            let weak = app.downgrade();
            window.on_window_should_close(cx, move |window, cx| {
                weak.update(cx, |app, cx| app.should_close(window, cx))
                    .unwrap_or(true)
            });
            let root_view: gpui_kit::AnyView = app.into();
            cx.new(|cx| Root::new(root_view, window, cx))
        })?;
        cx.activate(true);

        let app = app_entity.expect("window's root view was built");
        app.update(cx, |app, cx| {
            if let Some(note) = STARTUP_NOTE.lock().unwrap().take() {
                app.push_note("error", note);
            }
            app.start_pump(cx);
            app.start_mcp_watch(cx);
            app.start_notice_clock(cx);
            app.refresh_catalogue(cx);
            app.refresh_services(false, cx);
            app.refresh_tree(cx);
            app.refresh_sessions(cx);
            app.refresh_statuses(cx);
            if let Some(b) = &app.backend {
                b.refresh_context();
            }
        });
        Ok(())
    }

    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        args: &crate::start::ResolvedLaunch,
    ) -> Self {
        let session = cx.new(|_| Session::new());
        let composer = Self::make_composer(window, cx);
        let cwd = args.cwd.clone();
        let workdir_fs = Arc::new(reactor_fs::LocalFs::new(
            &cwd,
            cwd.file_name().map_or_else(
                || cwd.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
        ));

        // The agent. A failure to start is shown, not fatal: the window stays open. Started
        // before the panels because the transcript tab is the session's.
        let opts = StartOptions {
            cwd: cwd.clone(),
            resume: args.resume_session(&reactor_core::Paths::from_env()),
            model: args.launch_args.model.clone(),
            shared: None,
        };
        let (backend, startup_error) = match Backend::start(opts) {
            Ok(b) => (Some(Arc::new(b)), None),
            Err(e) => (None, Some(format!("could not start the agent: {e}"))),
        };
        let first_id = backend
            .as_ref()
            .map(|b| b.session_id.clone())
            .unwrap_or_default();

        let weak_app = cx.weak_entity();
        let (dock_area, _skin) = DockSkin::dock_area("reactor-main", Some(1), window, cx);
        // Built once and kept as handles: a layout preset rearranges these rather than
        // building new panels, so switching never costs the console its scrollback or the
        // transcript its scroll position (crate::layout).
        let transcript = cx.new(|cx| {
            TranscriptPanel::new(
                weak_app.clone(),
                first_id.clone(),
                session.clone(),
                composer.clone(),
                window,
                cx,
            )
        });
        let primary_transcript = transcript.clone();
        let transcript_slot = Arc::new(std::sync::Mutex::new(transcript.clone()));
        let slot_for_remove = transcript_slot.clone();
        let tree = cx.new(|cx| TreePanel::new(weak_app.clone(), window, cx));
        let tree_panel = tree.clone();
        let tools = cx.new(|cx| ToolsPanel::new(weak_app.clone(), window, cx));
        let toolsets = cx.new(|cx| ToolsetsPanel::new(weak_app.clone(), window, cx));
        let context = cx.new(|cx| ContextPanel::new(weak_app.clone(), window, cx));
        let services = cx.new(|cx| ServicesPanel::new(weak_app.clone(), window, cx));
        let files = cx.new(|cx| {
            crate::files::explorer::FilesPanel::new(
                weak_app.clone(),
                workdir_fs.clone(),
                window,
                cx,
            )
        });
        let console =
            cx.new(|cx| ConsolePanel::new(weak_app.clone(), Some(cwd.clone()), None, window, cx));
        let graph = cx.new(|cx| crate::graph::panel::GraphPanel::new(weak_app.clone(), window, cx));
        let graph_panel = graph.clone();
        let docs = cx.new(|cx| crate::docs_panel::DocsPanel::new(weak_app.clone(), window, cx));
        let docs_panel = docs.clone();
        let panels = crate::layout::Panels {
            transcript: panel_handle(transcript.clone()),
            tree: panel_handle(tree.clone()),
            tools: panel_handle(tools.clone()),
            toolsets: panel_handle(toolsets.clone()),
            context: panel_handle(context.clone()),
            services: panel_handle(services.clone()),
            files: panel_handle(files.clone()),
            console: panel_handle(console.clone()),
            graph: panel_handle(graph.clone()),
            docs: panel_handle(docs.clone()),
            // `DockArea` removes by typed entity; the handles above have lost the type.
            remove: std::sync::Arc::new(move |kind, area, window, cx| {
                use crate::layout::PanelKind::*;
                match kind {
                    Transcript => {
                        let current = slot_for_remove.lock().unwrap().clone();
                        area.remove_panel(current, window, cx)
                    }
                    Tree => area.remove_panel(tree.clone(), window, cx),
                    Tools => area.remove_panel(tools.clone(), window, cx),
                    Toolsets => area.remove_panel(toolsets.clone(), window, cx),
                    Context => area.remove_panel(context.clone(), window, cx),
                    Services => area.remove_panel(services.clone(), window, cx),
                    Files => area.remove_panel(files.clone(), window, cx),
                    Console => area.remove_panel(console.clone(), window, cx),
                    Graph => area.remove_panel(graph.clone(), window, cx),
                    Docs => area.remove_panel(docs.clone(), window, cx),
                }
            }),
        };
        let ui = crate::start::GuiConfig::load_ui();
        let layout = ui.start_layout();
        layout.apply(&panels, &dock_area, window, cx);

        // The graph panel reads the case through the backend, which exists only now.
        graph_panel.update(cx, |g, cx| g.set_backend(backend.clone(), cx));
        docs_panel.update(cx, |d, cx| d.set_backend(backend.clone(), cx));
        let reactor = match &backend {
            Some(b) => Client::for_session(b.paths.clone(), Some(cwd.clone())),
            None => Client::from_env(Some(cwd.clone())),
        };
        if let Some(b) = &backend {
            // A resumed session's transcript is its log.
            let store = b.store();
            session.update(cx, |s, _| {
                s.rebuild(&store.lock().unwrap());
                s.model = Some(b.model()).filter(|m| !m.is_empty());
            });
        }
        if let Some(message) = startup_error {
            session.update(cx, |s, _| {
                s.last_error = Some(message.clone());
                s.items.push(ChatItem::Error { message });
            });
        }

        let scenarios_dir = crate::backend::find_bundled("prompts/scenarios")
            .unwrap_or_else(|| PathBuf::from("prompts/scenarios"));
        let models = backend.as_ref().map(|b| b.models()).unwrap_or_default();
        let tree = backend.as_ref().map(|b| b.tree()).unwrap_or_default();
        let leaf_id = backend.as_ref().map(|b| b.store().lock().unwrap().head());
        // A resumed session brings its earlier messages with it.
        let prompt_history = backend
            .as_ref()
            .map(|b| {
                b.store()
                    .lock()
                    .unwrap()
                    .branch()
                    .into_iter()
                    .filter_map(|e| {
                        if let reactor_agent::entry::Kind::User { text } = &e.kind {
                            Some(text.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<String>>()
            })
            .map(crate::prompt_history::PromptHistory::from_messages)
            .unwrap_or_default();

        // The session this window opened with is the first of those open, and the active one.
        let open = backend
            .as_ref()
            .map(|b| OpenSession {
                id: first_id.clone(),
                bundle: Bundle {
                    backend: Some(b.clone()),
                    session: session.clone(),
                    composer: composer.clone(),
                    tree: tree.clone(),
                    leaf_id,
                    context: None,
                    preview: None,
                    context_busy: false,
                },
                handle: panel_handle(primary_transcript.clone()),
                transcript: primary_transcript.clone(),
                primary: true,
            })
            .into_iter()
            .collect();

        Self {
            menu_bar: crate::chrome::menu_bar(cx),
            workdir_fs,
            host_fs: Arc::new(reactor_fs::LocalFs::new("/", "/")),
            cwd,
            backend,
            embedding: None,
            graph_panel,
            docs_panel,
            tree_panel,
            reactor,
            session,
            composer,
            notifier: crate::notifications::Notifier::default(),
            pending_toasts: Vec::new(),
            live_toasts: Vec::new(),
            notices: None,
            models,
            catalogue: None,
            toolsets: None,
            services: None,
            activation_scope: None,
            // Set once here rather than left to default-false: `open()` kicks off the
            // first fetch of each right after construction, so a panel's first-ever
            // render already says "loading", never a beat of "no data" first.
            catalogue_loading: true,
            services_loading: true,
            tree,
            leaf_id,
            sessions: Vec::new(),
            open,
            active: first_id,
            transcript_slot,
            context: None,
            preview: None,
            context_busy: false,
            scenarios_dir,
            panels,
            layout,
            dock_area,
            palette: None,
            usage: crate::start::GuiConfig::load().command_usage,
            slash: SlashUi::default(),
            file_index: Default::default(),
            prompt_history,
            ui,
            inspect: None,
            family_drafts: Default::default(),
            model_draft: Default::default(),
            reembed_armed: false,
            identity_remove_armed: None,
            mcp_remove_armed: None,
            scenario: Default::default(),
            scenario_pick: None,
            plan: Default::default(),
            goal_editor: None,
            frame_popup: None,
            embedding_draft: crate::settings_window::ModelDraft {
                // The one that needs no key and keeps the text on this machine.
                provider: "ollama".into(),
                name: String::new(),
            },
            context_scope: crate::settings_window::ContextScope::Default,
            settings_window: None,
            closing: false,
            interrupted: false,
        }
    }

    /// Look for a change in the MCP servers' standing and refresh what shows tools. A server
    /// connects in the background, so the panels learn of it by asking, not by being told.
    fn start_mcp_watch(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.mcp_host() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let mut seen = 0;
            loop {
                cx.background_executor().timer(PUMP_TICK * 4).await;
                let now = host.generation();
                if now == seen {
                    continue;
                }
                seen = now;
                let ok = this
                    .update(cx, |app, cx| {
                        app.refresh_catalogue(cx);
                        if let Some(b) = &app.backend {
                            b.refresh_context();
                        }
                        cx.notify();
                    })
                    .is_ok();
                if !ok {
                    break;
                }
            }
        })
        .detach();
    }

    /// The event pump (SPEC.md §2): drain the backend's channel every tick and ingest.
    /// A poll loop, not a blocking recv — the pump future runs on gpui's foreground
    /// executor and only ever sleeps on a background timer.
    fn start_pump(&mut self, cx: &mut Context<Self>) {
        let Some(receiver) = self.backend.as_ref().and_then(|b| b.take_events()) else {
            return;
        };
        let id = self.active.clone();
        self.start_pump_for(id, receiver, cx);
    }

    /// One pump per open session: its events are ingested for it, whether or not it is the
    /// active one.
    fn start_pump_for(
        &mut self,
        id: String,
        receiver: mpsc::Receiver<UiEvent>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PUMP_TICK).await;
                let mut drained: Vec<UiEvent> = Vec::new();
                let mut disconnected = false;
                loop {
                    match receiver.try_recv() {
                        Ok(e) => drained.push(e),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
                let ok = this
                    .update(cx, |app, cx| {
                        for e in drained {
                            app.ingest_for(&id, e, cx);
                        }
                    })
                    .is_ok();
                if !ok || disconnected {
                    break;
                }
            }
        })
        .detach();
    }

    /// Ingest one backend event: the session view model learns, and what a panel shows
    /// that is derived from the store (tree, statuses, context) is recomputed.
    fn ingest(&mut self, event: UiEvent, cx: &mut Context<Self>) {
        use reactor_agent::agent::Event;
        let Some(backend) = self.backend.clone() else {
            return;
        };
        match event {
            UiEvent::Agent(e) => {
                match &e {
                    Event::Notice(n) => self.push_note("warning", n.clone()),
                    Event::Reduced { mode, trigger, before_tokens, after_tokens, .. } => self.push_note(
                        "info",
                        format!("context reduced ({mode:?}, {}): ~{before_tokens} → ~{after_tokens} tokens", crate::backend::trigger_label(*trigger)),
                    ),
                    _ => {}
                }
                let store = backend.store();
                self.session
                    .update(cx, |s, _| s.apply(&e, &store.lock().unwrap()));
                if matches!(e, Event::Appended(_)) {
                    self.refresh_tree(cx);
                    self.refresh_statuses(cx);
                }
                // A tool may have written to the case's graph: show it as it happens.
                if matches!(e, Event::ToolEnd { .. }) {
                    self.graph_panel.update(cx, |g, cx| g.refresh(cx));
                }
            }
            UiEvent::TurnEnded(result) => {
                backend.turn_finished();
                self.refresh_sessions(cx);
                self.tree_panel.update(cx, |_, cx| cx.notify());
                self.graph_panel.update(cx, |g, cx| g.refresh(cx));
                let store = backend.store();
                let ok = result.is_ok();
                // A finished prompt is annotated: when, how long, how much it streamed. It goes
                // in the log, so a resumed session shows it too.
                if let Some(run) = self.session.update(cx, |s, _| s.run.take())
                    && ok
                {
                    let now = chrono::Local::now();
                    let data = serde_json::json!({
                        "date": now.format("%Y-%m-%d").to_string(),
                        "time": now.format("%H:%M:%S").to_string(),
                        "duration_ms": run.elapsed(std::time::Instant::now()).as_millis() as u64,
                        "tokens": run.tokens(),
                        "stream_ms": run.stream_time().as_millis() as u64,
                    });
                    let _ = store
                        .lock()
                        .unwrap()
                        .append(reactor_agent::entry::Kind::Custom {
                            key: crate::session::STATS_KEY.to_string(),
                            data,
                        });
                }
                self.session
                    .update(cx, |s, _| s.end_turn(&store.lock().unwrap()));
                match result {
                    Ok(_) => {}
                    Err(e) if e == "cancelled" => self.push_note("info", "interrupted"),
                    Err(e) => self.session.update(cx, |s, _| {
                        s.last_error = Some(e.clone());
                        s.items.push(ChatItem::Error { message: e });
                    }),
                }
                self.refresh_tree(cx);
                self.refresh_statuses(cx);
                backend.refresh_context();
                // Messages scheduled while it ran go next, oldest first.
                if ok
                    && let Some(next) = self.session.update(cx, |s, _| {
                        (!s.follow_up.is_empty()).then(|| s.follow_up.remove(0))
                    })
                {
                    self.start_turn(next, cx);
                }
            }
            UiEvent::Reduced(result) => {
                self.context_busy = false;
                let store = backend.store();
                self.session.update(cx, |s, _| {
                    s.phase = AgentPhase::Idle;
                    s.rebuild(&store.lock().unwrap());
                });
                match result {
                    Ok(id) => self.push_note(
                        "info",
                        format!("reduced (#{id}) — undo it from the Context panel"),
                    ),
                    Err(e) => self.push_note("error", e),
                }
                self.refresh_tree(cx);
            }
            UiEvent::Inspect(result) => {
                if let Some(i) = &mut self.inspect {
                    i.loading = false;
                    match result {
                        Ok(p) => {
                            i.error = None;
                            if i.selected >= p.segments.len() {
                                i.selected = 0;
                            }
                            i.data = Some(p);
                            i.select_in_filter();
                        }
                        Err(e) => i.error = Some(e),
                    }
                }
            }
            UiEvent::Preview(result) => {
                self.context_busy = false;
                match result {
                    Ok(p) => self.preview = Some(p),
                    Err(e) => {
                        self.preview = None;
                        self.push_note("warning", e);
                    }
                }
            }
            UiEvent::Context(view) => {
                self.session
                    .update(cx, |s, _| s.context_percent = Some(view.percent()));
                self.context = Some(view);
            }
            UiEvent::Notice { kind, text } => {
                // An import ends with a notice: what is in the case has changed.
                self.docs_panel.update(cx, |d, cx| d.refresh(cx));
                self.push_note(kind, text)
            }
            UiEvent::Embedding(progress) => {
                // The search choices change when the vectors are complete.
                if progress.is_none_or(|p| p.complete()) {
                    self.docs_panel.update(cx, |d, cx| d.refresh(cx));
                }
                self.embedding = progress;
            }
        }
        cx.notify();
    }

    /// Tell the person something: a popup (gpui-kit's notification, for `ui.notice_seconds`) and
    /// a line in the history behind the bell. `kind` is `info`, `warning` or `error`.
    pub fn push_note(&mut self, kind: impl Into<SharedString>, message: impl Into<SharedString>) {
        let level = crate::notifications::Level::from_kind(&kind.into());
        let message = message.into().to_string();
        let now = chrono::Local::now();
        let (date, time) = (
            now.format("%Y-%m-%d").to_string(),
            now.format("%H:%M:%S").to_string(),
        );
        let id = self.notifier.push(level, message.clone(), date, time);
        self.pending_toasts.push((id, level, message));
    }

    /// Show what is waiting as popups, and take down the ones whose time has come. Popups need a
    /// window to be pushed into, which `push_note` does not have, so a clock does this
    /// (`start_notice_clock`). The component's own auto-hide is a fixed 5 s, so popups are pushed
    /// without it and removed here at the configured time.
    fn flush_toasts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::notifications::Level;
        use gpui_kit::component::WindowExt as _;
        use gpui_kit::component::notification::Notification;
        let timeout = Duration::from_secs(u64::from(self.ui.notice_seconds));
        let now = std::time::Instant::now();
        let settings = self.settings_window;

        for (id, level, message) in std::mem::take(&mut self.pending_toasts) {
            let note = || {
                let n = match level {
                    Level::Error => Notification::error(message.clone()),
                    Level::Warning => Notification::warning(message.clone()),
                    Level::Info => Notification::info(message.clone()),
                };
                n.id1::<NoticeKind>(id as usize).autohide(false)
            };
            window.push_notification(note(), cx);
            if let Some(h) = settings {
                let _ = h.update(cx, |_, w, cx| w.push_notification(note(), cx));
            }
            self.live_toasts.push((id, now + timeout));
        }

        let (due, live): (Vec<_>, Vec<_>) = std::mem::take(&mut self.live_toasts)
            .into_iter()
            .partition(|(_, deadline)| *deadline <= now);
        self.live_toasts = live;
        for (id, _) in due {
            window.remove_notification1::<NoticeKind>(id as usize, cx);
            if let Some(h) = settings {
                let _ = h.update(cx, |_, w, cx| {
                    w.remove_notification1::<NoticeKind>(id as usize, cx)
                });
            }
        }
    }

    /// The clock that shows and takes down popups.
    fn start_notice_clock(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut ticks = 0u32;
            loop {
                cx.background_executor().timer(INDICATOR_TICK).await;
                ticks = ticks.wrapping_add(1);
                if this.upgrade().is_none() {
                    break;
                }
                let main = cx.update(|cx| cx.try_global::<MainWindow>().cloned());
                let Some(main) = main else { continue };
                cx.update(|cx| {
                    let _ = cx.update_window(main.handle, |_, window, cx| {
                        if let Some(app) = main.app.upgrade() {
                            app.update(cx, |app, cx| {
                                if ticks.is_multiple_of(TOAST_TICKS) {
                                    app.flush_toasts(window, cx);
                                }
                                // The working indicators (a shimmer, a pulse) and the running
                                // prompt's clock are drawn from the wall clock, so a redraw is
                                // all they need: about sixty a second normally, about twelve
                                // where drawing is done by the CPU. Nothing redraws while
                                // nothing works.
                                let busy = app.session.read(cx).run.is_some()
                                    || !app.working_ids().is_empty();
                                let every = if crate::settings::LightRendering::on(cx) {
                                    LIGHT_INDICATOR_TICKS
                                } else {
                                    1
                                };
                                if busy && ticks.is_multiple_of(every) {
                                    cx.notify();
                                }
                            });
                        }
                    });
                });
            }
        })
        .detach();
    }

    /// Open the notification history, or close it if it is open.
    pub fn toggle_notices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.notices.is_some() {
            self.close_notices(window, cx);
            return;
        }
        let focus = cx.focus_handle();
        self.notices = Some(crate::notices_ui::NoticesUi {
            focus: focus.clone(),
        });
        self.notifier.mark_read();
        window.defer(cx, move |window, cx| window.focus(&focus, cx));
        cx.notify();
    }

    pub fn close_notices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.notices.take().is_some() {
            self.composer
                .update(cx, |state, cx| state.focus(window, cx));
            cx.notify();
        }
    }

    pub fn clear_notices(&mut self, cx: &mut Context<Self>) {
        self.notifier.clear();
        cx.notify();
    }

    // -- derived data ------------------------------------------------------------------------

    /// Read the case's sessions again.
    pub fn refresh_sessions(&mut self, cx: &mut Context<Self>) {
        if let Some(b) = &self.backend {
            let here: Vec<String> = self.open.iter().map(|o| o.id.clone()).collect();
            self.sessions = crate::session_list::session_rows(&b.case, &self.active, &here);
        }
        cx.notify();
    }

    // -- sessions open in the window ---------------------------------------------------------

    /// The session the window follows: its tree, its context, its status, its switches.
    pub fn active_session_id(&self) -> String {
        self.active.clone()
    }

    /// The case's id, which with a session's number names the session anywhere:
    /// `<case id>#<number>`.
    pub fn case_id(&self) -> String {
        self.backend
            .as_ref()
            .map(|b| b.case.meta().id.clone())
            .unwrap_or_default()
    }

    /// A prompt box. Enter sends (queued while a turn runs); typing offers the `/` commands.
    fn make_composer(window: &mut Window, cx: &mut Context<Self>) -> Entity<TextareaState> {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 8)
                .submit_on_enter(true)
                .placeholder("Prompt…")
        });
        cx.subscribe_in(
            &composer,
            window,
            |this, composer, event: &InputEvent, window, cx| {
                // The prompt that is typed in belongs to the active session, whatever was
                // active a moment ago.
                if *composer != this.composer
                    && let Some(id) = this
                        .open
                        .iter()
                        .find(|o| o.bundle.composer == *composer)
                        .map(|o| o.id.clone())
                {
                    this.activate_session(&id, window, cx);
                }
                match event {
                    InputEvent::PressEnter { shift, .. } if !*shift => {
                        this.send_composer(window, cx)
                    }
                    InputEvent::Change => this.refresh_slash(true, cx),
                    _ => {}
                }
            },
        )
        .detach();
        composer
    }

    /// The active session's own state, as it is now.
    fn live_bundle(&self) -> Bundle {
        Bundle {
            backend: self.backend.clone(),
            session: self.session.clone(),
            composer: self.composer.clone(),
            tree: self.tree.clone(),
            leaf_id: self.leaf_id,
            context: self.context.clone(),
            preview: self.preview.clone(),
            context_busy: self.context_busy,
        }
    }

    /// Put the active session's state away in its place in the list.
    fn store_live(&mut self) {
        let live = self.live_bundle();
        if let Some(o) = self.open.iter_mut().find(|o| o.id == self.active) {
            o.bundle = live;
        }
    }

    /// Make a session's state the one the window's fields hold.
    fn load_bundle(&mut self, id: &str) -> bool {
        let Some(b) = self
            .open
            .iter()
            .find(|o| o.id == id)
            .map(|o| o.bundle.clone())
        else {
            return false;
        };
        self.backend = b.backend;
        self.session = b.session;
        self.composer = b.composer;
        self.tree = b.tree;
        self.leaf_id = b.leaf_id;
        self.context = b.context;
        self.preview = b.preview;
        self.context_busy = b.context_busy;
        self.active = id.to_string();
        true
    }

    /// Make a session the active one: the tree, the context, the status bar and the graph
    /// panel's edits follow it from now on.
    pub fn activate_session(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.active == id || !self.open.iter().any(|o| o.id == id) {
            return;
        }
        self.store_live();
        self.load_bundle(id);
        // What belonged to the other one's prompt and context view.
        self.slash = SlashUi::default();
        self.inspect = None;
        self.refresh_tree(cx);
        self.refresh_sessions(cx);
        if let Some(b) = &self.backend {
            b.refresh_context();
        }
        let backend = self.backend.clone();
        self.graph_panel
            .update(cx, |g, cx| g.set_backend(backend.clone(), cx));
        self.docs_panel
            .update(cx, |d, cx| d.set_backend(backend, cx));
        // Its tab's title says which one is active.
        let _ = window;
        cx.notify();
    }

    /// Run `f` as if `id` were the active session, and put things back: how an event for a
    /// session that is not in front is handled with the code that handles the active one's.
    fn with_session<R>(&mut self, id: &str, f: impl FnOnce(&mut Self) -> R) -> Option<R> {
        if id == self.active {
            return Some(f(self));
        }
        if !self.open.iter().any(|o| o.id == id) {
            return None;
        }
        let before = self.active.clone();
        self.store_live();
        self.load_bundle(id);
        let result = f(self);
        self.store_live();
        self.load_bundle(&before);
        Some(result)
    }

    /// An event from session `id`'s backend.
    fn ingest_for(&mut self, id: &str, event: UiEvent, cx: &mut Context<Self>) {
        self.with_session(id, |app| app.ingest(event, cx));
    }

    /// Show a transcript tab: add it to the dock if it is not there, else bring it forward.
    fn show_transcript(
        &mut self,
        handle: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = handle.panel_id(cx);
        self.dock_area.update(cx, |area, cx| {
            if area.panel(panel).is_some() {
                area.select_panel(panel, window, cx);
            } else {
                area.add_panel_view(handle, DockPlacement::Center, None, window, cx);
            }
        });
    }

    /// Open a session of this case in a transcript tab of its own (a new session when `id`
    /// is `None`): the window keeps the ones it has. A session already open is brought
    /// forward, its tab put back if it was closed.
    pub fn open_session(
        &mut self,
        id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = &id
            && let Some(o) = self.open.iter().find(|o| o.id == *id)
        {
            let handle = o.handle.clone();
            self.show_transcript(handle, window, cx);
            self.activate_session(id, window, cx);
            return;
        }
        let Some(base) = self.backend.clone() else {
            self.push_note(
                "warning",
                "The agent did not start, so there is no case to open a session in.",
            );
            return;
        };
        let model = base.model();
        let started = Backend::start(StartOptions {
            cwd: self.cwd.clone(),
            resume: id,
            model: Some(model).filter(|m| !m.is_empty()),
            shared: Some(base.shared()),
        });
        let backend = match started {
            Ok(b) => Arc::new(b),
            Err(e) => {
                self.push_note("error", format!("could not open that session: {e}"));
                self.refresh_sessions(cx);
                return;
            }
        };
        let new_id = backend.session_id.clone();
        let session = cx.new(|_| Session::new());
        let store = backend.store();
        session.update(cx, |s, _| {
            s.rebuild(&store.lock().unwrap());
            s.model = Some(backend.model()).filter(|m| !m.is_empty());
        });
        let composer = Self::make_composer(window, cx);
        let weak = cx.weak_entity();
        let transcript = cx.new(|cx| {
            TranscriptPanel::new(
                weak,
                new_id.clone(),
                session.clone(),
                composer.clone(),
                window,
                cx,
            )
        });
        let handle = panel_handle(transcript.clone());
        let head = store.lock().unwrap().head();
        self.open.push(OpenSession {
            id: new_id.clone(),
            bundle: Bundle {
                backend: Some(backend.clone()),
                session,
                composer,
                tree: backend.tree(),
                leaf_id: Some(head),
                context: None,
                preview: None,
                context_busy: false,
            },
            transcript,
            handle: handle.clone(),
            primary: false,
        });
        if let Some(receiver) = backend.take_events() {
            self.start_pump_for(new_id.clone(), receiver, cx);
        }
        self.show_transcript(handle, window, cx);
        self.activate_session(&new_id, window, cx);
        self.refresh_sessions(cx);
    }

    /// Let go of a session this window has open: its tab goes and its lock is released. The
    /// session is kept in the case and can be opened again. Any session can be closed — the
    /// window's first too, when another is open to take its place as the one the layouts
    /// show — except the last one, and one that is working, which is left alone.
    pub fn close_session(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.open.iter().position(|o| o.id == id) else {
            return;
        };
        if self.open.len() == 1 {
            self.push_note(
                "info",
                "A window keeps one session open: open another first, or close the window.",
            );
            return;
        }
        let working = self.open[at]
            .bundle
            .backend
            .as_ref()
            .is_some_and(|b| b.is_running());
        if working {
            self.push_note(
                "warning",
                "That session is working. Interrupt it or let it finish first.",
            );
            return;
        }
        // Another one becomes the active one first, if this was.
        if self.active == id {
            let next = self
                .open
                .iter()
                .rev()
                .find(|o| o.id != id)
                .map(|o| o.id.clone());
            if let Some(next) = next {
                self.activate_session(&next, window, cx);
            }
        }
        if let Some(at) = self.open.iter().position(|o| o.id == id) {
            let closed = self.open.remove(at);
            // The tab the layouts place goes to another session: the one now in front.
            if closed.primary
                && let Some(next) = self.open.iter_mut().find(|o| o.id == self.active)
            {
                next.primary = true;
                self.panels.transcript = next.handle.clone();
                *self.transcript_slot.lock().unwrap() = next.transcript.clone();
            }
            self.dock_area.update(cx, |area, cx| {
                area.remove_panel(closed.transcript.clone(), window, cx)
            });
        }
        // What is left shows the session in front, its tab put back if it was closed.
        if let Some(handle) = self
            .open
            .iter()
            .find(|o| o.id == self.active)
            .map(|o| o.handle.clone())
        {
            self.show_transcript(handle, window, cx);
        }
        self.refresh_sessions(cx);
    }

    pub fn refresh_tree(&mut self, _cx: &mut Context<Self>) {
        if let Some(b) = &self.backend {
            self.tree = b.tree();
            self.leaf_id = Some(b.store().lock().unwrap().head());
        }
    }

    /// The status bar's left side, from the session's state modules.
    pub fn refresh_statuses(&mut self, cx: &mut Context<Self>) {
        let Some(b) = self.backend.clone() else {
            return;
        };
        let (state, settings) = (b.session_state(), Settings::load(&b.paths));
        let items = status_items(&state, &settings, &self.scenarios_dir);
        self.session.update(cx, |s, _| s.statuses = items);
        self.scenario = crate::scenario_ui::view_of(state.scenario.as_ref(), &self.scenarios_dir);
        self.plan = crate::plan_ui::plan_of(&state, &settings);
        cx.notify();
    }

    // -- the manifest and reporting settings (Settings window) ----------------------------

    /// The global settings, for the Manifest and Reporting pages.
    pub fn global_settings(&self) -> Settings {
        self.backend
            .as_ref()
            .map(|b| Settings::load(&b.paths))
            .unwrap_or_default()
    }

    /// Change a global setting and bring what shows it up to date.
    pub fn edit_settings(&mut self, edit: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        match backend.edit_settings(edit) {
            Ok(()) => {
                self.refresh_statuses(cx);
                backend.refresh_context();
            }
            Err(e) => self.push_note("error", format!("settings: {e}")),
        }
        cx.notify();
    }

    // -- the manifest and reporting (the Context panel's Plan tab) -------------------------

    /// Run a command of the agent's side quietly: what it says is not a notification, because
    /// the panel shows the result. The reason it failed is the `Err`.
    fn quiet_command(
        &mut self,
        name: &str,
        args: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let backend = self.backend.clone().ok_or("the agent is not running")?;
        let result = backend.command(name, args)?;
        if let Some(bad) = result.notices.iter().find(|n| {
            matches!(
                n.level,
                reactor_context::Level::Error | reactor_context::Level::Warning
            )
        }) {
            return Err(bad.message.clone());
        }
        self.refresh_statuses(cx);
        backend.refresh_context();
        Ok(())
    }

    /// The manifest on or off: the block and `update_steps` both go quiet when it is off.
    pub fn manifest_set(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Err(e) = self.quiet_command("manifest", if on { "on" } else { "off" }, cx) {
            self.push_note("warning", format!("manifest: {e}"));
        }
    }

    pub fn report_set(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Err(e) = self.quiet_command("report", if on { "on" } else { "off" }, cx) {
            self.push_note("warning", format!("reporting: {e}"));
        }
    }

    pub fn report_level(&mut self, level: u8, cx: &mut Context<Self>) {
        if let Err(e) = self.quiet_command("report", &format!("level {level}"), cx) {
            self.push_note("warning", format!("reporting: {e}"));
        }
    }

    /// Open the goal-and-guidelines editor.
    pub fn open_goal_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        let make =
            |text: &str, placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
                let text = text.to_string();
                cx.new(|cx| {
                    let mut s = TextareaState::new(window, cx).placeholder(placeholder);
                    s.set_value(&text, window, cx);
                    s
                })
            };
        let goal = make(
            self.plan.goal.as_deref().unwrap_or(""),
            "Find out …",
            window,
            cx,
        );
        let guidelines = make(
            self.plan.guidelines.as_deref().unwrap_or(""),
            "Read-only analysis. Label every inference.",
            window,
            cx,
        );
        let focus = goal.clone();
        self.goal_editor = Some(crate::manifest_ui::GoalEditor {
            goal,
            guidelines,
            error: None,
        });
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let (w_content, w_clear, w_save) = (weak.clone(), weak.clone(), weak.clone());
            dialog
                .w(px(640.))
                .title("Goal and guidelines")
                .on_close({
                    let weak = weak.clone();
                    move |_, _w, cx| {
                        let _ = weak.update(cx, |app, _| app.goal_editor = None);
                    }
                })
                .content(move |content, _w, cx| {
                    content.child(crate::manifest_ui::editor_view(&w_content, cx))
                })
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .child(
                            Button::new("goal-clear")
                                .label("Clear both")
                                .tooltip("Remove the goal and the guidelines")
                                .on_click(move |_, window, cx| {
                                    let _ = w_clear
                                        .update(cx, |app, cx| app.save_goal("", "", window, cx));
                                }),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("goal-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(Button::new("goal-save").label("Save").primary().on_click(
                            move |_, window, cx| {
                                let _ =
                                    w_save.update(cx, |app, cx| app.read_and_save_goal(window, cx));
                            },
                        )),
                )
        });
        window.defer(cx, move |window, cx| {
            focus.update(cx, |s, cx| s.focus(window, cx));
        });
        cx.notify();
    }

    fn read_and_save_goal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = &self.goal_editor else {
            return;
        };
        let (goal, guidelines) = (
            ed.goal.read(cx).value().to_string(),
            ed.guidelines.read(cx).value().to_string(),
        );
        self.save_goal(&goal, &guidelines, window, cx);
    }

    /// Store the goal and the guidelines (empty clears), and close the editor.
    pub fn save_goal(
        &mut self,
        goal: &str,
        guidelines: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::WindowExt as _;
        let arg = |t: &str| {
            let t = t.trim();
            if t.is_empty() {
                "clear".to_string()
            } else {
                t.to_string()
            }
        };
        let mut result = Ok(());
        if goal.trim() != self.plan.goal.as_deref().unwrap_or("") {
            result = self.quiet_command("goal", &arg(goal), cx);
        }
        if result.is_ok() && guidelines.trim() != self.plan.guidelines.as_deref().unwrap_or("") {
            result = self.quiet_command("guidelines", &arg(guidelines), cx);
        }
        match result {
            Ok(()) => {
                self.goal_editor = None;
                window.close_dialog(cx);
                self.push_note("info", "manifest: saved");
            }
            Err(e) => {
                if let Some(ed) = &mut self.goal_editor {
                    ed.error = Some(e);
                }
            }
        }
        cx.notify();
    }

    /// Open the Frame popup: what the model is told about the goal, and what is kept.
    pub fn open_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let (state, settings) = (backend.session_state(), Settings::load(&backend.paths));
        let frame = crate::plan_ui::frame_of(&state.manifest, &settings.manifest);
        let tokens = frame
            .sent
            .as_deref()
            .map(reactor_agent::context::estimate_tokens)
            .unwrap_or(0);
        self.frame_popup = Some(crate::manifest_ui::FramePopup {
            tab: 0,
            frame,
            tokens,
        });
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, _cx| {
            let (w_content, w_footer, w_close) = (weak.clone(), weak.clone(), weak.clone());
            let height = window.viewport_size().height * 0.5;
            dialog
                .w(px(720.))
                .title("The frame")
                .on_close(move |_, _w, cx| {
                    let _ = w_close.update(cx, |app, _| app.frame_popup = None);
                })
                .content(move |content, _w, cx| {
                    content.child(crate::manifest_ui::frame_view(&w_content, height, cx))
                })
                .footer(crate::manifest_ui::frame_footer(w_footer))
        });
        cx.notify();
    }

    pub fn close_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
        }
        self.frame_popup = None;
        cx.notify();
    }

    /// The text of the open Frame tab, to the clipboard.
    pub fn copy_frame(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &self.frame_popup {
            let text = match p.tab {
                0 => p.frame.sent.clone().unwrap_or_default(),
                _ => p.frame.state.clone(),
            };
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        }
    }

    // -- scenarios (the Context panel's Scenario section) ---------------------------------

    /// Start the chosen scenario: the same as `/reactor-scenario start <id>`, which opens with
    /// the first phase's briefing as a turn.
    pub fn scenario_start(&mut self, cx: &mut Context<Self>) {
        let id = self
            .scenario_pick
            .clone()
            .filter(|p| self.scenario.available.contains(p))
            .or_else(|| self.scenario.available.first().cloned());
        match id {
            Some(id) => self.backend_command("reactor-scenario", &format!("start {id}"), cx),
            None => self.push_note("warning", "scenario: there are no scenarios to start"),
        }
    }

    /// Make phase `n` (1-based) the current one, forwards or back. Only between turns: it
    /// would cut across what the agent is doing, and the next prompt is what starts the phase.
    pub fn scenario_goto(&mut self, n: usize, cx: &mut Context<Self>) {
        if self.session.read(cx).phase != AgentPhase::Idle {
            self.push_note(
                "info",
                "scenario: wait until the agent has finished, or interrupt it, to choose a phase",
            );
            return;
        }
        self.backend_command("reactor-scenario", &format!("goto {n}"), cx);
    }

    /// End the running scenario. Allowed at any time, a turn in progress included.
    pub fn scenario_abort(&mut self, cx: &mut Context<Self>) {
        self.backend_command("reactor-scenario", "stop", cx);
    }

    pub fn refresh_catalogue(&mut self, cx: &mut Context<Self>) {
        let reactor = self.reactor.clone();
        self.catalogue_loading = true;
        cx.spawn(async move |this, cx| {
            let (tools, toolsets, state) = cx
                .background_spawn(async move {
                    (
                        ReactorClient::tools(&reactor),
                        ReactorClient::toolsets(&reactor),
                        ReactorClient::state(&reactor),
                    )
                })
                .await;
            this.update(cx, |app, cx| {
                // A failure is said, not shown as an empty catalogue.
                for failure in [tools.as_ref().err(), toolsets.as_ref().err()]
                    .into_iter()
                    .flatten()
                    .take(1)
                {
                    app.push_note("error", format!("catalogue: {failure}"));
                }
                app.catalogue = tools.ok();
                app.toolsets = toolsets.ok();
                app.activation_scope = state.ok().and_then(|s| s.scope);
                app.catalogue_loading = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn refresh_services(&mut self, refresh: bool, cx: &mut Context<Self>) {
        let reactor = self.reactor.clone();
        self.services_loading = true;
        cx.spawn(async move |this, cx| {
            let services = cx
                .background_spawn(async move { ReactorClient::services(&reactor, refresh) })
                .await;
            this.update(cx, |app, cx| {
                match services {
                    Ok(payload) => app.services = Some(payload),
                    Err(failure) => app.push_note("error", format!("services: {failure}")),
                }
                app.services_loading = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Rearrange the window into a named preset (`crate::layout`).
    pub fn apply_layout(
        &mut self,
        preset: crate::layout::LayoutPreset,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        preset.apply(&self.panels, &self.dock_area, window, cx);
        self.layout = preset;
        cx.notify();
    }

    /// Show or hide one dock, from the Layout menu.
    pub fn toggle_dock(
        &mut self,
        side: crate::layout::DockSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::layout::toggle_dock(side, &self.dock_area, window, cx);
        cx.notify();
    }

    /// Ctrl+W / Cmd+W: close the tab in front — what its own close button does. "In front" is
    /// the displayed tab of the group the keyboard focus is in; with the focus nowhere in the
    /// dock, the front tab of the centre.
    pub fn close_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The palette is a dialog over the window: Ctrl+W there closes the dialog, not what is
        // behind it.
        if self.palette.is_some() {
            self.close_palette(window, cx);
            return;
        }
        let area = self.dock_area.clone();
        let target = {
            let area = area.read(cx);
            let mut fronts: Vec<(DockPlacement, Arc<dyn PanelView>)> = Vec::new();
            for placement in [
                DockPlacement::Center,
                DockPlacement::Left,
                DockPlacement::Right,
                DockPlacement::Bottom,
            ] {
                let Some(tree) = area.layout(placement) else {
                    continue;
                };
                tree.root().walk(&mut |node| {
                    if let gpui_kit::base::dock::PaneRef::Tabs { panels, active_ix } = node.kind()
                        && let Some(front) = panels
                            .get(active_ix)
                            .and_then(|id| area.panel(*id))
                    {
                        fronts.push((placement, front.clone()));
                    }
                });
            }
            let focused = fronts
                .iter()
                .find(|(_, panel)| panel.focus_handle(cx).contains_focused(window, cx))
                .cloned();
            focused.or_else(|| {
                fronts
                    .iter()
                    .find(|(placement, _)| *placement == DockPlacement::Center)
                    .cloned()
            })
        };
        let Some((_, panel)) = target else {
            return;
        };
        if !panel.closable(cx) {
            return;
        }
        let view = panel.view();
        let closed = area.update(cx, |area, cx| remove_any_panel(area, view, window, cx));
        if closed {
            cx.notify();
        }
    }

    /// Take one panel out of the dock — what a panel's own close button does.
    pub fn close_panel<P: gpui_kit::base::dock::Panel>(
        &mut self,
        panel: Entity<P>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dock_area
            .update(cx, |area, cx| area.remove_panel(panel, window, cx));
        cx.notify();
    }

    /// Close a panel that is open, open one that is closed — from the menu, the palette or a
    /// panel's own close button.
    pub fn toggle_panel(
        &mut self,
        kind: crate::layout::PanelKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::layout::toggle_panel(kind, self.layout, &self.panels, &self.dock_area, window, cx);
        cx.notify();
    }

    /// Open the agent's `docs_search` result in the Documents panel, with the passage at
    /// `index` selected. The passages are looked up in the case again, so what is shown is
    /// the stored text with its document and neighbours; any that cannot be found (the
    /// document was removed since) are left out.
    pub fn show_docs(
        &mut self,
        query: String,
        passages: Vec<reactor_agent::tools::docs::ResultPassage>,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use reactor_agent::tools::docs::defang;
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let mut hits = Vec::new();
        let mut chosen = 0;
        for (i, p) in passages.iter().enumerate() {
            let candidates = reactor_db::corpus::hits_at(backend.case.db(), &p.source, &p.section)
                .unwrap_or_default();
            let hit = candidates
                .iter()
                .find(|h| defang(&h.text) == p.text)
                .or(candidates.first())
                .cloned();
            if let Some(hit) = hit {
                if i == index {
                    chosen = hits.len();
                }
                hits.push(hit);
            }
        }
        let kind = crate::layout::PanelKind::Docs;
        let open = self.panels.is_open(kind, self.dock_area.read(cx), cx);
        if !open {
            self.toggle_panel(kind, window, cx);
        }
        let id = self.panels.view(kind).panel_id(cx);
        self.dock_area
            .update(cx, |area, cx| area.select_panel(id, window, cx));
        self.docs_panel
            .update(cx, |d, cx| d.show(query, hits, chosen, window, cx));
        cx.notify();
    }

    // -- the console --------------------------------------------------------

    /// Open a new terminal: another `ConsolePanel`, tabbed alongside any that already
    /// exist, in the bottom dock (created if need be). `initial`, when given, is a command
    /// run immediately — how installing a catalogued tool opens its own terminal already
    /// running `reactor install <id>`.
    pub fn open_console(
        &mut self,
        initial: Option<(String, Vec<String>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak_app = cx.weak_entity();
        let cwd = Some(self.cwd.clone());
        let panel =
            cx.new(|cx| crate::panels::ConsolePanel::new(weak_app, cwd, initial, window, cx));
        let handle = panel_handle(panel);
        self.dock_area.update(cx, |area, cx| {
            area.add_panel_view(handle, DockPlacement::Bottom, None, window, cx);
            // A collapsed dock would swallow the terminal that was just asked for.
            if area.has_dock(DockPlacement::Bottom) && !area.is_dock_open(DockPlacement::Bottom) {
                area.toggle_dock(DockPlacement::Bottom, window, cx);
            }
        });
    }

    // -- files ----------------------------------------------------------------

    /// The file a path in text names: relative paths are the working directory's, absolute
    /// ones inside it become relative to it, and anything else is the host's own disk.
    pub fn resolve_file(&self, text: &str) -> reactor_fs::FileRef {
        let text = text.trim().trim_matches(['"', '\'', '`']);
        let native = std::path::Path::new(text);
        let native = if native.is_absolute() {
            native.to_path_buf()
        } else {
            self.cwd.join(native)
        };
        match self.workdir_fs.virtual_path(&native) {
            Some(path) => reactor_fs::FileRef::new(self.workdir_fs.clone(), path),
            None => {
                let path = self
                    .host_fs
                    .virtual_path(&native)
                    .unwrap_or_else(reactor_fs::VPath::root);
                reactor_fs::FileRef::new(self.host_fs.clone(), path)
            }
        }
    }

    /// A click on a file reference in the transcript. Plain click: the viewer that suits the
    /// file. With Shift the hex viewer, with Ctrl/Cmd the text editor, with Alt a popup.
    pub fn open_reference(
        &mut self,
        path: &str,
        line: Option<u32>,
        modifiers: gpui_kit::Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::files::Viewer;
        let file =
            self.resolve_file(&crate::files::refs::native_path(&self.cwd, path).to_string_lossy());
        if modifiers.alt {
            crate::files::data::open_popup(file, window, cx);
            return;
        }
        let viewer = if modifiers.shift {
            Some(Viewer::Hex)
        } else if modifiers.secondary() {
            Some(Viewer::Editor)
        } else {
            None
        };
        self.open_file(viewer, file, line, window, cx);
    }

    /// Open a file in a viewer — a new tab in the centre, however many there already are.
    /// `viewer` None picks by what the file is.
    pub fn open_file(
        &mut self,
        viewer: Option<crate::files::Viewer>,
        file: reactor_fs::FileRef,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::files::Viewer;
        let viewer = viewer.unwrap_or_else(|| crate::files::suggest(&file));
        let weak_app = cx.weak_entity();
        let handle = match viewer {
            Viewer::Editor => panel_handle(cx.new(|cx| {
                crate::files::editor::EditorPanel::new(weak_app, file, line, window, cx)
            })),
            Viewer::Table => panel_handle(
                cx.new(|cx| crate::files::table::TablePanel::new(weak_app, file, window, cx)),
            ),
            Viewer::Data => {
                panel_handle(cx.new(|cx| crate::files::data::DataPanel::new(weak_app, file, cx)))
            }
            Viewer::Hex => panel_handle(
                cx.new(|cx| crate::files::hex::HexPanel::new(weak_app, file, window, cx)),
            ),
        };
        self.dock_area.update(cx, |area, cx| {
            area.add_panel_view(handle, DockPlacement::Center, None, window, cx);
        });
    }

    // -- turns --------------------------------------------------------------

    /// Enter in the composer: a `/command`, a prompt when idle, a queued follow-up while
    /// a turn runs.
    ///
    /// Clears the field *before* sending: gpui-component's textarea can dispatch one
    /// `Enter` to more than one action listener, which reached this function twice for a
    /// single keystroke (the "prompt sent twice" bug). Clearing first makes it idempotent.
    pub fn send_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A completion is open: Enter takes it rather than sending half a command.
        if self.slash.is_open() {
            self.accept_slash(window, cx);
            return;
        }
        let text = self.composer.read(cx).value().to_string();
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.composer.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        self.prompt_history.remember(&text);
        if let Some(cmd) = text.strip_prefix('/') {
            if let Some(name) = cmd.split_whitespace().next() {
                self.record_use(name);
            }
            self.run_command(cmd, window, cx);
        } else if self.session.read(cx).phase != AgentPhase::Idle {
            self.session.update(cx, |s, _| s.follow_up.push(text));
            cx.notify();
        } else {
            self.start_turn(text, cx);
        }
    }

    // -- the prompt's history ---------------------------------------------------------------------

    /// Whether the cursor is on the prompt's top line (`up`) or bottom line (`!up`), with nothing
    /// selected — the only places the arrows leave the text to walk the history. A wrapped line
    /// counts as several: the cursor has to be on the first (or last) one as drawn.
    fn cursor_at_edge(&self, up: bool, cx: &App) -> bool {
        let input = self.composer.read(cx);
        if !input.selected_range().is_empty() {
            return false;
        }
        let line = input.cursor_position().line as usize;
        let last_line = input.value().matches('\n').count();
        if (up && line != 0) || (!up && line != last_line) {
            return false;
        }
        match input.cursor_layout() {
            Some((cursor, line_height)) => {
                let bounds = input.input_bounds();
                if up {
                    cursor.top() - bounds.top() < line_height
                } else {
                    bounds.bottom() - cursor.bottom() < line_height
                }
            }
            // Not laid out yet (nothing drawn): the logical line is all there is to go on.
            None => true,
        }
    }

    /// Show a message from the history in the prompt, with the cursor at its end.
    fn show_in_prompt(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::input::Position;
        let (line, character) = match text.rsplit_once('\n') {
            Some((before, last)) => (
                before.matches('\n').count() as u32 + 1,
                last.encode_utf16().count() as u32,
            ),
            None => (0, text.encode_utf16().count() as u32),
        };
        self.composer.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.set_cursor_position(Position::new(line, character), window, cx);
        });
        // A `/command` from the history is not a request for the completion popup.
        self.slash.dismissed = true;
        self.refresh_slash(false, cx);
        cx.notify();
    }

    /// Up on the top line: the message before the one shown — or, in an empty prompt, the
    /// message scheduled to run next.
    pub fn history_prev(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // An empty prompt and a message scheduled to run next: bring the latest one back to
        // change. (Nothing is lost: it leaves the schedule only to sit in the input.)
        if !self.slash.is_open() && self.composer.read(cx).value().is_empty() {
            let latest = self.session.update(cx, |s, _| s.follow_up.pop());
            if let Some(text) = latest {
                self.show_in_prompt(&text, window, cx);
                return;
            }
        }
        if self.slash.is_open() || self.prompt_history.is_empty() || !self.cursor_at_edge(true, cx)
        {
            cx.propagate();
            return;
        }
        let current = self.composer.read(cx).value().to_string();
        // Nothing older: leave it to the input, which takes the cursor to the line start.
        let Some(text) = self.prompt_history.back(&current).map(str::to_string) else {
            cx.propagate();
            return;
        };
        self.show_in_prompt(&text, window, cx);
    }

    /// Down on the bottom line: the next message, and past the newest, what was being typed.
    pub fn history_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.prompt_history.is_walking()
            || self.slash.is_open()
            || !self.cursor_at_edge(false, cx)
        {
            cx.propagate();
            return;
        }
        if let Some(text) = self.prompt_history.forward() {
            self.show_in_prompt(&text, window, cx);
        }
    }

    fn start_turn(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else {
            self.push_note("error", "the agent is not running");
            return;
        };
        self.session.update(cx, |s, _| {
            s.phase = AgentPhase::Working;
            s.last_error = None;
            s.run = Some(crate::session::Run::new(std::time::Instant::now()));
        });
        backend.prompt(text);
        // The session list shows it working.
        self.tree_panel.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    // -- the command palette and the `/` popup ----------------------------------------------------

    /// Count a run of a command, in memory and on disk, for the ranking's tie-break.
    fn record_use(&mut self, key: &str) {
        *self.usage.entry(key.to_string()).or_insert(0) += 1;
        crate::start::GuiConfig::record_command(key);
    }

    /// Every entry the palette can offer right now: the commands, and what the session's
    /// state makes possible (each model, each tool, each scenario …).
    fn palette_entries(&self) -> Vec<Entry> {
        palette::build(&self.palette_snapshot())
    }

    /// What the app has that commands take as parameters: its models, tools, scenarios,
    /// identities and reductions. The palette's generated entries and the completion of a
    /// command's arguments are both made from it.
    fn palette_snapshot(&self) -> palette::Snapshot {
        let mut snapshot = palette::Snapshot {
            models: self.models.clone(),
            working: false,
            ..Default::default()
        };
        if let Some(c) = &self.catalogue {
            snapshot.tools = c
                .tools
                .iter()
                .map(|t| (t.id.clone(), t.status == "present", t.active))
                .collect();
        }
        if let Some(t) = &self.toolsets {
            snapshot.toolsets = t
                .toolsets
                .iter()
                .map(|t| (t.id.clone(), t.active))
                .collect();
        }
        if let Ok(read) = std::fs::read_dir(&self.scenarios_dir) {
            snapshot.scenarios = read
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .collect();
            snapshot.scenarios.sort();
        }
        if let Some(b) = &self.backend {
            let settings = Settings::load(&b.paths);
            snapshot.identities = reactor_context::identity::selectable_names(
                &b.session_state().identity,
                &settings.identity,
            );
            snapshot.user_identities = settings.identity.user.names().map(str::to_string).collect();
            snapshot.reductions = b.agent.reductions().iter().map(|r| r.entry).collect();
        }
        snapshot
    }

    // -- the context window ------------------------------------------------------------------

    /// Open the context-window popup, or close it if it is open. It is gpui-kit's dialog: the
    /// library draws the backdrop, handles focus, and closes on Esc or a click outside.
    pub fn toggle_inspect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        if self.inspect.is_some() && window.has_active_dialog(cx) {
            self.close_inspect(window, cx);
            return;
        }
        let Some(backend) = self.backend.clone() else {
            self.push_note("error", "the agent is not running");
            return;
        };
        self.inspect = Some(crate::inspect_ui::InspectUi {
            data: None,
            loading: true,
            error: None,
            selected: 0,
            filter: None,
        });
        backend.inspect();
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, _cx| {
            let weak = weak.clone();
            let height = window.viewport_size().height * 0.72;
            dialog
                .w(px(1040.))
                .margin_top(px(32.))
                .close_button(false)
                .content(move |content, _window, cx| {
                    content.child(crate::inspect_ui::view(&weak, height, cx))
                })
        });
        cx.notify();
    }

    pub fn close_inspect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
        }
        if self.inspect.take().is_some() {
            self.composer
                .update(cx, |state, cx| state.focus(window, cx));
            cx.notify();
        }
    }

    /// Measure again, with the session as it is now.
    pub fn refresh_inspect(&mut self, cx: &mut Context<Self>) {
        if let (Some(i), Some(b)) = (&mut self.inspect, &self.backend) {
            i.loading = true;
            b.inspect();
            cx.notify();
        }
    }

    pub fn inspect_select(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(i) = &mut self.inspect {
            i.selected = index;
            cx.notify();
        }
    }

    /// Show only one source's pieces; choosing it again shows all.
    pub fn inspect_filter(
        &mut self,
        origin: reactor_agent::inspect::Origin,
        cx: &mut Context<Self>,
    ) {
        if let Some(i) = &mut self.inspect {
            let only = vec![origin];
            i.filter = if i.filter.as_ref() == Some(&only) {
                None
            } else {
                Some(only)
            };
            i.select_in_filter();
            cx.notify();
        }
    }

    /// Show one kind of what the request is made of in the context window, opening it if need
    /// be: a row of the usage legend was clicked.
    pub fn show_kind(
        &mut self,
        kind: reactor_agent::inspect::Kind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A click outside the popup closes it without telling us: what is left of it is no
        // popup. Starting from that, the click would filter a window that is not there, and
        // every other row of the legend would seem dead.
        if self.inspect.is_some() && !gpui_kit::component::WindowExt::has_active_dialog(window, cx)
        {
            self.inspect = None;
        }
        if self.inspect.is_none() {
            self.toggle_inspect(window, cx);
        }
        if let Some(i) = &mut self.inspect {
            i.filter = Some(kind.origins().to_vec());
            i.select_in_filter();
            cx.notify();
        }
    }

    /// The whole request, as text, to the clipboard.
    pub fn copy_inspect(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = self.inspect.as_ref().and_then(|i| i.data.as_ref()) {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(d.as_text()));
            self.push_note("info", "the request is on the clipboard");
            cx.notify();
        }
    }

    // -- settings ----------------------------------------------------------------------------------
    //
    // The settings live in their own window (`crate::settings_window`), which reads and changes
    // the app through these methods. Nothing here needs a window of its own.

    /// Open the settings window, or bring it forward if it is open.
    pub fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app = cx.entity();
        // After this update: the new window's views read the app, which is being updated now.
        window.defer(cx, move |_window, cx| crate::settings_window::open(cx, app));
    }

    /// Change the settings, then keep everything in step: normalize, save, restyle the theme.
    pub fn update_ui(
        &mut self,
        change: impl FnOnce(&mut crate::settings::UiSettings),
        cx: &mut Context<Self>,
    ) {
        change(&mut self.ui);
        self.ui.normalize();
        crate::start::GuiConfig::save_ui(&self.ui);
        crate::settings::apply(&self.ui, cx);
        cx.notify();
    }

    /// The text shown in a font family field: what is being typed, else what is set.
    pub fn family_text(&self, slot: crate::settings::Slot) -> String {
        self.family_drafts
            .get(&slot)
            .cloned()
            .unwrap_or_else(|| self.ui.family(slot).unwrap_or("").to_string())
    }

    /// A font family typed into a field: empty clears it, an installed font is applied, and
    /// anything else (half a name) is kept as typed but not applied.
    pub fn set_family_text(
        &mut self,
        slot: crate::settings::Slot,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let typed = text.trim();
        self.family_drafts.insert(slot, text.to_string());
        if typed.is_empty() {
            self.family_drafts.remove(&slot);
            self.update_ui(|ui| ui.set_family(slot, None), cx);
        } else if crate::settings::font_exists(typed, cx) {
            let typed = typed.to_string();
            self.update_ui(|ui| ui.set_family(slot, Some(typed)), cx);
        } else {
            cx.notify();
        }
    }

    /// A size from a number field. Equal to the default is the same as unset.
    pub fn set_font_size(
        &mut self,
        slot: crate::settings::Slot,
        size: f64,
        default: f64,
        cx: &mut Context<Self>,
    ) {
        let unset = (size - default).abs() < 0.5;
        self.update_ui(
            |ui| {
                ui.set_size(
                    slot,
                    if unset {
                        None
                    } else {
                        Some(size.round() as f32)
                    },
                )
            },
            cx,
        );
    }

    /// Whether this slot differs from the defaults (its reset button shows).
    pub fn font_is_set(&self, slot: crate::settings::Slot) -> bool {
        self.ui.family(slot).is_some()
            || self.ui.size(slot).is_some()
            || self.family_drafts.contains_key(&slot)
    }

    pub fn reset_font(&mut self, slot: crate::settings::Slot, cx: &mut Context<Self>) {
        self.family_drafts.remove(&slot);
        self.update_ui(
            |ui| {
                ui.set_family(slot, None);
                ui.set_size(slot, None);
            },
            cx,
        );
    }

    // -- the model -----------------------------------------------------------------------------

    /// The default model and the picker's list, as `settings.json` has them.
    pub fn model_settings(&self) -> (Option<String>, Vec<String>) {
        match &self.backend {
            Some(b) => {
                let s = Settings::load(&b.paths);
                (s.default_model, s.models)
            }
            None => (None, Vec::new()),
        }
    }

    /// Change `settings.json`'s model keys and the picker that shows them.
    fn change_model_settings(
        &mut self,
        change: impl FnOnce(&mut Settings),
        cx: &mut Context<Self>,
    ) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let mut settings = Settings::load(&backend.paths);
        change(&mut settings);
        match settings.save(&backend.paths) {
            Ok(()) => self.models = backend.models(),
            Err(e) => self.push_note("error", format!("could not save settings.json: {e}")),
        }
        cx.notify();
    }

    /// `provider/name` from the model page's fields, if a name was typed.
    fn typed_model(&mut self) -> Option<String> {
        let name = self.model_draft.name.trim().to_string();
        if name.is_empty() {
            self.push_note("warning", "type a model name first");
            return None;
        }
        Some(format!("{}/{}", self.model_draft.provider, name))
    }

    pub fn set_typed_model(
        &mut self,
        what: crate::settings_window::ModelUse,
        cx: &mut Context<Self>,
    ) {
        use crate::settings_window::ModelUse;
        let Some(spec) = self.typed_model() else {
            return;
        };
        match what {
            ModelUse::Default => self.set_default_model(Some(spec), cx),
            ModelUse::Now => self.set_model(&spec, cx),
            ModelUse::List => self.change_model_settings(
                |s| {
                    if !s.models.contains(&spec) {
                        s.models.push(spec)
                    }
                },
                cx,
            ),
        }
    }

    /// The model new sessions start on; `None` clears it. It is also put in the picker's list.
    pub fn set_default_model(&mut self, spec: Option<String>, cx: &mut Context<Self>) {
        self.change_model_settings(
            |s| {
                if let Some(spec) = &spec
                    && !s.models.contains(spec)
                {
                    s.models.push(spec.clone());
                }
                s.default_model = spec;
            },
            cx,
        );
    }

    /// Offer a model in the picker.
    pub fn list_model(&mut self, spec: &str, cx: &mut Context<Self>) {
        let spec = spec.to_string();
        self.change_model_settings(
            |s| {
                if !s.models.contains(&spec) {
                    s.models.push(spec)
                }
            },
            cx,
        );
    }

    pub fn unlist_model(&mut self, spec: &str, cx: &mut Context<Self>) {
        self.change_model_settings(
            |s| {
                s.models.retain(|m| m != spec);
                if s.default_model.as_deref() == Some(spec) {
                    s.default_model = None;
                }
            },
            cx,
        );
    }

    /// This session's model becomes the default for new sessions.
    pub fn promote_model(&mut self, cx: &mut Context<Self>) {
        match self
            .session
            .read(cx)
            .model
            .clone()
            .filter(|m| !m.is_empty())
        {
            Some(model) => {
                self.set_default_model(Some(model.clone()), cx);
                self.push_note("info", format!("{model} is now the default model"));
            }
            None => self.push_note("warning", "this session has no model yet"),
        }
    }

    // -- the context budget ----------------------------------------------------------------------

    /// One layer of the context settings as stored.
    pub fn context_layer(&self, scope: crate::settings_window::ContextScope) -> ContextSettings {
        match (&self.backend, scope) {
            (Some(b), crate::settings_window::ContextScope::Default) => b.context_layers().0,
            (Some(b), crate::settings_window::ContextScope::Session) => b.context_layers().1,
            (None, _) => ContextSettings::default(),
        }
    }

    /// Change the layer being edited: read it, let `edit` change it, store it back.
    pub fn context_edit(
        &mut self,
        edit: impl FnOnce(&mut ContextSettings),
        cx: &mut Context<Self>,
    ) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let scope = self.context_scope;
        let global = scope == crate::settings_window::ContextScope::Default;
        let before = self.context_layer(scope);
        let mut layer = before.clone();
        edit(&mut layer);
        if layer == before {
            return;
        }
        match backend.replace_context_layer(global, layer) {
            Ok(()) => self.preview = None,
            Err(e) => self.push_note("error", e),
        }
        cx.notify();
    }

    /// This session's context values become the default, and the session inherits.
    pub fn context_promote(&mut self, cx: &mut Context<Self>) {
        self.make_context_default(cx);
    }

    // -- tool activation -------------------------------------------------------------------------

    pub fn tools_promote(&mut self, cx: &mut Context<Self>) {
        self.make_activation_default(cx);
    }

    pub fn tools_inherit(&mut self, cx: &mut Context<Self>) {
        self.inherit_activation(cx);
    }

    /// Open the palette, or close it if it is already open.
    pub fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        // The dialog may have been closed by the library (Esc, a click outside) without the app
        // hearing of it, so "open" is what the window says, not what `palette` holds.
        if self.palette.is_some() && window.has_active_dialog(cx) {
            self.close_palette(window, cx);
            return;
        }
        let entries = self.palette_entries();
        let shown = palette::rank("", &entries, &self.usage, false)
            .into_iter()
            .cloned()
            .collect();
        let list = cx.new(|cx| CommandState::new(window, cx));
        self.palette = Some(PaletteUi {
            list: list.clone(),
            snapshot: self.palette_snapshot(),
            entries,
            shown,
        });

        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let weak = weak.clone();
            let closed = weak.clone();
            dialog
                .w(px(640.))
                .margin_top(px(72.))
                .close_button(false)
                // However it was closed, the prompt has the keyboard at once rather than when
                // the library's close timer fires.
                .on_close(move |_, window, cx| {
                    let _ = closed.update(cx, |app, cx| {
                        if app.palette.take().is_some() {
                            app.composer.update(cx, |state, cx| state.focus(window, cx));
                            cx.notify();
                        }
                    });
                })
                .content(move |content, _window, cx| content.child(palette_list(&weak, cx)))
        });
        // After this frame renders: a view that is not in the tree yet cannot take focus.
        window.defer(cx, move |window, cx| {
            list.update(cx, |state, cx| state.focus(window, cx))
        });
        cx.notify();
    }

    /// Close the palette's dialog and give the prompt its focus back.
    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
        }
        if self.palette.take().is_some() {
            self.composer
                .update(cx, |state, cx| state.focus(window, cx));
            cx.notify();
        }
    }

    /// The palette's query changed: rank again.
    fn palette_query(&mut self, query: &str, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.palette {
            // A command's name and a space: what it takes, to choose from. Else the commands.
            p.shown = match palette::params(query, &p.snapshot, &self.usage) {
                Some(params) => params.entries(),
                None => palette::rank(query, &p.entries, &self.usage, false)
                    .into_iter()
                    .cloned()
                    .collect(),
            };
            cx.notify();
        }
    }

    /// Choose the `row`th entry shown.
    fn palette_confirm(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self
            .palette
            .as_ref()
            .and_then(|p| p.shown.get(row).cloned())
        else {
            return;
        };
        self.close_palette(window, cx);
        self.record_use(&entry.key);
        self.run_entry(&entry, window, cx);
    }

    /// Run an entry, or — when it needs arguments — start it in the composer.
    fn run_entry(&mut self, entry: &Entry, window: &mut Window, cx: &mut Context<Self>) {
        if entry.args == Args::Required {
            self.complete_composer(entry, window, cx);
            cx.notify();
        } else {
            self.run_command(&entry.run, window, cx);
        }
    }

    /// Recompute the `/` popup from the composer's text. `reopen` lifts an Esc dismissal —
    /// true when the text changed, false when it is being set programmatically.
    fn refresh_slash(&mut self, reopen: bool, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().to_string();
        let items: Vec<Entry> = match palette::slash_query(&text) {
            Some(query) => {
                let commands = palette::commands();
                palette::rank(query, &commands, &self.usage, true)
                    .into_iter()
                    .take(SLASH_ROWS)
                    .cloned()
                    .collect()
            }
            // A command's name and a space: what it takes, to choose from.
            None if text.starts_with('/') => {
                palette::params(&text, &self.palette_snapshot(), &self.usage)
                    .map(|p| p.entries().into_iter().take(SLASH_ROWS).collect())
                    .unwrap_or_default()
            }
            None => Vec::new(),
        };
        // Not a command: a `#` reference being typed at the cursor.
        let mut reference = None;
        let items = if items.is_empty() && !text.starts_with('/') {
            let cursor = self.composer.read(cx).cursor();
            match crate::ref_index::reference_at(&text, cursor) {
                Some((trigger, query, span)) => {
                    let found = self.reference_items(trigger, &query, cx);
                    if !found.is_empty() {
                        reference = Some((trigger, span));
                    }
                    found
                }
                None => Vec::new(),
            }
        } else {
            items
        };
        self.slash.reference = reference;
        if items != self.slash.items {
            self.slash.selected = 0;
        }
        if reopen {
            self.slash.dismissed = false;
        }
        self.slash.items = items;
        cx.notify();
    }

    /// The completions of a `#` reference: files of the working directory, or the case's
    /// documents, ranked as the commands are.
    fn reference_items(
        &mut self,
        trigger: crate::ref_index::Trigger,
        query: &str,
        cx: &mut Context<Self>,
    ) -> Vec<Entry> {
        use crate::ref_index::{Trigger, plausible, size};
        let Some(backend) = self.backend.clone() else {
            return Vec::new();
        };
        let mut entries: Vec<Entry> = Vec::new();
        match trigger {
            Trigger::File => {
                // Typing `#d` also offers `#doc:`: documents are reached through it.
                if !query.is_empty() && "doc:".starts_with(&query.to_ascii_lowercase()) {
                    entries.push(Entry {
                        key: "#doc:".into(),
                        title: "doc:".into(),
                        name: None,
                        detail: "Documents imported into this case".into(),
                        group: "Reference",
                        args: Args::None,
                        run: "doc:".into(),
                    });
                }
                if let Some(cwd) = backend.paths.cwd.clone() {
                    self.file_index.refresh(&cwd);
                }
                for f in self.file_index.files.iter().filter(|f| plausible(query, &f.path)) {
                    entries.push(Entry {
                        key: format!("#{}", f.path),
                        title: f.path.clone(),
                        name: None,
                        detail: size(f.bytes),
                        group: "File",
                        args: Args::None,
                        run: f.path.clone(),
                    });
                    if entries.len() >= 600 {
                        break;
                    }
                }
            }
            Trigger::Document => {
                let docs =
                    reactor_db::corpus::documents(backend.case.db()).unwrap_or_default();
                for d in docs {
                    let label = d.title.clone().filter(|t| !t.is_empty());
                    entries.push(Entry {
                        key: format!("#doc:{}", d.source_uri),
                        // What the query is matched against: the title and the source.
                        title: format!("{} {}", label.clone().unwrap_or_default(), d.source_uri),
                        name: None,
                        detail: format!(
                            "{} passages{}",
                            d.chunks,
                            label.map(|l| format!(" · {l}")).unwrap_or_default()
                        ),
                        group: "Document",
                        args: Args::None,
                        run: format!("doc:{}", d.source_uri),
                    });
                }
            }
        }
        let _ = cx;
        palette::rank(query, &entries, &self.usage, false)
            .into_iter()
            .take(SLASH_ROWS)
            .cloned()
            .collect()
    }

    /// Put the highlighted reference in the composer in place of what was typed of it, and the
    /// cursor after it.
    fn complete_reference(&mut self, entry: &Entry, window: &mut Window, cx: &mut Context<Self>) {
        let Some((trigger, span)) = self.slash.reference.clone() else {
            return;
        };
        let text = self.composer.read(cx).value().to_string();
        let span = span.start.min(text.len())..span.end.min(text.len());
        let (inserted, keep_open) = if entry.group == "Reference" {
            // `#doc:` — the kind chosen, the name still to come.
            ("#doc:".to_string(), true)
        } else {
            let name = entry.run.strip_prefix("doc:").unwrap_or(&entry.run);
            (format!("{} ", crate::ref_index::written(trigger, name)), false)
        };
        let mut new = String::with_capacity(text.len() + inserted.len());
        new.push_str(&text[..span.start]);
        new.push_str(&inserted);
        new.push_str(&text[span.end..]);
        let at = span.start + inserted.len();
        let (line, column) = crate::ref_index::position(&new, at);
        self.composer.update(cx, |state, cx| {
            state.set_value(&new, window, cx);
            state.set_cursor_position(
                gpui_kit::component::input::Position::new(line, column),
                window,
                cx,
            );
        });
        if !keep_open {
            self.slash.items.clear();
            self.slash.reference = None;
        }
        self.refresh_slash(true, cx);
    }

    pub fn slash_move(&mut self, by: isize, cx: &mut Context<Self>) {
        let n = self.slash.items.len();
        if n > 0 {
            self.slash.selected =
                (self.slash.selected as isize + by).rem_euclid(n as isize) as usize;
            cx.notify();
        }
    }

    pub fn slash_dismiss(&mut self, cx: &mut Context<Self>) {
        self.slash.dismissed = true;
        cx.notify();
    }

    /// Take the highlighted completion: commands that need nothing more run, the others are
    /// completed in the composer with a space after them.
    pub fn accept_slash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.slash.items.get(self.slash.selected).cloned() else {
            return;
        };
        self.record_use(&entry.key);
        if self.slash.reference.is_some() {
            self.complete_reference(&entry, window, cx);
            return;
        }
        if entry.args == Args::None {
            self.composer
                .update(cx, |state, cx| state.set_value("", window, cx));
            self.refresh_slash(true, cx);
            self.run_command(&entry.run, window, cx);
        } else {
            self.complete_composer(&entry, window, cx);
        }
    }

    /// Put a chosen completion in the composer — `/name ` for a command that takes arguments,
    /// the command with its parameter for one that is being completed — with the cursor at the
    /// *beginning of what was completed*: the start of the command, or of the parameter.
    fn complete_composer(&mut self, entry: &Entry, window: &mut Window, cx: &mut Context<Self>) {
        let text = format!("/{} ", entry.run);
        // Where the completed text begins: after the slash and what was typed before the
        // parameter, in the units the textarea counts in.
        let start = if entry.group == "Parameter" {
            1 + entry.run.rfind(' ').map_or(0, |i| i + 1)
        } else {
            0
        };
        let column: usize = text.chars().take(start).map(char::len_utf16).sum();
        self.composer.update(cx, |state, cx| {
            state.set_value(&text, window, cx);
            state.set_cursor_position(
                gpui_kit::component::input::Position::new(0, column as u32),
                window,
                cx,
            );
        });
        self.refresh_slash(true, cx);
    }

    /// Put a chosen completion into the composer, from a click on the popup.
    pub fn slash_pick(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.slash.selected = index;
        self.accept_slash(window, cx);
    }

    // -- slash commands ---------------------------------------------------------------------------

    /// A `/command`, from the composer or the palette. Every user-facing feature is reachable
    /// through here (`crate::palette` lists them).
    pub fn run_command(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (name, args) = cmd
            .split_once(char::is_whitespace)
            .map(|(n, a)| (n, a.trim()))
            .unwrap_or((cmd, ""));
        let mode = || match args {
            "fade" => Mode::Fade,
            "auto" => Mode::Auto,
            _ => Mode::Compact,
        };
        match name {
            "help" => {
                let lines: Vec<String> = palette::commands()
                    .iter()
                    .map(|e| format!("/{} — {}", e.run, e.detail))
                    .collect();
                self.push_note("info", lines.join("\n"));
            }
            "palette" => self.toggle_palette(window, cx),
            "settings" => self.toggle_settings(window, cx),
            "inspect" => self.toggle_inspect(window, cx),
            "notifications" => self.toggle_notices(window, cx),
            "quit" => self.request_quit(window, cx),
            "save-case" => self.save_case(window, cx),
            "import" => self.import_documents(args, window, cx),
            "embedding" => self.embedding_command(args, cx),
            "refresh" => {
                self.refresh_catalogue(cx);
                self.refresh_services(true, cx);
                self.refresh_tree(cx);
                self.refresh_statuses(cx);
                if let Some(b) = &self.backend {
                    b.refresh_context();
                }
            }
            "interrupt" => self.interrupt(window, cx),
            // The popups of the Plan tab: `/frame` shows the frame, `/goal` and `/guidelines`
            // without a text open the editor. With a text they are the commands as before.
            "frame" => self.open_frame(window, cx),
            "goal" | "guidelines" if args.is_empty() => self.open_goal_editor(window, cx),
            "layout" => match crate::layout::LayoutPreset::ALL
                .iter()
                .find(|p| p.label().eq_ignore_ascii_case(args))
            {
                Some(p) => self.apply_layout(*p, window, cx),
                None => self.push_note("warning", "layout: default, focus, analysis or catalogue"),
            },
            "dock" => match args.to_ascii_lowercase().as_str() {
                "left" => self.toggle_dock(crate::layout::DockSide::Left, window, cx),
                "right" => self.toggle_dock(crate::layout::DockSide::Right, window, cx),
                "bottom" => self.toggle_dock(crate::layout::DockSide::Bottom, window, cx),
                _ => self.push_note("warning", "dock: left, right or bottom"),
            },
            "panel" => match crate::layout::PanelKind::from_slug(args) {
                Some(kind) => self.toggle_panel(kind, window, cx),
                None => self.push_note(
                    "warning",
                    "panel: transcript, tree, tools, toolsets, context, services, files or console",
                ),
            },
            "open" => {
                let (viewer, path) = match args.split_once(' ') {
                    Some((first, rest)) if crate::files::Viewer::from_slug(first).is_some() => {
                        (crate::files::Viewer::from_slug(first), rest)
                    }
                    _ => (None, args),
                };
                if path.trim().is_empty() {
                    self.push_note("warning", "open: which file?");
                } else {
                    let file = self.resolve_file(path);
                    self.open_file(viewer, file, None, window, cx);
                }
            }
            "look" => {
                if args.trim().is_empty() {
                    self.push_note("warning", "look: which file?");
                } else {
                    let file = self.resolve_file(args);
                    crate::files::data::open_popup(file, window, cx);
                }
            }
            "console" => {
                let initial = (!args.is_empty())
                    .then(|| ("sh".to_owned(), vec!["-c".to_owned(), args.to_owned()]));
                self.open_console(initial, window, cx);
            }
            "install" => match args.split_whitespace().next() {
                Some(id) => self.open_console(
                    Some((
                        "reactor".to_owned(),
                        vec!["install".to_owned(), id.to_owned()],
                    )),
                    window,
                    cx,
                ),
                None => self.push_note("warning", "install: which tool?"),
            },
            "tool" | "toolset" => self.run_toggle(name, args, cx),
            "activation" => match args {
                "default" => self.make_activation_default(cx),
                "inherit" => self.inherit_activation(cx),
                _ => self.push_note("warning", "activation: default or inherit"),
            },
            "context" => self.run_context(args, cx),
            "branch" => match args.trim_start_matches('#').parse::<EntryId>() {
                Ok(id) => self.switch_branch(id, cx),
                Err(_) => self.push_note(
                    "warning",
                    "branch: an entry id, as the session tree shows them",
                ),
            },
            _ if self.backend.is_none() => self.push_note("error", "the agent is not running"),
            // The identity commands that open a window: the others are the agent's side's.
            "identity"
                if matches!(
                    args.split_whitespace().next(),
                    Some("new" | "edit" | "manage" | "show")
                ) =>
            {
                self.identity_window_command(args, window, cx)
            }
            "model" => self.set_model(args, cx),
            "preview" => self.preview_reduction(mode(), cx),
            "reduce" => self.reduce_now(mode(), cx),
            "undo" => {
                let target = match args.trim_start_matches('#') {
                    "" => self
                        .backend
                        .as_ref()
                        .and_then(|b| b.agent.reductions().last().map(|r| r.entry)),
                    n => n.parse::<EntryId>().ok(),
                };
                match target {
                    Some(id) => self.restore_reduction(id, cx),
                    None => self.push_note("info", "no reduction in force"),
                }
            }
            other => self.backend_command(other, args, cx),
        }
        cx.notify();
    }

    /// A command the agent's side handles (`/identity`, `/report`, `/manifest` …): run it, show
    /// what it says, and bring what it changed up to date.
    fn backend_command(&mut self, name: &str, args: &str, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        match backend.command(name, args) {
            Ok(result) => {
                for n in result.notices {
                    self.push_note(
                        match n.level {
                            reactor_context::Level::Info => "info",
                            reactor_context::Level::Warning => "warning",
                            reactor_context::Level::Error => "error",
                        },
                        n.message,
                    );
                }
                self.refresh_statuses(cx);
                self.refresh_catalogue(cx);
                backend.refresh_context();
                // A scenario opens with its briefing as the first turn.
                if let Some(text) = result.trigger_turn {
                    self.start_turn(text, cx);
                }
            }
            Err(e) => self.push_note("error", e),
        }
    }

    // -- identities (the settings page and the editor window) ---------------------------------

    /// Every identity — the built-ins, then the saved ones in the order they are kept — for the
    /// settings page and the editor.
    pub fn identity_rows(&self) -> Vec<IdentityRow> {
        let Some(b) = &self.backend else {
            return Vec::new();
        };
        let cfg = b.identity_settings();
        let state = b.session_state().identity;
        let active = state.active_name(&cfg).map(str::to_string);
        let row = |name: &str, text: &str, builtin: bool| IdentityRow {
            name: name.to_string(),
            builtin,
            text: text.to_string(),
            default: cfg.default == name,
            active: active.as_deref() == Some(name),
        };
        let mut rows: Vec<IdentityRow> = reactor_context::identity::BUILTINS
            .iter()
            .map(|(n, t)| row(n, t, true))
            .collect();
        rows.extend(
            cfg.user
                .names()
                .map(|n| row(n, cfg.user.get(n).unwrap_or(""), false)),
        );
        rows
    }

    /// `/identity new [name]`, `/identity edit <name>` and `/identity manage`: the ones that
    /// open a window — the editor, or Settings at the identities page.
    fn identity_window_command(&mut self, args: &str, window: &mut Window, cx: &mut Context<Self>) {
        use crate::identity_editor::Start;
        let mut words = args.split_whitespace();
        let (verb, name) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
        match verb {
            "show" => self.show_identity(name, window, cx),
            "manage" => self.open_settings_at(crate::settings_window::Page::Identities, cx),
            "new" => self.open_identity_editor(Start::New(name.to_string()), cx),
            _ if name.is_empty() => {
                self.push_note("warning", "usage: /identity edit <name>");
            }
            _ => match self.identity_rows().into_iter().find(|r| r.name == name) {
                Some(r) if r.builtin => {
                    self.push_note(
                        "info",
                        format!("identity: \"{name}\" is built in: the editor opens a copy of it"),
                    );
                    self.open_identity_editor(Start::Copy(r.name), cx);
                }
                Some(r) => self.open_identity_editor(Start::Edit(r.name), cx),
                None => {
                    let names: Vec<String> =
                        self.identity_rows().into_iter().map(|r| r.name).collect();
                    self.push_note(
                        "warning",
                        format!(
                            "identity: no identity \"{name}\" — available: {}",
                            names.join(", ")
                        ),
                    );
                }
            },
        }
    }

    /// `/identity show [name]`: the identity's text in a popup (the one in use, with no name),
    /// to read and scroll — not a notification, which is for a line.
    fn show_identity(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let cfg = backend.identity_settings();
        let state = backend.session_state().identity;
        let name = match name {
            "" => state.active_name(&cfg).map(str::to_string),
            n => Some(n.to_string()),
        };
        let Some(name) = name else {
            self.push_note("info", "identity: none is in use");
            return;
        };
        let Some(text) = state
            .resolve_text(&cfg, Some(&name))
            .filter(|t| !t.trim().is_empty())
            .map(str::to_string)
        else {
            self.push_note("warning", format!("identity: no identity \"{name}\""));
            return;
        };
        // What the editor can do with it: change a saved one, copy anything else.
        let start = if cfg.user.get(&name).is_some() {
            crate::identity_editor::Start::Edit(name.clone())
        } else if name == "custom" {
            crate::identity_editor::Start::New(String::new())
        } else {
            crate::identity_editor::Start::Copy(name.clone())
        };
        let weak = cx.weak_entity();
        let title = format!("Identity: {name}");
        window.open_dialog(cx, move |dialog, window, _cx| {
            let (text, start, weak) = (text.clone(), start.clone(), weak.clone());
            let height = window.viewport_size().height * 0.6;
            dialog
                .w(px(760.))
                .title(title.clone())
                .content(move |content, _window, cx| {
                    content.child(
                        div()
                            .id("identity-show")
                            .h(height)
                            .w_full()
                            .overflow_y_scroll()
                            .child(
                                gpui_kit::base::SelectableText::new(
                                    SharedString::from("identity-show-text"),
                                    text.clone(),
                                )
                                .text_style(
                                    gpui_kit::TextStyleRefinement {
                                        color: Some(cx.theme().foreground),
                                        ..Default::default()
                                    },
                                ),
                            ),
                    )
                })
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("identity-show-edit")
                                .label("Open in the editor")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let start = start.clone();
                                    let _ = weak
                                        .update(cx, |app, cx| app.open_identity_editor(start, cx));
                                }),
                        )
                        .child(
                            Button::new("identity-show-close")
                                .label("Close")
                                .primary()
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        ),
                )
        });
        cx.notify();
    }

    /// Select an identity for this session (what `/identity <name>` does).
    pub fn identity_use(&mut self, name: &str, cx: &mut Context<Self>) {
        self.backend_command("identity", name, cx);
        cx.notify();
    }

    /// Save an identity from the editor; the reason it was refused is the `Err`.
    pub fn identity_save(
        &mut self,
        old: Option<&str>,
        name: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let backend = self.backend.clone().ok_or("the agent is not running")?;
        backend.save_identity(old, name, text)?;
        self.push_note("info", format!("identity: saved \"{}\"", name.trim()));
        self.refresh_statuses(cx);
        cx.notify();
        Ok(())
    }

    /// Remove a saved identity — after a second press, since there is no undo.
    pub fn identity_remove(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.identity_remove_armed.as_deref() != Some(name) {
            self.identity_remove_armed = Some(name.to_string());
            cx.notify();
            return;
        }
        self.identity_remove_armed = None;
        match self.backend.as_ref().map(|b| b.delete_identity(name)) {
            Some(Ok(())) => self.push_note("info", format!("identity: deleted \"{name}\"")),
            Some(Err(e)) => self.push_note("warning", e),
            None => {}
        }
        self.refresh_statuses(cx);
        cx.notify();
    }

    pub fn identity_remove_cancel(&mut self, cx: &mut Context<Self>) {
        self.identity_remove_armed = None;
        cx.notify();
    }

    /// The identity new sessions start with (`""` for none).
    pub fn set_default_identity(&mut self, name: &str, cx: &mut Context<Self>) {
        match self.backend.as_ref().map(|b| b.set_default_identity(name)) {
            Some(Ok(())) => {}
            Some(Err(e)) => self.push_note("warning", e),
            None => {}
        }
        cx.notify();
    }

    // -- MCP servers (ADR-0048) ----------------------------------------------------------

    pub fn mcp_host(&self) -> Option<Arc<reactor_agent::mcp::McpHost>> {
        self.backend.as_ref().map(|b| b.mcp.clone())
    }

    /// The servers in `mcp.json` with where each stands. An unreadable file is the `Err`: the
    /// page says so rather than showing an empty list that a save would then write.
    pub fn mcp_rows(&self) -> Result<Vec<McpServerRow>, String> {
        let paths = self.backend.as_ref().map(|b| b.paths.clone());
        let Some(paths) = paths else {
            return Ok(Vec::new());
        };
        let config = reactor_core::mcp::Config::load(&paths).map_err(|e| e.to_string())?;
        let status = self.mcp_host().map(|h| h.status()).unwrap_or_default();
        let cache = reactor_core::mcp::Cache::load(&paths);
        Ok(config
            .servers
            .into_iter()
            .map(|server| {
                let live = status.iter().find(|s| s.name == server.name);
                let cached = cache.servers.get(&server.name);
                McpServerRow {
                    standing: live.map(|l| l.standing.clone()),
                    tools: live
                        .map(|l| l.tools)
                        .filter(|n| *n > 0)
                        .or(cached.map(|c| c.tools.len()))
                        .unwrap_or(0),
                    tokens: cached
                        .map(|c| c.tools.iter().map(|t| t.tokens).sum())
                        .unwrap_or(0),
                    server,
                }
            })
            .collect())
    }

    /// Change `mcp.json` with `edit`, then let the host and the panels follow. The reason a
    /// change was refused is the `Err`.
    fn mcp_edit(
        &mut self,
        edit: impl FnOnce(&mut reactor_core::mcp::Config) -> Result<(), String>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let backend = self.backend.clone().ok_or("the agent is not running")?;
        let mut config =
            reactor_core::mcp::Config::load(&backend.paths).map_err(|e| e.to_string())?;
        edit(&mut config)?;
        config.save(&backend.paths).map_err(|e| e.to_string())?;
        backend.mcp.sync();
        self.refresh_catalogue(cx);
        cx.notify();
        Ok(())
    }

    /// Whether MCP tools are named `mcp__<server>__<tool>` (off: by their own names).
    pub fn mcp_prefix(&self) -> bool {
        self.backend
            .as_ref()
            .and_then(|b| reactor_core::mcp::Config::load(&b.paths).ok())
            .is_some_and(|c| c.prefix)
    }

    pub fn mcp_set_prefix(&mut self, on: bool, cx: &mut Context<Self>) {
        let result = self.mcp_edit(
            |config| {
                config.prefix = on;
                Ok(())
            },
            cx,
        );
        match result {
            // Every id changed: what was switched on one tool at a time is by the old name.
            Ok(()) => self.push_note(
                "info",
                "mcp: tool names changed — tools you switched on one by one need switching on again; toolsets are unaffected",
            ),
            Err(e) => self.push_note("warning", format!("mcp: {e}")),
        }
        cx.notify();
    }

    /// Add a server, or change one (`old` is the name it has now; it may be renamed).
    pub fn mcp_save(
        &mut self,
        old: Option<&str>,
        server: reactor_core::mcp::Server,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let name = server.name.clone();
        self.mcp_edit(
            |config| {
                if old != Some(name.as_str()) && config.get(&name).is_some() {
                    return Err(format!("there is already a server named `{name}`"));
                }
                if let Some(old) = old {
                    config.remove(old);
                }
                config.put(server);
                Ok(())
            },
            cx,
        )?;
        self.push_note("info", format!("mcp: saved server \"{name}\""));
        Ok(())
    }

    /// Remove a server — after a second press, since the editor is the only copy of its
    /// settings.
    pub fn mcp_remove(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.mcp_remove_armed.as_deref() != Some(name) {
            self.mcp_remove_armed = Some(name.to_string());
            cx.notify();
            return;
        }
        self.mcp_remove_armed = None;
        let result = self.mcp_edit(
            |config| {
                config.remove(name);
                Ok(())
            },
            cx,
        );
        match result {
            Ok(()) => self.push_note("info", format!("mcp: removed server \"{name}\"")),
            Err(e) => self.push_note("warning", format!("mcp: {e}")),
        }
        cx.notify();
    }

    pub fn mcp_remove_cancel(&mut self, cx: &mut Context<Self>) {
        self.mcp_remove_armed = None;
        cx.notify();
    }

    /// Switch a server on or off: a server that is off is not connected to, and lists no tools.
    pub fn mcp_set_enabled(&mut self, name: &str, enabled: bool, cx: &mut Context<Self>) {
        let result = self.mcp_edit(
            |config| {
                let mut server = config
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("no server named `{name}`"))?;
                server.enabled = enabled;
                config.put(server);
                Ok(())
            },
            cx,
        );
        if let Err(e) = result {
            self.push_note("warning", format!("mcp: {e}"));
        }
    }

    /// Connect to a server again, for one that failed or was changed outside REactor.
    pub fn mcp_reconnect(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(host) = self.mcp_host() {
            host.reconnect(name);
        }
        cx.notify();
    }

    /// Open the editor window on an MCP server: a new one, or a saved one to change.
    pub fn open_mcp_editor(&mut self, start: crate::mcp_editor::Start, cx: &mut Context<Self>) {
        let app = cx.entity();
        cx.defer(move |cx| crate::mcp_editor::open(cx, app, start));
    }

    /// Open the editor window on an identity: a new one, a saved one to change, or a built-in
    /// to read (and save a copy of). The window is made after this update: its views read the
    /// app, which is being updated now.
    pub fn open_identity_editor(
        &mut self,
        start: crate::identity_editor::Start,
        cx: &mut Context<Self>,
    ) {
        let app = cx.entity();
        cx.defer(move |cx| crate::identity_editor::open(cx, app, start));
    }

    /// Open the settings window at a page (closing and reopening one that is open, since a
    /// window opens on the page it is told to).
    pub fn open_settings_at(&mut self, page: crate::settings_window::Page, cx: &mut Context<Self>) {
        let app = cx.entity();
        let open = self.settings_window.take();
        cx.defer(move |cx| {
            if let Some(handle) = open {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
            crate::settings_window::open_at(cx, app, page);
        });
    }

    /// `/tool <id> [on|off]` and `/toolset …`; without on/off it flips what the catalogue says.
    fn run_toggle(&mut self, kind: &str, args: &str, cx: &mut Context<Self>) {
        let mut parts = args.split_whitespace();
        let Some(id) = parts.next() else {
            self.push_note(
                "warning",
                format!("{kind}: which one? /{kind} <id> [on|off]"),
            );
            return;
        };
        let current = if kind == "tool" {
            self.catalogue
                .as_ref()
                .and_then(|c| c.tools.iter().find(|t| t.id == id))
                .map(|t| t.active)
        } else {
            self.toolsets
                .as_ref()
                .and_then(|c| c.toolsets.iter().find(|t| t.id == id))
                .map(|t| t.active)
        };
        let enable = match parts.next() {
            Some("on") => true,
            Some("off") => false,
            None => !current.unwrap_or(false),
            Some(_) => {
                self.push_note("warning", format!("{kind}: on or off"));
                return;
            }
        };
        if current.is_none() && (self.catalogue.is_some() || self.toolsets.is_some()) {
            self.push_note("warning", format!("{kind}: no {kind} called \"{id}\""));
            return;
        }
        if kind == "tool" {
            self.toggle_tool(id, enable, cx);
        } else {
            self.toggle_toolset(id, enable, cx);
        }
    }

    /// `/context`: show the settings, set one for this session, or promote/drop them.
    fn run_context(&mut self, args: &str, cx: &mut Context<Self>) {
        let (key, value) = args
            .split_once(char::is_whitespace)
            .map(|(k, v)| (k, v.trim()))
            .unwrap_or((args, ""));
        let number = |v: &str| -> Option<u64> {
            let v = v.to_ascii_lowercase();
            match v.strip_suffix('k') {
                Some(n) => n.parse::<u64>().ok().map(|n| n * 1000),
                None => v.parse().ok(),
            }
        };
        let fraction = |v: &str| {
            v.trim_end_matches('%')
                .parse::<f64>()
                .ok()
                .map(|n| if n > 1.0 { n / 100.0 } else { n })
        };
        let mut layer = ContextSettings::default();
        match (key, value) {
            ("", _) => {
                let note = match &self.context {
                    Some(c) => format!(
                        "context: mode {}, window {}, reserve {}, reduce at {:.0}%, keep {:.0}%, summarizer {}",
                        c.mode,
                        c.window,
                        c.reserve,
                        c.pct * 100.0,
                        c.keep * 100.0,
                        c.summarizer.as_deref().unwrap_or("the session's model")
                    ),
                    None => "context: not measured yet".to_string(),
                };
                self.push_note("info", note);
                return;
            }
            ("default", _) => return self.make_context_default(cx),
            ("inherit", _) => return self.inherit_context(cx),
            ("mode", v) if reactor_context::settings::CONTEXT_MODES.contains(&v) => {
                layer.mode = Some(v.to_string())
            }
            ("window", v) if number(v).is_some() => layer.window = number(v),
            ("reserve", v) if number(v).is_some() => layer.reserve = number(v),
            ("pct", v) if fraction(v).is_some() => layer.pct = fraction(v),
            ("keep", v) if fraction(v).is_some() => layer.keep = fraction(v),
            ("summarizer", v) if !v.is_empty() => layer.summarizer = Some(v.to_string()),
            _ => {
                self.push_note("warning", "context: mode <auto|fade|compact> | window <tokens> | reserve <tokens> | pct <%> | keep <%> | summarizer <provider/name> | default | inherit");
                return;
            }
        }
        self.set_context(layer, cx);
    }

    /// Interrupt: cancel the turn, and give anything scheduled back to the composer.
    pub fn interrupt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        backend.cancel();
        let queued = self
            .session
            .update(cx, |s, _| std::mem::take(&mut s.follow_up));
        if !queued.is_empty() {
            let text = queued.join("\n");
            self.composer
                .update(cx, |state, cx| state.set_value(&text, window, cx));
        }
        self.push_note("info", "interrupting…");
        cx.notify();
    }

    pub fn set_model(&mut self, spec: &str, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        match backend.set_model(spec) {
            Ok(()) => {
                self.session
                    .update(cx, |s, _| s.model = Some(spec.to_string()));
                self.models = backend.models();
                backend.refresh_context();
                self.push_note("info", format!("model: {spec}"));
            }
            Err(e) => self.push_note("error", e),
        }
        cx.notify();
    }

    // -- context ---------------------------------------------------------------------------------

    pub fn preview_reduction(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if let Some(b) = &self.backend {
            self.context_busy = true;
            self.preview = None;
            b.preview(mode);
        }
        cx.notify();
    }

    pub fn reduce_now(&mut self, mode: Mode, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        if self.session.read(cx).phase != AgentPhase::Idle {
            self.push_note("warning", "wait for the agent to finish, or interrupt it");
            return;
        }
        self.context_busy = true;
        self.preview = None;
        self.session
            .update(cx, |s, _| s.phase = AgentPhase::Compacting);
        backend.reduce_now(mode);
        cx.notify();
    }

    /// Hand the agent texts from the Documents panel. Each is one entry in the log, so each can
    /// be taken out again on its own.
    pub fn give_to_agent(
        &mut self,
        attachments: Vec<reactor_agent::entry::Attachment>,
        cx: &mut Context<Self>,
    ) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let mut tokens = 0;
        let mut count = 0;
        for a in attachments {
            tokens += a.tokens;
            match backend.attach(a) {
                Ok(_) => count += 1,
                Err(e) => self.push_note("error", e),
            }
        }
        if count > 0 {
            let store = backend.store();
            self.session
                .update(cx, |s, _| s.rebuild(&store.lock().unwrap()));
            self.preview = None;
            self.push_note(
                "info",
                format!(
                    "gave the agent {count} text{} (≈ {tokens} tokens)",
                    if count == 1 { "" } else { "s" }
                ),
            );
        }
        cx.notify();
    }

    /// Take something given to the agent out of its context again.
    pub fn detach_attachment(&mut self, entry: EntryId, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        match backend.detach(entry) {
            Ok(()) => {
                let store = backend.store();
                self.session
                    .update(cx, |s, _| s.rebuild(&store.lock().unwrap()));
                self.preview = None;
            }
            Err(e) => self.push_note("error", e),
        }
        cx.notify();
    }

    pub fn restore_reduction(&mut self, entry: EntryId, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        match backend.restore(entry) {
            Ok(()) => {
                let store = backend.store();
                self.session
                    .update(cx, |s, _| s.rebuild(&store.lock().unwrap()));
                self.preview = None;
                self.push_note("info", format!("restored #{entry}"));
            }
            Err(e) => self.push_note("error", e),
        }
        cx.notify();
    }

    /// Change one field of this session's context settings.
    pub fn set_context(&mut self, layer: ContextSettings, cx: &mut Context<Self>) {
        if let Some(b) = &self.backend
            && let Err(e) = b.set_session_context(layer)
        {
            self.push_note("error", e);
        }
        self.preview = None;
        cx.notify();
    }

    /// Drop this session's overrides: everything inherits the default again.
    pub fn inherit_context(&mut self, cx: &mut Context<Self>) {
        if let Some(b) = &self.backend
            && let Err(e) = b.inherit_context()
        {
            self.push_note("error", e);
        }
        cx.notify();
    }

    /// "Make this the default": this session's settings become the global ones.
    pub fn make_context_default(&mut self, cx: &mut Context<Self>) {
        if let Some(b) = &self.backend {
            match b.make_context_default() {
                Ok(()) => self.push_note(
                    "info",
                    "these settings are now the default for new sessions",
                ),
                Err(e) => self.push_note("error", e),
            }
        }
        cx.notify();
    }

    // -- activation ------------------------------------------------------------------------------

    pub fn toggle_tool(&mut self, id: &str, enable: bool, cx: &mut Context<Self>) {
        let reactor = self.reactor.clone();
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { ReactorClient::set_tool(&reactor, &id, enable) })
                .await;
            this.update(cx, |app, cx| match result {
                Ok(_) => {
                    app.refresh_catalogue(cx);
                    app.refresh_statuses(cx);
                    if let Some(b) = &app.backend {
                        b.refresh_context();
                    }
                }
                Err(e) => app.push_note("error", e.to_string()),
            })
            .ok();
        })
        .detach();
    }

    pub fn toggle_toolset(&mut self, id: &str, enable: bool, cx: &mut Context<Self>) {
        let reactor = self.reactor.clone();
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { ReactorClient::set_toolset(&reactor, &id, enable) })
                .await;
            this.update(cx, |app, cx| match result {
                Ok(_) => {
                    app.refresh_catalogue(cx);
                    if let Some(b) = &app.backend {
                        b.refresh_context();
                    }
                }
                Err(e) => app.push_note("error", e.to_string()),
            })
            .ok();
        })
        .detach();
    }

    /// "Make this the default": copy this session's activation to the machine's.
    pub fn make_activation_default(&mut self, cx: &mut Context<Self>) {
        let Some(paths) = self.backend.as_ref().map(|b| b.paths.clone()) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    reactor_core::commands::make_session_state_default(&paths).map(|_| ())
                })
                .await;
            this.update(cx, |app, cx| {
                match result {
                    Ok(()) => app.push_note(
                        "info",
                        "this session's tools are now the default for new sessions",
                    ),
                    Err(e) => app.push_note("warning", e.to_string()),
                }
                app.refresh_catalogue(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Drop this session's activation override: it inherits the machine's again.
    pub fn inherit_activation(&mut self, cx: &mut Context<Self>) {
        let Some(paths) = self.backend.as_ref().map(|b| b.paths.clone()) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(
                    async move { reactor_core::commands::clear_session_state(&paths) },
                )
                .await;
            this.update(cx, |app, cx| {
                if let Err(e) = result {
                    app.push_note("error", e.to_string());
                }
                app.refresh_catalogue(cx);
                if let Some(b) = &app.backend {
                    b.refresh_context();
                }
            })
            .ok();
        })
        .detach();
    }

    // -- the tree --------------------------------------------------------------------------------

    /// Continue from another branch's reply. Disabled while the agent works.
    pub fn switch_branch(&mut self, entry: EntryId, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        match backend.switch_branch(entry) {
            Ok(()) => {
                let store = backend.store();
                self.session
                    .update(cx, |s, _| s.rebuild(&store.lock().unwrap()));
                self.refresh_tree(cx);
                self.refresh_statuses(cx);
                backend.refresh_context();
            }
            Err(e) => self.push_note("warning", e),
        }
        cx.notify();
    }
}

impl ReactorApp {
    /// The status bar (SPEC.md §6): what the manifest, identity, reporting and scenario
    /// are doing on the left; model and context use on the right.
    fn render_status_bar(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let session = self.session.read(cx);
        let statuses = session.statuses.clone();
        let model = session.model.clone();
        let percent = session.context_percent;

        let mut left = h_flex().gap_3().items_center();
        // Which session everything on screen that belongs to one is about.
        let active = self.active.clone();
        if !active.is_empty() {
            left = left.child(crate::hints::tip(
                div()
                    .id("active-session")
                    .text_color(cx.theme().accent)
                    .text_size(cx.theme().font_size * 0.85)
                    .child(format!("session {}", crate::session_list::short_id(&active))),
                format!(
                    "Active session {}. The tree, the context and the status follow it: it is the session whose transcript was last in focus.",
                    crate::session_list::reference(&self.case_id(), &active)
                ),
            ));
        }
        for (_, text) in &statuses {
            left = left.child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .text_size(cx.theme().font_size * 0.85)
                    .child(text.clone()),
            );
        }

        let models = self.models.clone();
        let mut right = h_flex().gap_2().items_center();
        if let Some(p) = percent {
            right = right.child(
                div()
                    .text_color(if p >= 90 {
                        cx.theme().warning
                    } else {
                        cx.theme().muted_foreground
                    })
                    .text_size(cx.theme().font_size * 0.85)
                    .child(format!("context {p}%")),
            );
        }
        if let Some(p) = self.embedding {
            right = right.child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .text_size(cx.theme().font_size * 0.85)
                    .child(format!("embedding {}/{}", p.embedded, p.total)),
            );
        }
        right = right.child(
            Button::new("model-picker")
                .ghost()
                .small()
                .label(model.unwrap_or_else(|| "no model".to_owned()))
                .disabled(models.is_empty())
                .dropdown_menu(move |mut menu, _window, _cx| {
                    for spec in &models {
                        menu = menu.menu(
                            spec.clone(),
                            Box::new(SelectModelAction { spec: spec.clone() }),
                        );
                    }
                    menu
                }),
        );

        StatusBar::new().left(left).right(right)
    }
}

/// Take a panel out of the dock knowing it only as a view: the dock removes by typed entity, so
/// this tries every kind of panel the window has.
fn remove_any_panel(
    area: &mut DockArea,
    view: gpui_kit::AnyView,
    window: &mut Window,
    cx: &mut Context<DockArea>,
) -> bool {
    macro_rules! try_kinds {
        ($($kind:ty),+ $(,)?) => {$(
            if let Ok(entity) = view.clone().downcast::<$kind>() {
                area.remove_panel(entity, window, cx);
                return true;
            }
        )+};
    }
    try_kinds!(
        TranscriptPanel,
        crate::files::editor::EditorPanel,
        crate::files::table::TablePanel,
        crate::files::data::DataPanel,
        crate::files::hex::HexPanel,
        crate::files::explorer::FilesPanel,
        crate::graph::panel::GraphPanel,
        crate::docs_panel::DocsPanel,
        TreePanel,
        ToolsPanel,
        ToolsetsPanel,
        ServicesPanel,
        ContextPanel,
        ConsolePanel,
    );
    false
}

/// The palette's contents, for its dialog: the library's command list over the entries ranked for
/// what is typed. The list does the typing and keyboard navigation; ranking is ours
/// (`crate::palette`), so its own filtering is off. The dialog builds this again on every frame,
/// so it reads the app's current state each time.
fn palette_list(weak: &gpui_kit::WeakEntity<ReactorApp>, cx: &mut App) -> gpui_kit::AnyElement {
    let Some(app) = weak.upgrade() else {
        return div().into_any_element();
    };
    let Some((list, shown)) = app
        .read(cx)
        .palette
        .as_ref()
        .map(|p| (p.list.clone(), p.shown.clone()))
    else {
        return div().into_any_element();
    };

    let items: Vec<CommandItem> = shown
        .iter()
        .map(|entry| {
            let (title, detail) = (
                entry.title.clone(),
                entry
                    .name
                    .as_ref()
                    .map(|n| format!("/{n}"))
                    .unwrap_or_else(|| entry.group.to_string()),
            );
            CommandItem::new()
                .label(title.clone())
                .child(move |_window, cx| {
                    h_flex()
                        .w_full()
                        .justify_between()
                        .gap_3()
                        .child(div().child(title.clone()))
                        .child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .text_size(cx.theme().font_size * 0.85)
                                .child(detail.clone()),
                        )
                })
        })
        .collect();

    let (on_query, on_confirm, on_cancel) = (weak.clone(), weak.clone(), weak.clone());
    let commands = PaletteList::new(&list)
        .filterable(false)
        .bordered(false)
        // The dialog's own surface is the window background; the list defaults to the popover colour.
        .bg(cx.theme().tokens.background)
        .items(items)
        .placeholder("Type a command")
        .max_h(px(380.))
        .on_query(move |query, _window, cx| {
            on_query
                .update(cx, |app, cx| app.palette_query(query, cx))
                .ok();
        })
        .on_confirm(move |index, window, cx| {
            on_confirm
                .update(cx, |app, cx| app.palette_confirm(index.row, window, cx))
                .ok();
        })
        .on_cancel(move |window, cx| {
            // The dialog closes itself on this Esc; the app only forgets the palette, after the
            // dispatch, so the window's own Esc handler still sees it open.
            let weak = on_cancel.clone();
            window.defer(cx, move |window, cx| {
                weak.update(cx, |app, cx| app.close_palette(window, cx))
                    .ok();
            });
        })
        .empty(|_state, _window, cx| {
            div()
                .p_3()
                .text_color(cx.theme().muted_foreground)
                .child("no command matches")
        });

    // `ReactorPalette`: the context `ClosePalette`'s key binding hangs on, so Esc always closes —
    // the list's own Esc only clears a typed query first.
    div()
        .key_context("ReactorPalette")
        .child(commands)
        .into_any_element()
}

// ---------------------------------------------------------------------------
// ReactorApp — the root view's render (SPEC.md §6)
// ---------------------------------------------------------------------------

impl Render for ReactorApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("reactor-workspace")
            .size_full()
            .flex()
            .flex_col()
            .relative()
            // The status bar's model picker dispatches this from a `PopupMenu`, which
            // bubbles up the dispatch tree — this is the ancestor that answers it.
            .on_action(
                cx.listener(|this, action: &SelectModelAction, _window, cx| {
                    this.set_model(&action.spec, cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &HistoryPrev, window, cx| this.history_prev(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &HistoryNext, window, cx| this.history_next(window, cx)),
            )
            .on_action(cx.listener(|this, _: &SlashUp, _window, cx| this.slash_move(-1, cx)))
            .on_action(cx.listener(|this, _: &SlashDown, _window, cx| this.slash_move(1, cx)))
            .on_action(
                cx.listener(|this, _: &SlashAccept, window, cx| this.accept_slash(window, cx)),
            )
            .on_action(cx.listener(|this, _: &SlashDismiss, _window, cx| this.slash_dismiss(cx)))
            .on_action(
                cx.listener(|this, _: &ClosePalette, window, cx| this.close_palette(window, cx)),
            )
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| this.close_tab(window, cx)))
            .on_action(
                cx.listener(|this, _: &CloseNotices, window, cx| this.close_notices(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ComposerEsc, window, cx| {
                // Esc closes the palette wherever focus is — this is the handler every Esc
                // reaches — and must not also interrupt the agent.
                if this.palette.is_some() {
                    // The library may have closed the dialog already (a click outside): then
                    // the app's note of it is stale, and this Esc is an ordinary one.
                    use gpui_kit::component::WindowExt as _;
                    if window.has_active_dialog(cx) {
                        this.close_palette(window, cx);
                        return;
                    }
                    this.palette = None;
                }
                if this.notices.is_some() {
                    this.close_notices(window, cx);
                    return;
                }
                if this.session.read(cx).phase == AgentPhase::Working {
                    this.interrupt(window, cx);
                }
            }))
            .child(
                crate::chrome::title_bar(
                    format!("REactor — {}", self.cwd.display()),
                    self.menu_bar.as_ref(),
                    Some(crate::notices_ui::bell(self, cx.weak_entity(), cx)),
                    cx,
                )
                // The title bar's own X removes the window without asking anyone, so
                // the question about an unsaved state (ADR-0045) is put here as well as
                // on the platform's close request.
                .on_close_window({
                    let weak = cx.weak_entity();
                    move |_, window, cx| {
                        let (may_close, helper_started) = weak
                            .update(cx, |app, cx| (app.should_close(window, cx), app.closing))
                            .unwrap_or((true, false));
                        if may_close {
                            window.remove_window();
                            // A helper is waiting for this process to let go of the
                            // unsaved state: do not linger behind another window.
                            if helper_started {
                                cx.quit();
                            }
                        }
                    }
                }),
            )
            .child(self.dock_area.clone())
            .child(self.render_status_bar(window, cx))
            .when_some(
                crate::notices_ui::history(self, cx.weak_entity(), cx),
                |el, popup| el.child(popup),
            )
    }
}

/// Words of a command's arguments, where `"double quotes"` hold one together: a path with a
/// space in it is one path.
fn split_words(text: &str) -> Vec<String> {
    let (mut words, mut cur, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    words.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        words.push(cur);
    }
    words
}

#[cfg(test)]
mod split_tests {
    use super::split_words;

    #[test]
    fn arguments_split_on_spaces_unless_quoted() {
        assert_eq!(split_words("  a  b "), ["a", "b"]);
        assert_eq!(
            split_words(r#"/x/my docs "/x/my docs/a b.md" https://e.org/p"#),
            ["/x/my", "docs", "/x/my docs/a b.md", "https://e.org/p"]
        );
        assert_eq!(split_words(r#""" a"#), ["", "a"]);
        assert!(split_words("   ").is_empty());
    }
}
