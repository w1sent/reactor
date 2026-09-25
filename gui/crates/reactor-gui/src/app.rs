//! The application window: one session, one pi child, the docked layout of
//! gui/SPEC.md §6.
//!
//! The GUI holds no facts: the transcript renders what pi streams
//! ([`crate::session::Session`]), the side panels render what the `reactor`
//! CLI answers ([`reactor_cli::ReactorClient`]), and extension views render
//! the envelopes of [ADR-0032](../../docs/adr/0032-the-gui-extends-pis-rpc-through-existing-channels-only.md).
//! Panels are views over one state object — stateless presentation, per the
//! ADR-0029 discipline.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{DockArea, DockLayout, DockPlacement, DockSkin, panel_handle};
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Root, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{App, Entity, KeyBinding, Render, SharedString, Window, div, px};
use serde::Deserialize;
use serde_json::{Value, json};

use reactor_cli::{CliClient, ReactorClient};
use reactor_rpc::{Command, Incoming, RpcClient, SpawnConfig};

use crate::contract::{self, View};
use crate::panels::{
    ConsolePanel, ExtensionViewsPanel, ServicesPanel, ToolsPanel, ToolsetsPanel, TranscriptPanel,
    TreePanel,
};
use crate::session::{AgentPhase, ChatItem, Session};
use crate::views;

/// How long one RPC round trip may take from the UI thread. pi answers every
/// command eventually; the bound keeps a wedged child from freezing a panel.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the event pump drains the child's channel (gui/SPEC.md §2).
const PUMP_TICK: Duration = Duration::from_millis(50);
/// How much of a tool card's output the transcript renders before it sheds
/// the tail (the raw/debug view keeps the rest, gui/SPEC.md §2).
pub const TOOL_OUTPUT_MAX_CHARS: usize = 2000;

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

actions!(
    reactor_gui,
    [
        /// Esc, per pi's interactive semantics: clear the queue, then abort.
        ComposerEsc,
        /// Refresh the catalogue and services panels (`reactor` round trips).
        RefreshReactor,
        /// Reload extensions — a `/reload` prompt through RPC `prompt`.
        ReloadExtensions,
        /// Kill the pi child and respawn it — the GUI's restart affordance.
        RestartAgent,
    ]
);

/// Model picked from the header dropdown.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct SelectModelAction {
    pub provider: String,
    pub model_id: String,
}

/// Thinking level picked from the header dropdown.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct SelectThinkingAction {
    pub level: String,
}

/// A catalogue row's enable/disable — the selector's mutation, through the
/// CLI (gui/SPEC.md §5).
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct ToggleToolAction {
    pub id: String,
    pub enable: bool,
}

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct ToggleToolsetAction {
    pub id: String,
    pub enable: bool,
}

/// An envelope view's action — dispatched as the view's own event command
/// through RPC `prompt` (ADR-0032's inbound channel).
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct UiEventAction {
    pub command: String,
    pub payload: String,
}

/// The session tree's branch switch — through the tree bridge
/// (gui/SPEC.md §4.6), disabled while the agent streams.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct SwitchBranchAction {
    pub entry_id: String,
}

/// An envelope view's dismiss — the extension clears its own widget.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct DismissViewAction {
    pub view_id: String,
}

// ---------------------------------------------------------------------------
// ReactorApp — the root view
// ---------------------------------------------------------------------------

