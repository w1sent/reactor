//! The agent, in this process.
//!
//! [`Backend`] owns a tokio runtime on its own threads and the
//! [`reactor_agent::agent::Agent`] running on it; the window talks to it through plain
//! method calls and reads what happens from one `std::sync::mpsc` channel that its pump
//! polls. gpui keeps its own executor and never sees a future: the two meet at that
//! channel and at the session store, whose short locks are the only shared state
//! (docs/adr/0042).
//!
//! Everything a panel shows is computed here into plain data ([`TreeRow`],
//! [`ContextView`], [`PlanView`]) so `panels.rs` renders and decides nothing.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use reactor_agent::agent::{Agent, AgentConfig, CommandResult, Event, Outcome};
use reactor_agent::budget::Plan;
use reactor_agent::context::SessionState;
use reactor_agent::entry::{Block, EntryId, Kind, Mode, Trigger};
use reactor_agent::llm::{LlmSummarizer, Switchable};
use reactor_agent::provider::{AnyLlm, default_window};
use reactor_agent::store::Store;
use reactor_agent::tools::Tools;
use reactor_context::settings::{ContextSettings, Origin, Settings};
use reactor_core::Paths;
use tokio_util::sync::CancellationToken;

pub type GuiAgent = Agent<Switchable<AnyLlm>, LlmSummarizer<Switchable<AnyLlm>>>;

/// What the window learns from the backend.
#[derive(Debug, Clone)]
pub enum UiEvent {
    Agent(Event),
    TurnEnded(std::result::Result<Outcome, String>),
    /// A manual reduction finished.
    Reduced(std::result::Result<EntryId, String>),
    Preview(std::result::Result<PlanView, String>),
    Context(ContextView),
}

/// How to start a session.
#[derive(Debug, Clone, Default)]
pub struct StartOptions {
    pub cwd: PathBuf,
    /// Open this existing session directory instead of creating one.
    pub resume: Option<PathBuf>,
    /// `provider/name`; else settings' `defaultModel`; else `REACTOR_MODEL`.
    pub model: Option<String>,
}

pub struct Backend {
    runtime: tokio::runtime::Runtime,
    pub agent: Arc<GuiAgent>,
    llm: Switchable<AnyLlm>,
    summarizer: Switchable<AnyLlm>,
    ui_tx: Sender<UiEvent>,
    ui_rx: Mutex<Option<Receiver<UiEvent>>>,
    running: Mutex<Option<CancellationToken>>,
    pub paths: Paths,
    pub session_dir: PathBuf,
    /// The model in use (`provider/name`), or empty when none is chosen.
    model: Mutex<String>,
    /// The window a model gets when nothing sets one.
    default_window: Mutex<u64>,
}

/// Where a bundled directory (`prompts/scenarios`, `skills`) is: `REACTOR_PACKAGE_ROOT`,
/// then a checkout next to the running binary, then `~/.reactor/`.
pub fn find_bundled(sub: &str) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(root) = std::env::var_os("REACTOR_PACKAGE_ROOT").filter(|v| !v.is_empty()) {
        candidates.push(PathBuf::from(root).join(sub));
    }
    if let Ok(exe) = std::env::current_exe() {
        // target/{debug,release}/reactor-gui → the checkout it was built in.
        candidates.extend(exe.ancestors().skip(1).take(4).map(|a| a.join(sub)));
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".reactor").join(sub));
    }
    candidates.into_iter().find(|p| p.is_dir())
}

fn new_session_id() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{secs}-{}", std::process::id())
}

fn llm_for(spec: Option<&str>) -> (AnyLlm, String, u64) {
    match spec {
        Some(spec) => match AnyLlm::from_spec(spec) {
            Ok((llm, provider)) => (llm, spec.to_string(), default_window(&provider)),
            Err(e) => (AnyLlm::Missing(e.to_string()), String::new(), 128_000),
        },
        None => (
            AnyLlm::Missing("no model chosen -- pick one in the status bar, or set `defaultModel` in ~/.reactor/settings.json".into()),
            String::new(),
            128_000,
        ),
    }
}

