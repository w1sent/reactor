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
use crate::panels::{ConsolePanel, ContextPanel, ServicesPanel, ToolsPanel, ToolsetsPanel, TranscriptPanel, TreePanel};
use crate::session::{AgentPhase, ChatItem, Session, status_items};

/// How often the event pump drains the backend's channel (SPEC.md §2).
const PUMP_TICK: Duration = Duration::from_millis(50);
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
        /// Esc in the context-window popup.
        CloseInspect,
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

// ---------------------------------------------------------------------------
// ReactorApp — the root view
// ---------------------------------------------------------------------------

/// The one window's state.
pub struct ReactorApp {
    pub cwd: PathBuf,
    /// `None` only when the agent could not start; the window opens anyway and says why.
    backend: Option<Arc<Backend>>,
    reactor: Client,
    pub session: Entity<Session>,
    pub composer: Entity<TextareaState>,
    /// Notifications: the popups, and the history behind the bell.
    pub notifier: crate::notifications::Notifier,
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
    /// Which layer of the context budget the settings window edits.
    pub context_scope: crate::settings_window::ContextScope,
    /// The settings window, while it is open.
    pub settings_window: Option<gpui_kit::AnyWindowHandle>,
}

/// The open command palette.
struct PaletteUi {
    list: Entity<CommandState>,
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
    cx.bind_keys(vec![
        // `secondary` is Cmd on macOS and Ctrl elsewhere.
        KeyBinding::new("secondary-p", OpenPalette, None),
        KeyBinding::new("secondary-,", OpenSettings, None),
        KeyBinding::new("escape", CloseInspect, Some("ReactorInspect")),
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

impl ReactorApp {
    /// Open the main window: theme, dock, agent, event pump — in the order the spec's §6
    /// anatomy needs.
    pub fn open(cx: &mut App, args: &crate::start::ResolvedLaunch) -> anyhow::Result<()> {
        cx.bind_keys(vec![KeyBinding::new("escape", ComposerEsc, None)]);

        // Record the workdir as recent here: this is the one choke point every launch path
        // funnels through (a positional cwd, the chooser, the session picker).
        let mut config = crate::start::GuiConfig::load();
        config.push_workdir(&args.cwd);

        let bounds = gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds::centered(None, gpui_kit::size(px(1420.), px(920.)), cx));
        let mut app_entity: Option<Entity<ReactorApp>> = None;
        let _handle = cx.open_window(crate::chrome::window_options(bounds), |window, cx| {
            crate::theme::install_ayu_dark(cx);
            let app: Entity<ReactorApp> = cx.new(|cx| ReactorApp::new(window, cx, args));
            app_entity = Some(app.clone());
            cx.set_global(MainWindow { handle: window.window_handle(), app: app.downgrade() });
            let root_view: gpui_kit::AnyView = app.into();
            cx.new(|cx| Root::new(root_view, window, cx))
        })?;
        cx.activate(true);

        let app = app_entity.expect("window's root view was built");
        app.update(cx, |app, cx| {
            app.start_pump(cx);
            app.start_notice_clock(cx);
            app.refresh_catalogue(cx);
            app.refresh_services(false, cx);
            app.refresh_tree(cx);
            app.refresh_statuses(cx);
            if let Some(b) = &app.backend {
                b.refresh_context();
            }
        });
        Ok(())
    }

    fn new(window: &mut Window, cx: &mut Context<Self>, args: &crate::start::ResolvedLaunch) -> Self {
        let session = cx.new(|_| Session::new());
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 8)
                .submit_on_enter(true)
                .placeholder("Prompt…")
        });
        let cwd = args.cwd.clone();