/// The one window's state: the pi child, the session view model, and the CLI
/// client for the side panels (gui/SPEC.md §6).
pub struct ReactorApp {
    pub cwd: PathBuf,
    client: Option<RpcClient>,
    reactor: CliClient,
    pub session: Entity<Session>,
    pub composer: Entity<TextareaState>,
    /// Input/editor scratch states reused across extension dialogs — created
    /// once at startup, because entity creation needs a window.
    pub dialog_input: Entity<InputState>,
    pub dialog_editor: Entity<TextareaState>,
    /// Extension views (the envelopes of ADR-0032), most recent last.
    pub views: Vec<View>,
    /// Extension dialogs awaiting an answer (`select|confirm|input|editor`),
    /// oldest first — rendered as native modals, answered with the request's
    /// id (gui/SPEC.md §4.5).
    pub dialogs: VecDeque<reactor_rpc::ExtensionUiRequest>,
    /// Transient `ctx.ui.notify` toasts, most recent last.
    pub notifications: Vec<(String, String)>,
    /// `get_commands` — the command menu's feed (gui/SPEC.md §6).
    pub commands: Vec<CommandInfo>,
    /// `get_available_models` — the header picker's feed: provider, id, name.
    pub models: Vec<ModelInfo>,
    /// `get_available_thinking_levels`.
    pub thinking_levels: Vec<String>,
    /// Side-panel data, from the CLI (gui/SPEC.md §5).
    pub catalogue: Option<reactor_cli::ToolsPayload>,
    pub toolsets: Option<reactor_cli::ToolsetsPayload>,
    pub services: Option<reactor_cli::ServicesPayload>,
    /// Whether a `reactor tools`/`toolsets --format json` round trip is in
    /// flight — the Tools/Toolsets panels' loading indicator (gui/SPEC.md
    /// §5). One flag for both: they share the one fetch.
    pub catalogue_loading: bool,
    /// Whether a `reactor services --format json` round trip is in flight.
    pub services_loading: bool,
    /// `get_tree` — the session tree panel's feed.
    pub tree: Option<Value>,
    /// Whether a `get_tree` RPC round trip is in flight.
    pub tree_loading: bool,
    pub leaf_id: Option<String>,
    /// The session file (from `get_state`), for the window title.
    pub session_file: Option<String>,
    /// Queue text restored on Esc-interrupt (gui/SPEC.md §6) — surfaced as a
    /// notification in v0.1, since setting the composer needs a window.
    pub queue_restored: Option<String>,
    /// The last error pi reported, if any.
    pub last_error: Option<String>,
    /// Everything the console pane has shown since it was last cleared.
    pub console_buffer: crate::console::ConsoleBuffer,
    /// The command running in it, if one is.
    console_session: Option<crate::console::ConsoleSession>,
    /// What the running command is, for the pane's header.
    pub console_command: Option<String>,
    dock_area: Entity<DockArea>,
}

/// One entry of `get_commands` (gui/SPEC.md §6): invocable as `/name`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CommandInfo {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

/// `get_available_models`' model, as the GUI displays it.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelInfo {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// The `clear_queue` response payload: the queue texts to restore — the
/// interactive-Esc behavior of gui/SPEC.md §6.
#[derive(Debug, Clone, Deserialize)]
struct QueueText {
    #[serde(default, rename = "steering")]
    steering: Vec<String>,
    #[serde(default, rename = "followUp")]
    follow_up: Vec<String>,
}

impl ReactorApp {
    /// Open the main window: theme, dock, pi child, event pump — in the
    /// order the spec's §6 anatomy needs.
    pub fn open(cx: &mut App, args: &crate::start::ResolvedLaunch) -> anyhow::Result<()> {
        cx.bind_keys(vec![KeyBinding::new("escape", ComposerEsc, None)]);

        // Record the workdir as recent here, not only on the chooser's own
        // "Open session" button: this is the one choke point every launch
        // path funnels through (a positional cwd on the command line, the
        // chooser, the session picker), so it is the only place that can
        // never miss a launch (the bug: recents stayed empty for anyone who
        // always launches `reactor-gui <dir>` directly, gui/SPEC.md §8).
        let mut config = crate::start::GuiConfig::load();
        config.push_workdir(&args.cwd);

        let bounds = gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds::centered(
            None,
            gpui_kit::size(px(1420.), px(920.)),
            cx,
        ));
        let mut app_entity: Option<Entity<ReactorApp>> = None;
        let _handle = cx.open_window(
            gpui_kit::WindowOptions {
                window_bounds: Some(bounds),
                ..Default::default()
            },
            |window, cx| {
                // The theme rides the registry (gui/SPEC.md §7) before any
                // component renders.
                crate::theme::install_ayu_dark(cx);
                let app: Entity<ReactorApp> = cx.new(|cx| ReactorApp::new(window, cx, args));
                app_entity = Some(app.clone());
                let root_view: gpui_kit::AnyView = app.into();
                cx.new(|cx| Root::new(root_view, window, cx))
            },
        )?;
        cx.activate(true);

        // The pump bridges reactor-rpc's reader thread into entity updates —
        // gui/SPEC.md §2.
        let app = app_entity.expect("window's root view was built");
        app.update(cx, |app, cx| {
            app.start_pump(cx);
            app.refresh_models(cx);
            app.refresh_commands(cx);
            app.refresh_tree(cx);
            app.refresh_catalogue(cx);
            app.refresh_services(false, cx);
            // Only a resumed/continued/forked session has pre-existing
            // history to backfill. Calling this unconditionally raced the
            // live event stream for a brand-new session: `get_entries`'
            // round trip could return *after* the first turn had already
            // streamed in live, and its entries — now including that same
            // turn, freshly flushed to the session file — were appended a
            // second time, duplicating the assistant's reply (and its
            // thinking block) verbatim in the transcript.
            if args.launch_args.resumes_existing_session() {
                app.load_entries(cx);
            }
            app.load_state(cx);
        });