impl Backend {
    pub fn start(opts: StartOptions) -> std::result::Result<Backend, String> {
        let mut paths = Paths::from_env();
        paths.cwd = Some(opts.cwd.clone());
        let settings = Settings::load(&paths);

        let (store, session_dir) = match &opts.resume {
            Some(dir) => (Store::open(dir).map_err(|e| e.to_string())?, dir.clone()),
            None => {
                let id = new_session_id();
                let dir = paths.config_dir.join("sessions").join(&id);
                (Store::create(&dir, &id, &opts.cwd).map_err(|e| e.to_string())?, dir)
            }
        };
        // Activation is per session (ADR-0038): this file, inside the session, wins over
        // the machine's.
        let paths = paths.with_session_state(session_dir.join("activation.json"));

        let spec = opts.model.clone().or_else(|| settings.default_model.clone()).or_else(|| std::env::var("REACTOR_MODEL").ok());
        let (llm, model, window) = llm_for(spec.as_deref());
        let llm = Switchable::new(llm);
        let summarizer = Switchable::new(llm.current());

        let mut cfg = AgentConfig::new(opts.cwd.clone(), paths.clone(), window);
        if let Some(d) = find_bundled("prompts/scenarios") {
            cfg.scenarios_dir = d;
        }
        cfg.skills_dir = find_bundled("skills");
        let (agent, mut events) = Agent::new(llm.clone(), LlmSummarizer { llm: summarizer.clone() }, store, Tools::standard(opts.cwd.clone()), cfg);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("reactor-agent")
            .enable_all()
            .build()
            .map_err(|e| format!("could not start the agent runtime: {e}"))?;

        let (ui_tx, ui_rx) = mpsc::channel();
        let forward = ui_tx.clone();
        runtime.spawn(async move {
            while let Some(e) = events.recv().await {
                if forward.send(UiEvent::Agent(e)).is_err() {
                    break;
                }
            }
        });

        let backend = Backend {
            runtime,
            agent: Arc::new(agent),
            llm,
            summarizer,
            ui_tx,
            ui_rx: Mutex::new(Some(ui_rx)),
            running: Mutex::new(None),
            paths,
            session_dir,
            model: Mutex::new(model),
            default_window: Mutex::new(window),
        };
        backend.sync_summarizer();
        Ok(backend)
    }

    /// The channel the window's pump drains. Once.
    pub fn take_events(&self) -> Option<Receiver<UiEvent>> {
        self.ui_rx.lock().unwrap().take()
    }

    pub fn store(&self) -> Arc<Mutex<Store>> {
        self.agent.store()
    }

    pub fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }

    pub fn is_running(&self) -> bool {
        self.running.lock().unwrap().is_some()
    }

    // -- turns ---------------------------------------------------------------------------

    /// Start a turn on `text`. The outcome arrives as [`UiEvent::TurnEnded`].
    pub fn prompt(&self, text: String) {
        let cancel = CancellationToken::new();
        *self.running.lock().unwrap() = Some(cancel.clone());
        let (agent, tx) = (self.agent.clone(), self.ui_tx.clone());
        self.runtime.spawn(async move {
            let result = agent.prompt(&text, cancel).await.map_err(|e| e.to_string());
            let _ = tx.send(UiEvent::TurnEnded(result));
        });
    }

    /// Called by the window when it sees the turn end.
    pub fn turn_finished(&self) {
        *self.running.lock().unwrap() = None;
    }

    pub fn cancel(&self) {
        if let Some(c) = self.running.lock().unwrap().as_ref() {
            c.cancel();
        }
    }

    /// A `/command`: `goal`, `guidelines`, `manifest`, `frame`, `identity`, `report`,
    /// `reactor-scenario`.
    pub fn command(&self, name: &str, args: &str) -> std::result::Result<CommandResult, String> {
        self.agent.command(name, args).map_err(|e| e.to_string())
    }

    // -- models ----------------------------------------------------------------------------

    /// Choose the model, taking effect on the next call.
    pub fn set_model(&self, spec: &str) -> std::result::Result<(), String> {
        let (llm, provider) = AnyLlm::from_spec(spec).map_err(|e| e.to_string())?;
        self.llm.set(llm);
        *self.model.lock().unwrap() = spec.to_string();
        *self.default_window.lock().unwrap() = default_window(&provider);
        self.sync_summarizer();
        Ok(())
    }

    /// The summarizer follows the session's model unless a `summarizer` setting names another.
    pub fn sync_summarizer(&self) {
        let (global, session) = self.agent.context_layers();
        let spec = session.over(&global).summarizer;
        match spec.as_deref().map(AnyLlm::from_spec) {
            Some(Ok((llm, _))) => self.summarizer.set(llm),
            _ => self.summarizer.set(self.llm.current()),
        }
    }

    /// The models to offer: the settings' list, plus the current one.
    pub fn models(&self) -> Vec<String> {
        let mut models = Settings::load(&self.paths).models;
        let current = self.model();
        if !current.is_empty() && !models.contains(&current) {
            models.insert(0, current);
        }
        models
    }

    // -- context ----------------------------------------------------------------------------

    /// Recompute the context view and send it as [`UiEvent::Context`].
    pub fn refresh_context(&self) {
        let (agent, tx) = (self.agent.clone(), self.ui_tx.clone());
        let (model, window) = (self.model(), *self.default_window.lock().unwrap());
        self.runtime.spawn(async move {
            if let Ok(m) = agent.measure().await {
                let _ = tx.send(UiEvent::Context(context_view(&agent, model, window, m.total)));
            }
        });
    }

    pub fn preview(&self, mode: Mode) {
        let (agent, tx) = (self.agent.clone(), self.ui_tx.clone());
        self.runtime.spawn(async move {
            let _ = tx.send(UiEvent::Preview(agent.preview(mode).await.map(|p| PlanView::of(&p, mode)).map_err(|e| e.to_string())));
        });
    }

    pub fn reduce_now(&self, mode: Mode) {
        let (agent, tx) = (self.agent.clone(), self.ui_tx.clone());
        self.runtime.spawn(async move {
            let _ = tx.send(UiEvent::Reduced(agent.reduce_now(mode).await.map_err(|e| e.to_string())));
        });
        self.refresh_context();
    }

    pub fn restore(&self, reduction: EntryId) -> std::result::Result<(), String> {
        self.agent.restore(reduction).map_err(|e| e.to_string())?;
        self.refresh_context();
        Ok(())
    }

    /// Change this session's layer of the context settings (`None` leaves a field alone;
    /// use [`Backend::inherit_context`] to drop the layer).
    pub fn set_session_context(&self, layer: ContextSettings) -> std::result::Result<(), String> {
        let (_, current) = self.agent.context_layers();
        self.agent.set_session_context(&layer.over(&current)).map_err(|e| e.to_string())?;
        self.sync_summarizer();
        self.refresh_context();
        Ok(())
    }

    pub fn inherit_context(&self) -> std::result::Result<(), String> {
        self.agent.set_session_context(&ContextSettings::default()).map_err(|e| e.to_string())?;
        self.sync_summarizer();
        self.refresh_context();
        Ok(())
    }

    /// "Make this the default": this session's effective layer becomes the global one.
    pub fn make_context_default(&self) -> std::result::Result<(), String> {
        let (global, session) = self.agent.context_layers();
        self.agent.set_global_context(session.over(&global)).map_err(|e| e.to_string())?;
        self.agent.set_session_context(&ContextSettings::default()).map_err(|e| e.to_string())?;
        self.refresh_context();
        Ok(())
    }

    // -- the tree ------------------------------------------------------------------------------

    pub fn tree(&self) -> Vec<TreeRow> {
        tree_rows(&self.store().lock().unwrap())
    }

    /// Move the tip: the next turn continues from `entry`.
    pub fn switch_branch(&self, entry: EntryId) -> std::result::Result<(), String> {
        if self.is_running() {
            return Err("the agent is working -- interrupt it first".into());
        }
        self.store().lock().unwrap().set_head(entry).map_err(|e| e.to_string())
    }

    pub fn session_state(&self) -> SessionState {
        self.agent.session_state()
    }
}