        // Composer: Enter submits (queued while a turn runs).
        cx.subscribe_in(&composer, window, |this, _composer, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { shift, .. } if !*shift => this.send_composer(window, cx),
            InputEvent::Change => this.refresh_slash(true, cx),
            _ => {}
        })
        .detach();

        let weak_app = cx.weak_entity();
        let (dock_area, _skin) = DockSkin::dock_area("reactor-main", Some(1), window, cx);
        // Built once and kept as handles: a layout preset rearranges these rather than
        // building new panels, so switching never costs the console its scrollback or the
        // transcript its scroll position (crate::layout).
        let panels = crate::layout::Panels {
            transcript: panel_handle(cx.new(|cx| TranscriptPanel::new(weak_app.clone(), composer.clone(), cx))),
            tree: panel_handle(cx.new(|cx| TreePanel::new(weak_app.clone(), window, cx))),
            tools: panel_handle(cx.new(|cx| ToolsPanel::new(weak_app.clone(), window, cx))),
            toolsets: panel_handle(cx.new(|cx| ToolsetsPanel::new(weak_app.clone(), window, cx))),
            context: panel_handle(cx.new(|cx| ContextPanel::new(weak_app.clone(), window, cx))),
            services: panel_handle(cx.new(|cx| ServicesPanel::new(weak_app.clone(), window, cx))),
            console: panel_handle(cx.new(|cx| ConsolePanel::new(weak_app.clone(), Some(cwd.clone()), None, window, cx))),
        };
        let ui = crate::start::GuiConfig::load_ui();
        let layout = ui.start_layout();
        layout.apply(&panels, &dock_area, window, cx);

        // The agent. A failure to start is shown, not fatal: the window stays open.
        let opts = StartOptions {
            cwd: cwd.clone(),
            resume: args.resume_dir(&reactor_core::Paths::from_env()),
            model: args.launch_args.model.clone(),
        };
        let (backend, startup_error) = match Backend::start(opts) {
            Ok(b) => (Some(Arc::new(b)), None),
            Err(e) => (None, Some(format!("could not start the agent: {e}"))),
        };
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

        let scenarios_dir = crate::backend::find_bundled("prompts/scenarios").unwrap_or_else(|| PathBuf::from("prompts/scenarios"));
        let models = backend.as_ref().map(|b| b.models()).unwrap_or_default();
        let tree = backend.as_ref().map(|b| b.tree()).unwrap_or_default();
        let leaf_id = backend.as_ref().map(|b| b.store().lock().unwrap().head());
        // A resumed session brings its earlier messages with it.
        let prompt_history = backend
            .as_ref()
            .map(|b| b.store().lock().unwrap().branch().into_iter().filter_map(|e| if let reactor_agent::entry::Kind::User { text } = &e.kind { Some(text.clone()) } else { None }).collect::<Vec<String>>())
            .map(crate::prompt_history::PromptHistory::from_messages)
            .unwrap_or_default();

        Self {
            menu_bar: crate::chrome::menu_bar(cx),
            cwd,
            backend,
            reactor,
            session,
            composer,
            notifier: crate::notifications::Notifier::default(),
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
            prompt_history,
            ui,
            inspect: None,
            family_drafts: Default::default(),
            model_draft: Default::default(),
            context_scope: crate::settings_window::ContextScope::Default,
            settings_window: None,
        }
    }

    /// The event pump (SPEC.md §2): drain the backend's channel every tick and ingest.
    /// A poll loop, not a blocking recv — the pump future runs on gpui's foreground
    /// executor and only ever sleeps on a background timer.
    fn start_pump(&mut self, cx: &mut Context<Self>) {
        let Some(receiver) = self.backend.as_ref().and_then(|b| b.take_events()) else {
            return;
        };
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
                            app.ingest(e, cx);
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
        let Some(backend) = self.backend.clone() else { return };
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
                self.session.update(cx, |s, _| s.apply(&e, &store.lock().unwrap()));
                if matches!(e, Event::Appended(_)) {
                    self.refresh_tree(cx);
                    self.refresh_statuses(cx);
                }
            }
            UiEvent::TurnEnded(result) => {
                backend.turn_finished();
                let store = backend.store();
                self.session.update(cx, |s, _| s.end_turn(&store.lock().unwrap()));
                let ok = result.is_ok();
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
                // Messages typed while it ran go next, oldest first.
                if ok && let Some(next) = self.session.update(cx, |s, _| (!s.follow_up.is_empty()).then(|| s.follow_up.remove(0))) {
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
                    Ok(id) => self.push_note("info", format!("reduced (#{id}) — undo it from the Context panel")),
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
                self.session.update(cx, |s, _| s.context_percent = Some(view.percent()));
                self.context = Some(view);
            }
        }
        cx.notify();
    }

    /// Tell the person something: a popup in the corner for `ui.notice_seconds`, and a line in
    /// the history behind the bell. `kind` is `info`, `warning` or `error`.
    pub fn push_note(&mut self, kind: impl Into<SharedString>, message: impl Into<SharedString>) {
        let level = crate::notifications::Level::from_kind(&kind.into());
        let now = chrono::Local::now();
        let (date, time) = (now.format("%Y-%m-%d").to_string(), now.format("%H:%M:%S").to_string());
        let timeout = Duration::from_secs(u64::from(self.ui.notice_seconds));
        self.notifier.push(level, message.into().to_string(), date, time, std::time::Instant::now(), timeout);
    }

    /// The popups expire on a clock of their own, so they go even when nothing else happens.
    fn start_notice_clock(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let alive = this
                    .update(cx, |app, cx| {
                        if app.notifier.expire(std::time::Instant::now()) {
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn dismiss_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        self.notifier.dismiss(id);
        cx.notify();
    }

    /// Open the notification history, or close it if it is open.
    pub fn toggle_notices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.notices.is_some() {
            self.close_notices(window, cx);
            return;
        }
        let focus = cx.focus_handle();
        self.notices = Some(crate::notices_ui::NoticesUi { focus: focus.clone() });
        self.notifier.mark_read();
        window.defer(cx, move |window, cx| window.focus(&focus, cx));
        cx.notify();
    }

    pub fn close_notices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.notices.take().is_some() {
            self.composer.update(cx, |state, cx| state.focus(window, cx));
            cx.notify();
        }
    }

    pub fn clear_notices(&mut self, cx: &mut Context<Self>) {
        self.notifier.clear();
        cx.notify();
    }

    // -- derived data ------------------------------------------------------------------------

    pub fn refresh_tree(&mut self, _cx: &mut Context<Self>) {
        if let Some(b) = &self.backend {
            self.tree = b.tree();
            self.leaf_id = Some(b.store().lock().unwrap().head());
        }
    }

    /// The status bar's left side, from the session's state modules.
    pub fn refresh_statuses(&mut self, cx: &mut Context<Self>) {
        let Some(b) = self.backend.clone() else { return };
        let (state, settings) = (b.session_state(), Settings::load(&b.paths));
        let items = status_items(&state, &settings, &self.scenarios_dir);
        self.session.update(cx, |s, _| s.statuses = items);
    }

    pub fn refresh_catalogue(&mut self, cx: &mut Context<Self>) {
        let reactor = self.reactor.clone();
        self.catalogue_loading = true;
        cx.spawn(async move |this, cx| {
            let (tools, toolsets, state) = cx
                .background_spawn(async move { (ReactorClient::tools(&reactor), ReactorClient::toolsets(&reactor), ReactorClient::state(&reactor)) })
                .await;
            this.update(cx, |app, cx| {
                // A failure is said, not shown as an empty catalogue.
                for failure in [tools.as_ref().err(), toolsets.as_ref().err()].into_iter().flatten().take(1) {
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
            let services = cx.background_spawn(async move { ReactorClient::services(&reactor, refresh) }).await;
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
    pub fn apply_layout(&mut self, preset: crate::layout::LayoutPreset, window: &mut Window, cx: &mut Context<Self>) {
        preset.apply(&self.panels, &self.dock_area, window, cx);
        self.layout = preset;
        cx.notify();
    }

    /// Show or hide one dock, from the Layout menu.
    pub fn toggle_dock(&mut self, side: crate::layout::DockSide, window: &mut Window, cx: &mut Context<Self>) {
        crate::layout::toggle_dock(side, &self.dock_area, window, cx);
        cx.notify();
    }

    // -- the console --------------------------------------------------------

    /// Open a new terminal: another `ConsolePanel`, tabbed alongside any that already
    /// exist, in the bottom dock (created if need be). `initial`, when given, is a command
    /// run immediately — how installing a catalogued tool opens its own terminal already
    /// running `reactor install <id>`.
    pub fn open_console(&mut self, initial: Option<(String, Vec<String>)>, window: &mut Window, cx: &mut Context<Self>) {
        let weak_app = cx.weak_entity();
        let cwd = Some(self.cwd.clone());
        let panel = cx.new(|cx| crate::panels::ConsolePanel::new(weak_app, cwd, initial, window, cx));
        let handle = panel_handle(panel);
        self.dock_area.update(cx, |area, cx| {
            area.add_panel_view(handle, DockPlacement::Bottom, None, window, cx);
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
            Some((before, last)) => (before.matches('\n').count() as u32 + 1, last.encode_utf16().count() as u32),
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

    /// Up on the top line: the message before the one shown.
    pub fn history_prev(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.slash.is_open() || self.prompt_history.is_empty() || !self.cursor_at_edge(true, cx) {
            cx.propagate();
            return;
        }
        let current = self.composer.read(cx).value().to_string();
        // Nothing older: leave it to the input, which takes the cursor to the line start.
        let Some(text) = self.prompt_history.prev(&current).map(str::to_string) else {
            cx.propagate();
            return;
        };
        self.show_in_prompt(&text, window, cx);
    }

    /// Down on the bottom line: the next message, and past the newest, what was being typed.
    pub fn history_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.prompt_history.is_walking() || self.slash.is_open() || !self.cursor_at_edge(false, cx) {
            cx.propagate();
            return;
        }
        if let Some(text) = self.prompt_history.next() {
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
        });
        backend.prompt(text);
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
        let mut snapshot = palette::Snapshot { models: self.models.clone(), working: false, ..Default::default() };
        if let Some(c) = &self.catalogue {
            snapshot.tools = c.tools.iter().map(|t| (t.id.clone(), t.status == "present", t.active)).collect();
        }
        if let Some(t) = &self.toolsets {
            snapshot.toolsets = t.toolsets.iter().map(|t| (t.id.clone(), t.active)).collect();
        }
        if let Ok(read) = std::fs::read_dir(&self.scenarios_dir) {
            snapshot.scenarios = read.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().to_str().map(str::to_string)).collect();
            snapshot.scenarios.sort();
        }
        if let Some(b) = &self.backend {
            let settings = Settings::load(&b.paths);
            snapshot.identities = reactor_context::identity::selectable_names(&b.session_state().identity, &settings.identity);
            snapshot.reductions = b.agent.reductions().iter().map(|r| r.entry).collect();
        }
        palette::build(&snapshot)
    }

    // -- the context window ------------------------------------------------------------------

    /// Open the context-window popup, or close it if it is open.
    pub fn toggle_inspect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inspect.is_some() {
            self.close_inspect(window, cx);
            return;
        }
        let Some(backend) = self.backend.clone() else {
            self.push_note("error", "the agent is not running");
            return;
        };
        self.palette = None;
        let focus = cx.focus_handle();
        self.inspect = Some(crate::inspect_ui::InspectUi { data: None, loading: true, error: None, selected: 0, filter: None, focus: focus.clone() });
        backend.inspect();
        // After this frame renders: a view that is not in the tree yet cannot take focus.
        window.defer(cx, move |window, cx| window.focus(&focus, cx));
        cx.notify();
    }

    pub fn close_inspect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inspect.take().is_some() {
            self.composer.update(cx, |state, cx| state.focus(window, cx));
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
    pub fn inspect_filter(&mut self, origin: reactor_agent::inspect::Origin, cx: &mut Context<Self>) {
        if let Some(i) = &mut self.inspect {
            i.filter = if i.filter == Some(origin) { None } else { Some(origin) };
            if let Some(d) = &i.data
                && i.filter.is_some_and(|f| d.segments.get(i.selected).is_none_or(|s| s.origin != f))
                && let Some(first) = d.segments.iter().position(|s| Some(s.origin) == i.filter)
            {
                i.selected = first;
            }
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
    pub fn update_ui(&mut self, change: impl FnOnce(&mut crate::settings::UiSettings), cx: &mut Context<Self>) {
        change(&mut self.ui);
        self.ui.normalize();
        crate::start::GuiConfig::save_ui(&self.ui);
        crate::settings::apply(&self.ui, cx);
        cx.notify();
    }

    /// The text shown in a font family field: what is being typed, else what is set.
    pub fn family_text(&self, slot: crate::settings::Slot) -> String {
        self.family_drafts.get(&slot).cloned().unwrap_or_else(|| self.ui.family(slot).unwrap_or("").to_string())
    }

    /// A font family typed into a field: empty clears it, an installed font is applied, and
    /// anything else (half a name) is kept as typed but not applied.
    pub fn set_family_text(&mut self, slot: crate::settings::Slot, text: &str, cx: &mut Context<Self>) {
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
    pub fn set_font_size(&mut self, slot: crate::settings::Slot, size: f64, default: f64, cx: &mut Context<Self>) {
        let unset = (size - default).abs() < 0.5;
        self.update_ui(|ui| ui.set_size(slot, if unset { None } else { Some(size.round() as f32) }), cx);
    }

    /// Whether this slot differs from the defaults (its reset button shows).
    pub fn font_is_set(&self, slot: crate::settings::Slot) -> bool {
        self.ui.family(slot).is_some() || self.ui.size(slot).is_some() || self.family_drafts.contains_key(&slot)
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
    fn change_model_settings(&mut self, change: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else { return };
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

    pub fn set_typed_model(&mut self, what: crate::settings_window::ModelUse, cx: &mut Context<Self>) {
        use crate::settings_window::ModelUse;
        let Some(spec) = self.typed_model() else { return };
        match what {
            ModelUse::Default => self.set_default_model(Some(spec), cx),
            ModelUse::Now => self.set_model(&spec, cx),
            ModelUse::List => self.change_model_settings(|s| if !s.models.contains(&spec) { s.models.push(spec) }, cx),
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
        match self.session.read(cx).model.clone().filter(|m| !m.is_empty()) {
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
    pub fn context_edit(&mut self, edit: impl FnOnce(&mut ContextSettings), cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else { return };
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
        if self.palette.is_some() {
            self.close_palette(window, cx);
            return;
        }
        let entries = self.palette_entries();
        let shown = palette::rank("", &entries, &self.usage, false).into_iter().cloned().collect();
        let list = cx.new(|cx| CommandState::new(window, cx));
        self.palette = Some(PaletteUi { list: list.clone(), entries, shown });
        // After this frame renders: a view that is not in the tree yet cannot take focus.
        window.defer(cx, move |window, cx| list.update(cx, |state, cx| state.focus(window, cx)));
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self.composer.update(cx, |state, cx| state.focus(window, cx));
            cx.notify();
        }
    }

    /// The palette's query changed: rank again.
    fn palette_query(&mut self, query: &str, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.palette {
            p.shown = palette::rank(query, &p.entries, &self.usage, false).into_iter().cloned().collect();
            cx.notify();
        }
    }

    /// Choose the `row`th entry shown.
    fn palette_confirm(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.palette.as_ref().and_then(|p| p.shown.get(row).cloned()) else { return };
        self.close_palette(window, cx);
        self.record_use(&entry.key);
        self.run_entry(&entry, window, cx);
    }

    /// Run an entry, or — when it needs arguments — start it in the composer.
    fn run_entry(&mut self, entry: &Entry, window: &mut Window, cx: &mut Context<Self>) {
        if entry.args == Args::Required {
            let text = format!("/{} ", entry.run);
            self.composer.update(cx, |state, cx| {
                state.set_value(&text, window, cx);
                state.focus(window, cx);
            });
            self.refresh_slash(false, cx);
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
                palette::rank(query, &commands, &self.usage, true).into_iter().take(SLASH_ROWS).cloned().collect()
            }
            None => Vec::new(),
        };
        if items != self.slash.items {
            self.slash.selected = 0;
        }
        if reopen {
            self.slash.dismissed = false;
        }
        self.slash.items = items;
        cx.notify();
    }

    pub fn slash_move(&mut self, by: isize, cx: &mut Context<Self>) {
        let n = self.slash.items.len();
        if n > 0 {
            self.slash.selected = (self.slash.selected as isize + by).rem_euclid(n as isize) as usize;
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
        let Some(entry) = self.slash.items.get(self.slash.selected).cloned() else { return };
        self.record_use(&entry.key);
        if entry.args == Args::None {
            self.composer.update(cx, |state, cx| state.set_value("", window, cx));
            self.refresh_slash(true, cx);
            self.run_command(&entry.run, window, cx);
        } else {
            let text = format!("/{} ", entry.run);
            self.composer.update(cx, |state, cx| {
                state.set_value(&text, window, cx);
                state.focus(window, cx);
            });
            self.refresh_slash(true, cx);
        }
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
        let (name, args) = cmd.split_once(char::is_whitespace).map(|(n, a)| (n, a.trim())).unwrap_or((cmd, ""));
        let mode = || match args {
            "fade" => Mode::Fade,
            "auto" => Mode::Auto,
            _ => Mode::Compact,
        };
        match name {
            "help" => {
                let lines: Vec<String> = palette::commands().iter().map(|e| format!("/{} — {}", e.run, e.detail)).collect();
                self.push_note("info", lines.join("\n"));
            }
            "palette" => self.toggle_palette(window, cx),
            "settings" => self.toggle_settings(window, cx),
            "inspect" => self.toggle_inspect(window, cx),
            "notifications" => self.toggle_notices(window, cx),
            "quit" => cx.quit(),
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
            "layout" => match crate::layout::LayoutPreset::ALL.iter().find(|p| p.label().eq_ignore_ascii_case(args)) {
                Some(p) => self.apply_layout(*p, window, cx),
                None => self.push_note("warning", "layout: default, focus, analysis or catalogue"),
            },
            "dock" => match args.to_ascii_lowercase().as_str() {
                "left" => self.toggle_dock(crate::layout::DockSide::Left, window, cx),
                "right" => self.toggle_dock(crate::layout::DockSide::Right, window, cx),
                "bottom" => self.toggle_dock(crate::layout::DockSide::Bottom, window, cx),
                _ => self.push_note("warning", "dock: left, right or bottom"),
            },
            "console" => {
                let initial = (!args.is_empty()).then(|| ("sh".to_owned(), vec!["-c".to_owned(), args.to_owned()]));
                self.open_console(initial, window, cx);
            }
            "install" => match args.split_whitespace().next() {
                Some(id) => self.open_console(Some(("reactor".to_owned(), vec!["install".to_owned(), id.to_owned()])), window, cx),
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
                Err(_) => self.push_note("warning", "branch: an entry id, as the session tree shows them"),
            },
            _ if self.backend.is_none() => self.push_note("error", "the agent is not running"),
            "model" => self.set_model(args, cx),
            "preview" => self.preview_reduction(mode(), cx),
            "reduce" => self.reduce_now(mode(), cx),
            "undo" => {
                let target = match args.trim_start_matches('#') {
                    "" => self.backend.as_ref().and_then(|b| b.agent.reductions().last().map(|r| r.entry)),
                    n => n.parse::<EntryId>().ok(),
                };
                match target {
                    Some(id) => self.restore_reduction(id, cx),
                    None => self.push_note("info", "no reduction in force"),
                }
            }
            other => {
                let Some(backend) = self.backend.clone() else { return };
                match backend.command(other, args) {
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
        }
        cx.notify();
    }

    /// `/tool <id> [on|off]` and `/toolset …`; without on/off it flips what the catalogue says.
    fn run_toggle(&mut self, kind: &str, args: &str, cx: &mut Context<Self>) {
        let mut parts = args.split_whitespace();
        let Some(id) = parts.next() else {
            self.push_note("warning", format!("{kind}: which one? /{kind} <id> [on|off]"));
            return;
        };
        let current = if kind == "tool" {
            self.catalogue.as_ref().and_then(|c| c.tools.iter().find(|t| t.id == id)).map(|t| t.active)
        } else {
            self.toolsets.as_ref().and_then(|c| c.toolsets.iter().find(|t| t.id == id)).map(|t| t.active)
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
        let (key, value) = args.split_once(char::is_whitespace).map(|(k, v)| (k, v.trim())).unwrap_or((args, ""));
        let number = |v: &str| -> Option<u64> {
            let v = v.to_ascii_lowercase();
            match v.strip_suffix('k') {
                Some(n) => n.parse::<u64>().ok().map(|n| n * 1000),
                None => v.parse().ok(),
            }
        };
        let fraction = |v: &str| v.trim_end_matches('%').parse::<f64>().ok().map(|n| if n > 1.0 { n / 100.0 } else { n });
        let mut layer = ContextSettings::default();
        match (key, value) {
            ("", _) => {
                let note = match &self.context {
                    Some(c) => format!(
                        "context: mode {}, window {}, reserve {}, reduce at {:.0}%, keep {:.0}%, summarizer {}",
                        c.mode, c.window, c.reserve, c.pct * 100.0, c.keep * 100.0, c.summarizer.as_deref().unwrap_or("the session's model")
                    ),
                    None => "context: not measured yet".to_string(),
                };
                self.push_note("info", note);
                return;
            }
            ("default", _) => return self.make_context_default(cx),
            ("inherit", _) => return self.inherit_context(cx),
            ("mode", v) if reactor_context::settings::CONTEXT_MODES.contains(&v) => layer.mode = Some(v.to_string()),
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

    /// Interrupt: cancel the turn, and give anything queued back to the composer.
    pub fn interrupt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else { return };
        backend.cancel();
        let queued = self.session.update(cx, |s, _| std::mem::take(&mut s.follow_up));
        if !queued.is_empty() {
            let text = queued.join("\n");
            self.composer.update(cx, |state, cx| state.set_value(&text, window, cx));
        }
        self.push_note("info", "interrupting…");
        cx.notify();
    }

    pub fn set_model(&mut self, spec: &str, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else { return };
        match backend.set_model(spec) {
            Ok(()) => {
                self.session.update(cx, |s, _| s.model = Some(spec.to_string()));
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
        let Some(backend) = self.backend.clone() else { return };
        if self.session.read(cx).phase != AgentPhase::Idle {
            self.push_note("warning", "wait for the agent to finish, or interrupt it");
            return;
        }
        self.context_busy = true;
        self.preview = None;
        self.session.update(cx, |s, _| s.phase = AgentPhase::Compacting);
        backend.reduce_now(mode);
        cx.notify();
    }

    pub fn restore_reduction(&mut self, entry: EntryId, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else { return };
        match backend.restore(entry) {
            Ok(()) => {
                let store = backend.store();
                self.session.update(cx, |s, _| s.rebuild(&store.lock().unwrap()));
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
                Ok(()) => self.push_note("info", "these settings are now the default for new sessions"),
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
            let result = cx.background_spawn(async move { ReactorClient::set_tool(&reactor, &id, enable) }).await;
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
            let result = cx.background_spawn(async move { ReactorClient::set_toolset(&reactor, &id, enable) }).await;
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
        let Some(paths) = self.backend.as_ref().map(|b| b.paths.clone()) else { return };
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { reactor_core::commands::make_session_state_default(&paths).map(|_| ()) }).await;
            this.update(cx, |app, cx| {
                match result {
                    Ok(()) => app.push_note("info", "this session's tools are now the default for new sessions"),
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
        let Some(paths) = self.backend.as_ref().map(|b| b.paths.clone()) else { return };
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { reactor_core::commands::clear_session_state(&paths) }).await;
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
        let Some(backend) = self.backend.clone() else { return };
        match backend.switch_branch(entry) {
            Ok(()) => {
                let store = backend.store();
                self.session.update(cx, |s, _| s.rebuild(&store.lock().unwrap()));
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
    fn render_status_bar(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.session.read(cx);
        let statuses = session.statuses.clone();
        let model = session.model.clone();
        let percent = session.context_percent;

        let mut left = h_flex().gap_3().items_center();
        for (_, text) in &statuses {
            left = left.child(div().text_color(cx.theme().muted_foreground).text_size(cx.theme().font_size * 0.85).child(text.clone()));
        }

        let models = self.models.clone();
        let mut right = h_flex().gap_2().items_center();
        if let Some(p) = percent {
            right = right.child(
                div()
                    .text_color(if p >= 90 { cx.theme().warning } else { cx.theme().muted_foreground })
                    .text_size(cx.theme().font_size * 0.85)
                    .child(format!("context {p}%")),
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
                        menu = menu.menu(spec.clone(), Box::new(SelectModelAction { spec: spec.clone() }));
                    }
                    menu
                }),
        );

        StatusBar::new().left(left).right(right)
    }
}

impl ReactorApp {
    /// The command palette: a scrim over the window and the list in a card near the top.
    /// The list does the typing and keyboard navigation; ranking is ours (`crate::palette`),
    /// so its own filtering is off.
    fn render_palette(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let palette = self.palette.as_ref()?;
        let list = palette.list.clone();
        let weak = cx.weak_entity();
        let theme = cx.theme().clone();

        let items: Vec<CommandItem> = palette
            .shown
            .iter()
            .map(|entry| {
                let (title, detail) = (entry.title.clone(), entry.name.as_ref().map(|n| format!("/{n}")).unwrap_or_else(|| entry.group.to_string()));
                CommandItem::new().label(title.clone()).child(move |_window, cx| {
                    h_flex()
                        .w_full()
                        .justify_between()
                        .gap_3()
                        .child(div().child(title.clone()))
                        .child(div().text_color(cx.theme().muted_foreground).text_size(cx.theme().font_size * 0.85).child(detail.clone()))
                })
            })
            .collect();

        let (on_query, on_confirm, on_cancel) = (weak.clone(), weak.clone(), weak);
        let commands = PaletteList::new(&list)
            .filterable(false)
            .items(items)
            .placeholder("Type a command")
            .max_h(px(380.))
            .on_query(move |query, _window, cx| {
                on_query.update(cx, |app, cx| app.palette_query(query, cx)).ok();
            })
            .on_confirm(move |index, window, cx| {
                on_confirm.update(cx, |app, cx| app.palette_confirm(index.row, window, cx)).ok();
            })
            .on_cancel(move |window, cx| {
                // Deferred: this runs mid-dispatch, and Esc goes on to the window's own
                // handler, which must still see the palette open (see `ComposerEsc`).
                let weak = on_cancel.clone();
                window.defer(cx, move |window, cx| {
                    weak.update(cx, |app, cx| app.close_palette(window, cx)).ok();
                });
            })
            .empty(|_state, _window, cx| div().p_3().text_color(cx.theme().muted_foreground).child("no command matches"));

        Some(
            div()
                .id("palette-scrim")
                .key_context("ReactorPalette")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .bg(theme.background.opacity(0.6))
                .flex()
                .justify_center()
                .items_start()
                .pt(px(72.))
                .on_mouse_down(MouseButton::Left, cx.listener(move |app, _, window, cx| app.close_palette(window, cx)))
                .child(div().w(px(640.)).max_w_full().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(commands)),
        )
    }
}

// ---------------------------------------------------------------------------
// ReactorApp — the root view's render (SPEC.md §6)
// ---------------------------------------------------------------------------

impl Render for ReactorApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Root's overlay layers: notifications, dialogs (§6).
        let notification_layer = Root::render_notification_layer(window, cx);
        let dialog_layer = Root::render_dialog_layer(window, cx);

        div()
            .id("reactor-workspace")
            .size_full()
            .flex()
            .flex_col()
            .relative()
            // The status bar's model picker dispatches this from a `PopupMenu`, which
            // bubbles up the dispatch tree — this is the ancestor that answers it.
            .on_action(cx.listener(|this, action: &SelectModelAction, _window, cx| {
                this.set_model(&action.spec, cx);
            }))
            .on_action(cx.listener(|this, _: &HistoryPrev, window, cx| this.history_prev(window, cx)))
            .on_action(cx.listener(|this, _: &HistoryNext, window, cx| this.history_next(window, cx)))
            .on_action(cx.listener(|this, _: &SlashUp, _window, cx| this.slash_move(-1, cx)))
            .on_action(cx.listener(|this, _: &SlashDown, _window, cx| this.slash_move(1, cx)))
            .on_action(cx.listener(|this, _: &SlashAccept, window, cx| this.accept_slash(window, cx)))
            .on_action(cx.listener(|this, _: &SlashDismiss, _window, cx| this.slash_dismiss(cx)))
            .on_action(cx.listener(|this, _: &ClosePalette, window, cx| this.close_palette(window, cx)))
            .on_action(cx.listener(|this, _: &CloseInspect, window, cx| this.close_inspect(window, cx)))
            .on_action(cx.listener(|this, _: &CloseNotices, window, cx| this.close_notices(window, cx)))
            .on_action(cx.listener(|this, _: &ComposerEsc, window, cx| {
                // Esc closes the palette wherever focus is — this is the handler every Esc
                // reaches — and must not also interrupt the agent.
                if this.palette.is_some() {
                    this.close_palette(window, cx);
                    return;
                }
                if this.inspect.is_some() {
                    this.close_inspect(window, cx);
                    return;
                }
                if this.notices.is_some() {
                    this.close_notices(window, cx);
                    return;
                }
                if this.session.read(cx).phase == AgentPhase::Working {
                    this.interrupt(window, cx);
                }
            }))
            .child(crate::chrome::title_bar(
                format!("REactor — {}", self.cwd.display()),
                self.menu_bar.as_ref(),
                Some(crate::notices_ui::bell(self, cx.weak_entity(), cx)),
                cx,
            ))
            .child(self.dock_area.clone())
            .child(self.render_status_bar(window, cx))
            .when_some(self.render_palette(cx), |el, palette| el.child(palette))
            .when_some(crate::inspect_ui::overlay(self, cx.weak_entity(), cx), |el, popup| el.child(popup))
            .when_some(crate::notices_ui::history(self, cx.weak_entity(), cx), |el, popup| el.child(popup))
            .when_some(crate::notices_ui::toasts(self, cx.weak_entity(), cx), |el, popup| el.child(popup))
            .when_some(notification_layer, |el, layer| el.child(layer))
            .when_some(dialog_layer, |el, layer| el.child(layer))
    }
}
