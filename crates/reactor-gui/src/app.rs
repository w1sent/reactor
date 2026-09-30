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
use crate::panels::{ConsolePanel, ContextPanel, ServicesPanel, ToolsPanel, ToolsetsPanel, TranscriptPanel, TreePanel};
use crate::session::{AgentPhase, ChatItem, Session, status_items};

/// How often the event pump drains the backend's channel (SPEC.md §2).
const PUMP_TICK: Duration = Duration::from_millis(50);
/// How much of a tool card's output the transcript renders before it sheds the tail
/// (the whole of it is in the session and `history_read` reaches it).
pub const TOOL_OUTPUT_MAX_CHARS: usize = 2000;

/// The commands the composer understands, for its hint and the `/help` line.
pub const COMMANDS: &[(&str, &str)] = &[
    ("goal", "set the session goal (/goal clear)"),
    ("guidelines", "set session guidelines (/guidelines clear)"),
    ("manifest", "on | off | clear the manifest"),
    ("frame", "show the manifest"),
    ("identity", "select or write the working persona"),
    ("report", "reporting on | off | level <0-2> | status"),
    ("reactor-scenario", "list | start <id> | status | next | stop"),
    ("model", "switch model: /model provider/name"),
    ("preview", "what a context reduction would do  [auto|fade|compact]"),
    ("reduce", "reduce the context now  [auto|fade|compact]"),
    ("undo", "undo the latest reduction"),
    ("help", "this list"),
];

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
    /// Transient toasts, most recent last.
    pub notifications: Vec<(String, String)>,
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
}

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
                .placeholder("Prompt — Enter sends (queues while the agent works), Shift+Enter newline, /help for commands")
        });
        let cwd = args.cwd.clone();

        // Composer: Enter submits (queued while a turn runs).
        cx.subscribe_in(&composer, window, |this, _composer, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift, .. } = event
                && !*shift
            {
                this.send_composer(window, cx);
            }
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
        let layout = crate::layout::LayoutPreset::Default;
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

        Self {
            menu_bar: crate::chrome::menu_bar(cx),
            cwd,
            backend,
            reactor,
            session,
            composer,
            notifications: Vec::new(),
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

    pub fn push_note(&mut self, kind: impl Into<SharedString>, message: impl Into<SharedString>) {
        self.notifications.push((kind.into().to_string(), message.into().to_string()));
        if self.notifications.len() > 3 {
            self.notifications.remove(0);
        }
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
        let text = self.composer.read(cx).value().to_string();
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.composer.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        if let Some(cmd) = text.strip_prefix('/') {
            self.run_command(cmd, cx);
        } else if self.session.read(cx).phase != AgentPhase::Idle {
            self.session.update(cx, |s, _| s.follow_up.push(text));
            cx.notify();
        } else {
            self.start_turn(text, cx);
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

    /// A `/command` from the composer.
    fn run_command(&mut self, cmd: &str, cx: &mut Context<Self>) {
        let (name, args) = cmd.split_once(char::is_whitespace).map(|(n, a)| (n, a.trim())).unwrap_or((cmd, ""));
        let Some(backend) = self.backend.clone() else {
            self.push_note("error", "the agent is not running");
            return;
        };
        let mode = || match args {
            "fade" => Mode::Fade,
            "auto" => Mode::Auto,
            _ => Mode::Compact,
        };
        match name {
            "help" => {
                let lines: Vec<String> = COMMANDS.iter().map(|(n, d)| format!("/{n} — {d}")).collect();
                self.push_note("info", lines.join("\n"));
            }
            "model" => self.set_model(args, cx),
            "preview" => self.preview_reduction(mode(), cx),
            "reduce" => self.reduce_now(mode(), cx),
            "undo" => match backend.agent.reductions().last().map(|r| r.entry) {
                Some(id) => self.restore_reduction(id, cx),
                None => self.push_note("info", "no reduction in force"),
            },
            other => match backend.command(other, args) {
                Ok(result) => {
                    for n in result.notices {
                        self.push_note(match n.level {
                            reactor_context::Level::Info => "info",
                            reactor_context::Level::Warning => "warning",
                            reactor_context::Level::Error => "error",
                        }, n.message);
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
            },
        }
        cx.notify();
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
        if statuses.is_empty() {
            left = left.child(div().text_color(cx.theme().muted_foreground).text_size(cx.theme().font_size * 0.85).child("no goal, identity or scenario set — /help"));
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
            .on_action(cx.listener(|this, _: &ComposerEsc, window, cx| {
                if this.session.read(cx).phase == AgentPhase::Working {
                    this.interrupt(window, cx);
                }
            }))
            .child(crate::chrome::title_bar(format!("REactor — {}", self.cwd.display()), self.menu_bar.as_ref(), cx))
            .child(self.dock_area.clone())
            .child(self.render_status_bar(window, cx))
            .when_some(notification_layer, |el, layer| el.child(layer))
            .when_some(dialog_layer, |el, layer| el.child(layer))
    }
}