// -- view data -------------------------------------------------------------------------------

/// One row of the session tree.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeRow {
    pub id: EntryId,
    /// Fork depth: it grows only where the tree actually branches, not per message.
    pub depth: usize,
    pub label: String,
    /// On the current branch (root to head).
    pub on_branch: bool,
    pub is_head: bool,
    /// A safe place to continue from: a final assistant reply.
    pub can_switch: bool,
}

/// The tree, depth-first: user turns, final replies, reductions and labels. Tool rounds
/// are elided — a session of a thousand tool calls would otherwise be a thousand rows.
pub fn tree_rows(store: &Store) -> Vec<TreeRow> {
    let head = store.head();
    let on_branch: std::collections::HashSet<EntryId> = store.branch().iter().map(|e| e.id).collect();
    let mut children: std::collections::HashMap<EntryId, Vec<EntryId>> = std::collections::HashMap::new();
    for e in store.all() {
        if let Some(p) = e.parent {
            children.entry(p).or_default().push(e.id);
        }
    }
    let leaves: std::collections::HashSet<EntryId> = store.leaves().into_iter().collect();
    let mut rows = Vec::new();
    // Iterative depth-first: a long session must not blow the stack.
    let mut stack: Vec<(EntryId, usize)> = vec![(0, 0)];
    while let Some((id, depth)) = stack.pop() {
        let Some(entry) = store.get(id) else { continue };
        let kids = children.get(&id).cloned().unwrap_or_default();
        let forks = kids.len() > 1;
        let final_reply = matches!(&entry.kind, Kind::Assistant { blocks, .. } if !blocks.iter().any(|b| matches!(b, Block::ToolCall { .. })));
        let shown = match &entry.kind {
            Kind::User { .. } | Kind::Reduction(_) | Kind::Label { .. } => true,
            Kind::Assistant { .. } => final_reply || forks || leaves.contains(&id),
            _ => false,
        };
        if shown {
            let label = match &entry.kind {
                Kind::User { text } => format!("you: {}", first_words(text, 60)),
                Kind::Assistant { blocks, .. } => {
                    let text = blocks.iter().find_map(|b| match b { Block::Text { text } => Some(text.as_str()), _ => None }).unwrap_or("");
                    format!("agent: {}", first_words(text, 60))
                }
                Kind::Reduction(r) => format!("context reduced ({:?}, {} entries)", r.mode, r.covers.len()),
                Kind::Label { text } => text.clone(),
                _ => String::new(),
            };
            rows.push(TreeRow { id, depth, label, on_branch: on_branch.contains(&id), is_head: id == head, can_switch: final_reply });
        }
        // Children in log order; the first child continues at this depth, the others fork.
        for (i, k) in kids.iter().enumerate().rev() {
            stack.push((*k, if forks && i > 0 { depth + 1 } else { depth }));
        }
    }
    rows
}