        Ok(())
    }

    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        args: &crate::start::ResolvedLaunch,
    ) -> Self {
        let _weak_app = cx.weak_entity();
        let session = cx.new(|_| Session::new());
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 8)
                .submit_on_enter(true)
                .placeholder(
                    "Prompt — Enter sends (follow-up while streaming), Shift+Enter newline",
                )
        });
        let dialog_input = cx.new(|cx| InputState::new(window, cx).placeholder("…"));
        let dialog_editor = cx.new(|cx| TextareaState::new(window, cx));
        let cwd = args.cwd.clone();
        let reactor = CliClient::new(Some(cwd.clone()));

        // Composer: Enter submits (queued as a follow-up while streaming) —
        // the send builds the prompt with pi's queueing contract (§6).
        cx.subscribe_in(
            &composer,
            window,
            |this, _composer, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift, .. } = event {
                    if !*shift {
                        this.send_composer(window, cx);
                    }
                }
            },
        )
        .detach();

        let weak_app = cx.weak_entity();
        let (dock_area, _skin) = DockSkin::dock_area("reactor-main", Some(1), window, cx);
        let center = DockLayout::tabs().panel_view(
            panel_handle(cx.new(|cx| TranscriptPanel::new(weak_app.clone(), composer.clone(), cx))),
            cx,
        );
        let left = DockLayout::tabs().panel_view(
            panel_handle(cx.new(|cx| TreePanel::new(weak_app.clone(), window, cx))),
            cx,
        );
        let right = DockLayout::v_split()
            .child(
                DockLayout::tabs()
                    .panel_view(
                        panel_handle(cx.new(|cx| ToolsPanel::new(weak_app.clone(), window, cx))),
                        cx,
                    )
                    .panel_view(
                        panel_handle(cx.new(|cx| ToolsetsPanel::new(weak_app.clone(), window, cx))),
                        cx,
                    )
                    .panel_view(
                        panel_handle(
                            cx.new(|cx| ExtensionViewsPanel::new(weak_app.clone(), window, cx)),
                        ),
                        cx,
                    ),
                None,
            )
            .child(
                DockLayout::tabs().panel_view(
                    panel_handle(cx.new(|cx| ServicesPanel::new(weak_app.clone(), window, cx))),
                    cx,
                ),
                Some(px(280.)),
            );

        let bottom = DockLayout::tabs().panel_view(
            panel_handle(cx.new(|cx| ConsolePanel::new(weak_app.clone(), window, cx))),
            cx,
        );

        dock_area.update(cx, |dock, cx| {
            dock.set_center(center, window, cx);
            for (placement, layout, size) in [
                (DockPlacement::Left, left, px(300.)),
                (DockPlacement::Right, right, px(380.)),
                (DockPlacement::Bottom, bottom, px(220.)),
            ] {
                dock.set_dock(placement, layout, window, cx);
                dock.set_dock_size(placement, size, window, cx);
            }
        });

        // Kill the child on app quit (gui/SPEC.md §3): pi handles SIGTERM with
        // its own cleanup, so the watchdog below is the fallback, not the path.
        let (client, startup_error) = Self::spawn_pi(cx, args);
        let quit_client = client.clone();
        cx.on_app_quit(move |_, _| {
            quit_client.shutdown();
            async {}
        })
        .detach();

        if let Some(message) = startup_error {
            cx.spawn(async move |this, cx| {
                this.update(cx, |app, cx| {
                    app.session.update(cx, |session, _| {
                        session.last_error = Some(message.clone());
                        session.items.push(ChatItem::Error {
                            message: message.clone(),
                        });
                    });
                })
                .ok();
            })
            .detach();
        }

        Self {
            cwd,
            client: Some(client),
            reactor,
            session,
            composer,
            dialog_input,
            dialog_editor,
            views: Vec::new(),
            dialogs: std::collections::VecDeque::new(),
            notifications: Vec::new(),
            commands: Vec::new(),
            models: Vec::new(),
            thinking_levels: Vec::new(),
            catalogue: None,
            toolsets: None,
            services: None,
            // Set once here rather than left to default-false: `open()`
            // kicks off the first fetch of each right after construction, so
            // a panel's first-ever render should already say "loading",
            // never a beat of "no data" first (the missing-loading-indicator
            // bug this fixes).
            catalogue_loading: true,
            services_loading: true,
            tree: None,
            tree_loading: true,
            leaf_id: None,
            session_file: None,
            queue_restored: None,
            last_error: None,
            console_buffer: crate::console::ConsoleBuffer::default(),
            console_session: None,
            console_command: None,
            dock_area,
        }
    }

    /// Spawn `pi --mode rpc` with the launch flags and the handshake
    /// (gui/SPEC.md §3). Failures surface as an error item — the GUI stays
    /// open and shows why it cannot reach pi, `reactor doctor` being the
    /// follow-up (gui/SPEC.md §8).
    fn spawn_pi(
        _cx: &mut Context<Self>,
        args: &crate::start::ResolvedLaunch,
    ) -> (RpcClient, Option<String>) {
        let config = SpawnConfig {
            program: None,
            args: args.launch_args.pi_args(),
            cwd: Some(args.cwd.clone()),
            env: vec![("REACTOR_GUI".to_owned(), "1".to_owned())],
        };
        match RpcClient::spawn(&config) {
            Ok(client) => (client, None),
            Err(e) => {
                let message = format!("could not start pi: {e}");
                // A dead client: all sends fail, the event channel closes at
                // once — the GUI renders the failure and stays open.
                let dead = RpcClient::connect(
                    std::io::BufReader::new(std::io::empty()),
                    Box::new(std::io::sink()),
                    None,
                );
                (dead, Some(message))
            }
        }
    }

    /// The event pump (gui/SPEC.md §2): drain the child's channel every tick
    /// and ingest. A poll loop, not a blocking recv — the pump future runs on
    /// gpui's foreground executor and only ever sleeps on a background timer.
    fn start_pump(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let Some(receiver) = client.take_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PUMP_TICK).await;
                let mut drained: Vec<Incoming> = Vec::new();
                let mut disconnected = false;
                while let Ok(incoming) = receiver.try_recv() {
                    drained.push(incoming);
                }
                if let Err(mpsc::TryRecvError::Disconnected) = receiver.try_recv() {
                    // The sender is dropped and the channel drained — the
                    // child is gone.
                    disconnected = true;
                }
                let ok = this
                    .update(cx, |app, cx| {
                        for incoming in drained {
                            app.ingest(incoming, cx);
                        }
                        // The console rides the same tick: one timer for
                        // everything that streams, rather than a second loop
                        // keeping its own time.
                        app.drain_console(cx);
                    })
                    .is_ok();
                if !ok || disconnected {
                    break;
                }
            }
        })
        .detach();
    }

    /// Ingest one incoming line: the session view model learns, the contract
    /// layer learns (dialogs, views, notifications) — gui/SPEC.md §4.4 (the
    /// GUI holds no facts beyond the render).
    fn ingest(&mut self, incoming: Incoming, cx: &mut Context<Self>) {
        if let Incoming::ExtensionUiRequest(ref request) = incoming {
            use reactor_rpc::UiMethod;
            match &request.method {
                UiMethod::Notify {
                    message,
                    notify_type,
                } => {
                    self.push_note(notify_type.clone(), message.clone());
                    cx.notify();
                    return;
                }
                UiMethod::Select { .. }
                | UiMethod::Confirm { .. }
                | UiMethod::Input { .. }
                | UiMethod::Editor { .. } => {
                    self.dialogs.push_back(request.clone());
                    cx.notify();
                    return;
                }
                UiMethod::SetWidget {
                    widget_key,
                    widget_lines,
                    ..
                } => {
                    // The envelope contract (ADR-0032): a `reactor:` widget
                    // with a marker line is a view; anything else is a plain
                    // text widget rendered above the composer.
                    let lines: &[String] =
                        widget_lines.as_ref().map(|l| l.as_slice()).unwrap_or(&[]);
                    if widget_key.starts_with(contract::KEY_PREFIX) {
                        if let Some(view) = View::parse(widget_key, lines) {
                            self.views.retain(|v| v.view_id != view.view_id);
                            self.views.push(view);
                        }
                        cx.notify();
                        return;
                    }
                }
                _ => {}
            }
        }
        self.session
            .update(cx, |session, _| session.ingest(&incoming));
        // A settled agent is the moment to re-pull the tree (gui/SPEC.md §6).
        if let Incoming::Event(reactor_rpc::Event::AgentSettled) = incoming {
            self.refresh_tree(cx);
        }
        cx.notify();
    }

    fn push_note(&mut self, kind: impl Into<SharedString>, message: impl Into<SharedString>) {
        self.notifications
            .push((kind.into().to_string(), message.into().to_string()));
        if self.notifications.len() > 3 {
            self.notifications.remove(0);
        }
    }

    // -- fetches ------------------------------------------------------------

    pub fn refresh_models(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let thinking_client = client.clone();
        cx.spawn(async move |this, cx| {
            let models = cx
                .background_spawn(async move {
                    client.request_timeout(Command::GetAvailableModels, REQUEST_TIMEOUT)
                })
                .await;
            let levels = cx
                .background_spawn(async move {
                    thinking_client
                        .request_timeout(Command::GetAvailableThinkingLevels, REQUEST_TIMEOUT)
                })
                .await;
            this.update(cx, |app, cx| {
                if let Ok(response) = models {
                    app.models = response
                        .data
                        .and_then(|d| d.get("models").cloned())
                        .and_then(|m| serde_json::from_value::<Vec<ModelInfo>>(m).ok())
                        .unwrap_or_default();
                }
                if let Ok(response) = levels {
                    app.thinking_levels = response
                        .data
                        .and_then(|d| d.get("levels").cloned())
                        .and_then(|l| serde_json::from_value::<Vec<String>>(l).ok())
                        .unwrap_or_default();
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn refresh_commands(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let response = cx
                .background_spawn(async move {
                    client.request_timeout(Command::GetCommands, REQUEST_TIMEOUT)
                })
                .await;
            this.update(cx, |app, cx| {
                if let Ok(response) = response {
                    app.commands = response
                        .data
                        .and_then(|d| d.get("commands").cloned())
                        .and_then(|c| serde_json::from_value::<Vec<CommandInfo>>(c).ok())
                        .unwrap_or_default();
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn refresh_tree(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        self.tree_loading = true;
        cx.spawn(async move |this, cx| {
            let response = cx
                .background_spawn(async move {
                    client.request_timeout(Command::GetTree, REQUEST_TIMEOUT)
                })
                .await;
            this.update(cx, |app, cx| {
                if let Ok(response) = response {
                    if response.success {
                        app.tree = response.data.clone();
                        app.leaf_id = response
                            .data
                            .and_then(|d| d.get("leafId").cloned())
                            .and_then(|l| l.as_str().map(str::to_owned));
                    }
                }
                app.tree_loading = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn refresh_catalogue(&mut self, cx: &mut Context<Self>) {
        let reactor = self.reactor.clone();
        self.catalogue_loading = true;
        cx.spawn(async move |this, cx| {
            let (tools, toolsets) = cx
                .background_spawn(async move {
                    (
                        ReactorClient::tools(&reactor),
                        ReactorClient::toolsets(&reactor),
                    )
                })
                .await;
            this.update(cx, |app, cx| {
                app.catalogue = tools.ok();
                app.toolsets = toolsets.ok();
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
                if let Ok(payload) = services {
                    app.services = Some(payload);
                }
                app.services_loading = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// History: a resumed session's transcript comes from the session file
    /// (`get_entries`), since the event stream only carries new activity
    /// (gui/SPEC.md §2).
    pub fn load_entries(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let response = cx
                .background_spawn(async move {
                    client.request_timeout(Command::GetEntries { since: None }, REQUEST_TIMEOUT)
                })
                .await;
            this.update(cx, |app, cx| {
                if let Ok(response) = response {
                    let entries = response
                        .data
                        .and_then(|d| d.get("entries").cloned())
                        .and_then(|e| serde_json::from_value::<Vec<Value>>(e).ok())
                        .unwrap_or_default();
                    app.session.update(cx, |session, _| {
                        for entry in entries {
                            session.ingest_entry(&entry);
                        }
                    });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn load_state(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let response = cx
                .background_spawn(async move {
                    client.request_timeout(Command::GetState, REQUEST_TIMEOUT)
                })
                .await;
            this.update(cx, |app, cx| {
                if let Ok(response) = response {
                    if let Some(data) = &response.data {
                        app.session.update(cx, |session, _| {
                            session.session_file = data
                                .get("sessionFile")
                                .and_then(Value::as_str)
                                .map(str::to_owned);
                            session.model =
                                data.get("model").filter(|m| !m.is_null()).and_then(|m| {
                                    let provider = m.get("provider").and_then(Value::as_str)?;
                                    let id = m.get("id").and_then(Value::as_str)?;
                                    Some(format!("{provider}/{id}"))
                                });
                            session.thinking_level = data
                                .get("thinkingLevel")
                                .and_then(Value::as_str)
                                .map(str::to_owned);
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // -- the console --------------------------------------------------------

    /// Whether a command is running — the pane's input sends to it instead of
    /// starting a new one, and the Stop affordance is live.
    pub fn console_busy(&self) -> bool {
        self.console_session.is_some()
    }

    /// Run a line the user typed, through their shell so pipes and `&&`
    /// behave as they would in a terminal.
    pub fn console_run_shell(&mut self, line: String, cx: &mut Context<Self>) {
        let cwd = self.cwd.clone();
        self.start_console(
            line.clone(),
            crate::console::ConsoleSession::shell(&line, Some(cwd)),
            cx,
        );
    }

    /// Run a command the GUI built itself — `reactor install <id>` from the
    /// Tools panel. Not routed through the shell: the arguments are already
    /// separated and must not be re-split by quoting rules.
    pub fn console_run(&mut self, program: &str, args: Vec<String>, cx: &mut Context<Self>) {
        let cwd = self.cwd.clone();
        let shown = std::iter::once(program.to_owned())
            .chain(args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        self.start_console(
            shown,
            crate::console::ConsoleSession::program(program, &args, Some(cwd)),
            cx,
        );
    }

    fn start_console(
        &mut self,
        shown: String,
        spawned: std::io::Result<crate::console::ConsoleSession>,
        cx: &mut Context<Self>,
    ) {
        if self.console_busy() {
            // One command at a time: a second would interleave its output
            // into the same pane with no way to tell which wrote what.
            self.push_note("warning", "a command is already running");
            return;
        }
        self.console_buffer.push_line(format!("$ {shown}"));
        match spawned {
            Ok(session) => {
                self.console_session = Some(session);
                self.console_command = Some(shown);
            }
            Err(e) => self.console_buffer.push_line(format!("could not run: {e}")),
        }
        cx.notify();
    }

    /// Send a line to the running command's stdin — how an installer's
    /// "Proceed? [y/N]" gets answered.
    pub fn console_send(&mut self, line: String, cx: &mut Context<Self>) {
        let Some(session) = self.console_session.as_mut() else {
            return;
        };
        // Echoed because the command's own output will not contain it: the
        // pane is a pipe, not a terminal echoing keystrokes.
        self.console_buffer.push_line(format!("> {line}"));
        if let Err(e) = session.write_line(&line) {
            self.console_buffer
                .push_line(format!("could not send: {e}"));
        }
        cx.notify();
    }

    pub fn console_stop(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.console_session.as_mut() {
            session.kill();
            self.console_buffer.push_line("^C");
        }
        cx.notify();
    }

    pub fn console_clear(&mut self, cx: &mut Context<Self>) {
        self.console_buffer.clear();
        cx.notify();
    }

    /// Move whatever the running command has produced into the pane, and
    /// notice when it finishes.
    fn drain_console(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.console_session.as_mut() else {
            return;
        };
        let before = self.console_buffer.lines().len();
        let finished = session.drain(&mut self.console_buffer);
        let grew = self.console_buffer.lines().len() != before;
        let Some(code) = finished else {
            if grew {
                cx.notify();
            }
            return;
        };
        self.console_session = None;
        self.console_command = None;
        self.console_buffer.push_line(match code {
            Some(0) => "[done]".to_owned(),
            Some(code) => format!("[exit {code}]"),
            None => "[killed]".to_owned(),
        });
        // A command that just finished may well have changed what is
        // installed — an install certainly did, and a hand-typed `brew
        // install` counts too. Re-reading the catalogue is cheap and keeps
        // the Tools panel honest without the user knowing to refresh it.
        self.refresh_catalogue(cx);
        self.refresh_services(false, cx);
        cx.notify();
    }

    // -- user actions -------------------------------------------------------

    /// Enter in the composer: send when idle, queue a follow-up while
    /// streaming (gui/SPEC.md §6).
    ///
    /// Clears the field *before* sending, not after: gpui-component's
    /// textarea can dispatch one `Enter` keystroke to more than one action
    /// listener (its own key context plus an ambient one both matching the
    /// same action), which was reaching this function twice for a single
    /// keystroke — the "prompt sent twice" bug. Clearing first makes the
    /// function idempotent against that: a second call reads the
    /// already-emptied field and returns at the guard above, instead of
    /// reading the same text again and sending it a second time.
    pub fn send_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        self.composer.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        let streaming = self.session.read(cx).phase != AgentPhase::Idle;
        let command = Command::Prompt {
            message: text.clone(),
            images: Vec::new(),
            streaming_behavior: if streaming {
                Some("followUp".to_owned())
            } else {
                None
            },
        };
        if client.send(command.to_value()).is_err() {
            // The pipe to pi is gone — restore what the user typed rather
            // than silently discarding it.
            self.composer.update(cx, |state, cx| {
                state.set_value(&text, window, cx);
            });
        }
    }

    /// Interrupt: `clear_queue` then `abort` — the RPC-documented interactive
    /// Esc behavior, with the queue text restored into the composer
    /// (gui/SPEC.md §6).
    pub fn interrupt(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let abort_client = client.clone();
        cx.spawn(async move |this, cx| {
            let cleared = cx
                .background_spawn(async move {
                    client.request_timeout(Command::ClearQueue, REQUEST_TIMEOUT)
                })
                .await;
            let queue_text: Option<String> = cleared
                .ok()
                .and_then(|r| r.data)
                .and_then(|d| serde_json::from_value::<QueueText>(d).ok())
                .map(|q| {
                    q.steering
                        .into_iter()
                        .chain(q.follow_up)
                        .collect::<Vec<_>>()
                })
                .map(|texts| texts.join("\n"))
                .filter(|t| !t.is_empty());
            let _ = abort_client.send(json!({ "type": "abort" }));
            this.update(cx, |app, cx| {
                if let Some(text) = queue_text {
                    app.queue_restored = Some(text);
                }
                app.push_note("info", "interrupted — queue cleared");
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Run an envelope view's action — the inbound channel of ADR-0032:
    /// extension commands via RPC `prompt`, immediate and transcript-free
    /// (verified in pi 0.87.0; see "Facts for reactor-gui" in pi-api-notes).
    pub fn run_ui_event(&mut self, command: String, payload: String, _cx: &mut Context<Self>) {
        if let Some(client) = self.client.clone() {
            let _ =
                client.send(json!({ "type": "prompt", "message": format!("{command} {payload}") }));
        }
    }

    pub fn set_model(&mut self, provider: &str, model_id: &str, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let provider = provider.to_owned();
        let model_id = model_id.to_owned();
        cx.spawn(async move |this, cx| {
            let response = cx
                .background_spawn(async move {
                    client
                        .request_timeout(Command::SetModel { provider, model_id }, REQUEST_TIMEOUT)
                })
                .await;
            this.update(cx, |app, cx| {
                match response {
                    Ok(response) => {
                        if let Some(data) = &response.data {
                            app.session.update(cx, |session, _| {
                                session.model = Some(format!(
                                    "{}/{}",
                                    data.get("provider").and_then(Value::as_str).unwrap_or(""),
                                    data.get("id").and_then(Value::as_str).unwrap_or("")
                                ));
                            });
                        }
                    }
                    Err(e) => app.push_note("error", e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn set_thinking(&mut self, level: &str, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let level = level.to_owned();
        cx.spawn(async move |this, cx| {
            let response = cx
                .background_spawn(async move {
                    client.request_timeout(Command::SetThinkingLevel(level), REQUEST_TIMEOUT)
                })
                .await;
            this.update(cx, |app, cx| {
                match response {
                    Ok(response) => {
                        app.session.update(cx, |session, _| {
                            session.thinking_level = response
                                .data
                                .and_then(|d| d.get("level").cloned())
                                .and_then(|l| l.as_str().map(str::to_owned));
                        });
                    }
                    Err(e) => app.push_note("error", e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn toggle_tool(&mut self, id: &str, enable: bool, cx: &mut Context<Self>) {
        let reactor = self.reactor.clone();
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { ReactorClient::set_tool(&reactor, &id, enable) })
                .await;
            this.update(cx, |app, cx| match result {
                Ok(_) => app.refresh_catalogue(cx),
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
                Ok(_) => app.refresh_catalogue(cx),
                Err(e) => app.push_note("error", e.to_string()),
            })
            .ok();
        })
        .detach();
    }

    pub fn reload_extensions(&mut self, cx: &mut Context<Self>) {
        if let Some(client) = self.client.clone() {
            let _ = client.send(json!({ "type": "prompt", "message": "/reload" }));
        }
        cx.notify();
    }

    /// Branch switch — the tree bridge of gui/SPEC.md §4.6: an extension
    /// command through `prompt`, which calls `ctx.navigateTree`. Rejects
    /// while streaming, so the affordance is disabled then (§4.6).
    pub fn switch_branch(&mut self, entry_id: &str, cx: &mut Context<Self>) {
        self.run_ui_event(
            "/reactor-tree".to_owned(),
            json!({ "entryId": entry_id }).to_string(),
            cx,
        );
    }

    /// Dismiss an envelope view: the GUI-side overlay goes away and the
    /// extension is told to clear its widget (it owns the view, §4.4).
    pub fn dismiss_view(&mut self, view_id: &str, cx: &mut Context<Self>) {
        if let Some(view) = self.views.iter().find(|v| v.view_id == view_id).cloned() {
            let payload = contract::event_command("", &view.view_id, "close", None);
            let payload = payload
                .split_once(' ')
                .map(|(_, payload)| payload.to_owned())
                .unwrap_or_default();
            self.run_ui_event(view.command.clone(), payload, cx);
        }
        self.views.retain(|v| v.view_id != view_id);
        cx.notify();
    }
}

impl ReactorApp {
    /// The status bar (gui/SPEC.md §6): extension statuses left (key-sorted,
    /// the `0-reactor` anchor leads), model · thinking · context% right.
    fn render_status_bar(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let session = self.session.read(cx);
        let statuses = session.statuses.clone();
        let model = session.model.clone();
        let thinking = session.thinking_level.clone();

        let mut left = h_flex().gap_3().items_center();
        for (key, text) in &statuses {
            left = left.child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .text_size(cx.theme().font_size * 0.85)
                    .child(format!("{key}: {text}")),
            );
        }
        if statuses.is_empty() {
            left = left.child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .text_size(cx.theme().font_size * 0.85)
                    .child("no extensions reporting"),
            );
        }

        // Model and thinking-level pickers — `SelectModelAction`/
        // `SelectThinkingAction` and `set_model`/`set_thinking` already
        // existed; nothing ever built the menu that dispatches them; the
        // status bar showed the current value as inert text with no way to
        // change it (the "missing model selection" gap this fixes).
        let models = self.models.clone();
        let thinking_levels = self.thinking_levels.clone();

        let mut right = h_flex().gap_2().items_center();
        right = right.child(
            Button::new("model-picker")
                .ghost()
                .small()
                .label(model.unwrap_or_else(|| "model".to_owned()))
                .disabled(models.is_empty())
                .dropdown_menu(move |mut menu, _window, _cx| {
                    for info in &models {
                        let label = if info.name.is_empty() {
                            format!("{}/{}", info.provider, info.id)
                        } else {
                            info.name.clone()
                        };
                        menu = menu.menu(
                            label,
                            Box::new(SelectModelAction {
                                provider: info.provider.clone(),
                                model_id: info.id.clone(),
                            }),
                        );
                    }
                    menu
                }),
        );
        right = right.child(
            Button::new("thinking-picker")
                .ghost()
                .small()
                .label(thinking.unwrap_or_else(|| "thinking".to_owned()))
                .disabled(thinking_levels.is_empty())
                .dropdown_menu(move |mut menu, _window, _cx| {
                    for level in &thinking_levels {
                        menu = menu.menu(
                            level.clone(),
                            Box::new(SelectThinkingAction {
                                level: level.clone(),
                            }),
                        );
                    }
                    menu
                }),
        );

        StatusBar::new().left(left).right(right)
    }

    /// The one overlay-placed extension view (if any), floated over the
    /// whole workspace — today's selector/guide semantics under the
    /// envelope contract (gui/SPEC.md §4.2's "overlay" placement matches the
    /// TUI's existing overlay behavior for those two extensions). A
    /// side-placed view docks in [`crate::panels::ExtensionViewsPanel`]
    /// instead; both share [`crate::views::render_content`].
    fn render_overlay(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let view = self
            .views
            .iter()
            .find(|v| v.placement == contract::Placement::Overlay)
            .cloned()?;
        let theme = cx.theme().clone();
        let weak = cx.weak_entity();
        let dismiss_view_id = view.view_id.clone();
        let dispatch_view = view.clone();

        Some(
            div()
                .id("view-overlay-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::black().opacity(0.6))
                .child(
                    v_flex()
                        .id("view-overlay")
                        .w(px(640.))
                        .max_h(px(560.))
                        .bg(theme.background)
                        .border_1()
                        .border_color(theme.border)
                        .rounded_lg()
                        .child(views::view_chrome(&view, {
                            let weak = weak.clone();
                            move |_window, cx| {
                                if let Some(app) = weak.upgrade() {
                                    app.update(cx, |app, cx| {
                                        app.dismiss_view(&dismiss_view_id, cx)
                                    });
                                }
                            }
                        }))
                        .child(
                            div()
                                .id("view-overlay-body")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .child(views::render_content(
                                    &view.content,
                                    &theme,
                                    move |action, row, _window, cx| {
                                        if let Some(app) = weak.upgrade() {
                                            let (command, payload) =
                                                dispatch_view.dispatch(action, row);
                                            app.update(cx, |app, cx| {
                                                app.run_ui_event(command, payload, cx)
                                            });
                                        }
                                    },
                                )),
                        )
                        .when_some(view.footer.clone(), |el, footer| {
                            el.child(views::view_footer(&footer, &theme))
                        }),
                ),
        )
    }
}

// ---------------------------------------------------------------------------
// ReactorApp — the root view's render (gui/SPEC.md §6)
// ---------------------------------------------------------------------------

impl Render for ReactorApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Root's overlay layers: notifications, dialogs (§6).
        let notification_layer = Root::render_notification_layer(window, cx);
        let dialog_layer = Root::render_dialog_layer(window, cx);
        let view_overlay = self.render_overlay(cx);

        div()
            .id("reactor-workspace")
            .size_full()
            .flex()
            .flex_col()
            .relative()
            // The status bar's model/thinking pickers dispatch these actions
            // from a `PopupMenu`, which bubbles them up the dispatch tree —
            // this is the ancestor that answers them.
            .on_action(
                cx.listener(|this, action: &SelectModelAction, _window, cx| {
                    this.set_model(&action.provider, &action.model_id, cx);
                }),
            )
            .on_action(
                cx.listener(|this, action: &SelectThinkingAction, _window, cx| {
                    this.set_thinking(&action.level, cx);
                }),
            )
            .child(self.dock_area.clone())
            .child(self.render_status_bar(window, cx))
            .when_some(notification_layer, |el, layer| el.child(layer))
            .when_some(dialog_layer, |el, layer| el.child(layer))
            .when_some(view_overlay, |el, layer| el.child(layer))
    }
}