fn first_words(text: &str, n: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() <= n { line.to_string() } else { format!("{}…", line.chars().take(n - 1).collect::<String>()) }
}

/// What the context panel shows.
#[derive(Debug, Clone)]
pub struct ContextView {
    pub model: String,
    /// Estimated tokens in the next request.
    pub used: u64,
    pub window: u64,
    pub hard: u64,
    pub trigger: u64,
    pub mode: String,
    pub pct: f64,
    pub keep: f64,
    pub reserve: u64,
    pub summarizer: Option<String>,
    /// Where each setting comes from: this session, or the default.
    pub origins: Vec<(String, Origin)>,
    pub reductions: Vec<reactor_agent::agent::ReductionInfo>,
}

impl ContextView {
    pub fn percent(&self) -> u64 {
        if self.hard == 0 { 0 } else { (self.used * 100 / self.hard).min(999) }
    }
}

pub fn context_view(agent: &GuiAgent, model: String, _default_window: u64, used: u64) -> ContextView {
    let b = agent.effective_budget();
    let (global, session) = agent.context_layers();
    ContextView {
        model,
        used,
        window: b.window,
        hard: b.hard(),
        trigger: b.trigger(),
        mode: match b.mode {
            Mode::Auto => "auto",
            Mode::Fade => "fade",
            Mode::Compact => "compact",
        }
        .into(),
        pct: b.pct,
        keep: b.keep,
        reserve: b.reserve,
        summarizer: session.over(&global).summarizer,
        origins: session.origins().into_iter().map(|(k, o)| (k.to_string(), o)).collect(),
        reductions: agent.reductions(),
    }
}

/// What a reduction would do, for a preview.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanView {
    pub mode: Mode,
    pub entries: usize,
    pub mechanical: usize,
    pub mechanical_tokens: u64,
    pub conceptual: usize,
    pub conceptual_tokens: u64,
    pub before_tokens: u64,
    pub after_tokens: u64,
    pub needs_model: bool,
}

impl PlanView {
    pub fn of(p: &Plan, mode: Mode) -> PlanView {
        PlanView {
            mode,
            entries: p.covers.len(),
            mechanical: p.mechanical.len(),
            mechanical_tokens: p.mechanical_tokens,
            conceptual: p.conceptual.len(),
            conceptual_tokens: p.conceptual_tokens,
            before_tokens: p.before_tokens,
            after_tokens: p.estimated_after_tokens,
            needs_model: p.needs_summarizer(),
        }
    }
}

pub fn trigger_label(t: Trigger) -> &'static str {
    match t {
        Trigger::Budget => "automatic",
        Trigger::Manual => "manual",
    }
}

/// Session directories under `~/.reactor/sessions/`, newest first — the picker's feed.
pub fn list_sessions(paths: &Paths, cwd: Option<&Path>) -> Vec<SessionSummary> {
    let dir = paths.config_dir.join("sessions");
    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };
    let mut out: Vec<SessionSummary> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let store = Store::open(&path).ok()?;
            let session_cwd = match &store.get(0)?.kind {
                Kind::Session { cwd, .. } => cwd.clone(),
                _ => return None,
            };
            if let Some(want) = cwd
                && Path::new(&session_cwd) != want
            {
                return None;
            }
            let first_user = store.all().iter().find_map(|e| match &e.kind {
                Kind::User { text } => Some(first_words(text, 80)),
                _ => None,
            });
            let ts = store.get(0).map(|e| e.ts).unwrap_or(0);
            Some(SessionSummary { dir: path, cwd: session_cwd, first_prompt: first_user, started_ms: ts, entries: store.len() })
        })
        .collect();
    out.sort_by(|a, b| b.started_ms.cmp(&a.started_ms));
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    pub dir: PathBuf,
    pub cwd: String,
    pub first_prompt: Option<String>,
    pub started_ms: u64,
    pub entries: usize,
}
