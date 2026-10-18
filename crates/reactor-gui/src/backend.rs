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
use reactor_context::settings::{ContextSettings, MemorySettings, Origin, Settings};
use reactor_core::Paths;
use reactor_db::Case;
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
    /// The context window, taken apart.
    Inspect(std::result::Result<reactor_agent::inspect::ContextPreview, String>),
    Context(ContextView),
    /// Something to tell the person, from work done in the background.
    Notice {
        kind: &'static str,
        text: String,
    },
    /// Vectors are being made: how far along, or `None` when the run is over.
    Embedding(Option<reactor_db::corpus::Progress>),
    /// What a menu shows has changed in the background (an Ollama model's thinking levels).
    Redraw,
}

/// What every session open in one window shares: the case (one handle to its database, not
/// one per session) and the embedding model (so that choosing it applies to all).
#[derive(Clone)]
pub struct Shared {
    pub case: Case,
    pub embedder: reactor_agent::embed::EmbedderSlot,
    /// The MCP servers, connected to once for the window rather than once per session.
    pub mcp: Arc<reactor_agent::mcp::McpHost>,
}

/// How to start a session.
#[derive(Clone, Default)]
pub struct StartOptions {
    pub cwd: PathBuf,
    /// Open this existing session of the cwd's case instead of creating one.
    pub resume: Option<String>,
    /// `provider/name`; else settings' `defaultModel`; else `REACTOR_MODEL`.
    pub model: Option<String>,
    /// What the window's other sessions already have open. `None` for the first.
    pub shared: Option<Shared>,
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
    /// The user's embedding model, shared with the agent so that choosing one applies at once.
    pub embedder: reactor_agent::embed::EmbedderSlot,
    /// The MCP servers' connections (ADR-0048), shared with the window's other sessions.
    pub mcp: Arc<reactor_agent::mcp::McpHost>,
    embed_cancel: Mutex<Option<CancellationToken>>,
    /// The cwd's case, or the unsaved state it has until it is saved as one (ADR-0045).
    pub case: Case,
    pub session_id: String,
    /// The session's own directory in the case (`activation.json`).
    pub session_dir: PathBuf,
    /// The model in use (`provider/name`), or empty when none is chosen.
    model: Mutex<String>,
    /// The thinking level this session sends (`off`, `low`, `medium`, `high`), or `None` for
    /// the model's own default. Starts at the global `defaultThinking`.
    thinking: Mutex<Option<String>>,
    /// The levels the server reported for the Ollama model in use, as `(spec, levels)`.
    ollama_levels: Arc<Mutex<Option<(String, Vec<String>)>>>,
    /// The window a model gets when nothing sets one.
    default_window: Mutex<u64>,
}

/// Where a bundled directory (`prompts/scenarios`, `skills`) is: `REACTOR_PACKAGE_ROOT`,
/// then a checkout next to the running binary, then the config directory (`~/.reactor/`),
/// where `reactor setup` seeds the scenarios.
pub fn find_bundled(sub: &str) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(root) = std::env::var_os("REACTOR_PACKAGE_ROOT").filter(|v| !v.is_empty()) {
        candidates.push(PathBuf::from(root).join(sub));
    }
    if let Ok(exe) = std::env::current_exe() {
        // target/{debug,release}/reactor-gui → the checkout it was built in.
        candidates.extend(exe.ancestors().skip(1).take(4).map(|a| a.join(sub)));
    }
    // Where `reactor setup` seeds the scenarios: the config directory, which
    // `REACTOR_CONFIG_DIR` moves (`Paths::from_env`).
    match std::env::var_os("REACTOR_CONFIG_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => candidates.push(PathBuf::from(dir).join(sub)),
        None => {
            if let Some(home) = std::env::var_os("HOME") {
                candidates.push(PathBuf::from(home).join(".reactor").join(sub));
            }
        }
    }
    candidates.into_iter().find(|p| p.is_dir())
}

/// What the settings choose about building a provider.
fn provider_options(settings: &Settings) -> reactor_agent::provider::Options {
    reactor_agent::provider::Options {
        prefix_one_hour: settings.cache.prefix_one_hour(),
    }
}

fn llm_for(
    spec: Option<&str>,
    thinking: Option<&str>,
    options: &reactor_agent::provider::Options,
) -> (AnyLlm, String, u64) {
    match spec {
        Some(spec) => match AnyLlm::from_spec_with(spec, thinking, options) {
            Ok((llm, provider)) => (llm, spec.to_string(), default_window(&provider)),
            Err(e) => (AnyLlm::Missing(e.to_string()), String::new(), 128_000),
        },
        None => (
            AnyLlm::Missing("no model chosen -- pick one beside Send, or set `defaultModel` in ~/.reactor/settings.json".into()),
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

        // The folder's case, else its unsaved state: nothing is written into a folder
        // that has not been saved as a case (ADR-0045).
        let case = match &opts.shared {
            Some(shared) => shared.case.clone(),
            None => Case::open_any(&opts.cwd, &paths.config_dir).map_err(|e| e.to_string())?,
        };
        if case.is_saved() {
            let _ = reactor_db::case::remember(&paths.config_dir, case.root());
        }
        let (store, session_id) = match &opts.resume {
            Some(id) => (
                Store::open(&case, id).map_err(|e| e.to_string())?,
                id.clone(),
            ),
            None => {
                // The case's next number: session 1, 2, 3 …
                let store = Store::create_next(&case).map_err(|e| e.to_string())?;
                let id = store.session().to_string();
                (store, id)
            }
        };
        let session_dir = store.dir().to_path_buf();
        // Activation is per session (ADR-0038): this file, inside the session, wins over
        // the machine's.
        let paths = paths.with_session_state(session_dir.join("activation.json"));

        let spec = opts
            .model
            .clone()
            .or_else(|| settings.default_model.clone())
            .or_else(|| std::env::var("REACTOR_MODEL").ok());
        let (llm, model, window) =
            llm_for(
                spec.as_deref(),
                settings.default_thinking.as_deref(),
                &provider_options(&settings),
            );
        let llm = Switchable::new(llm);
        let summarizer = Switchable::new(llm.current());

        // The embedding model the user chose, if any. A model that cannot be set up is said
        // so in the window, which still opens: search is then by keyword.
        let embedder = match &opts.shared {
            Some(shared) => shared.embedder.clone(),
            None => reactor_agent::embed::EmbedderSlot::default(),
        };
        let mut embed_problem = None;
        if opts.shared.is_none()
            && let Some(spec) = &settings.embedding.model
        {
            match reactor_agent::embed::from_spec(spec) {
                Ok(e) => embedder.set(Some(e)),
                Err(e) => embed_problem = Some(format!("embedding model: {e}")),
            }
        }
        // The servers are connected to once for the window: a later session shares them.
        let mcp = match &opts.shared {
            Some(shared) => shared.mcp.clone(),
            None => {
                let host = reactor_agent::mcp::McpHost::new(paths.clone());
                host.sync();
                host
            }
        };
        let mut cfg = AgentConfig::new(opts.cwd.clone(), paths.clone(), window);
        cfg.embedder = embedder.clone();
        cfg.mcp = Some(mcp.clone());
        let mut scenarios_problem = None;
        match find_bundled("prompts/scenarios") {
            Some(d) => cfg.scenarios_dir = d,
            // Said once, by the window's first session: without it the scenario list is
            // simply empty, which looks like there are none.
            None if opts.shared.is_none() => {
                scenarios_problem = Some(
                    "no scenarios found: run `reactor setup` to seed them into the config directory"
                        .to_string(),
                );
            }
            None => {}
        }
        cfg.skills_dir = find_bundled("skills");
        let (mut agent, mut events) = Agent::new(
            llm.clone(),
            // If the model named for summaries fails, the main one is asked instead.
            LlmSummarizer::new(summarizer.clone()).with_fallback(llm.clone()),
            store,
            Tools::standard(opts.cwd.clone()),
            cfg,
        );
        // The session may hand a subtask to a child agent (ADR-0062).
        agent.enable_children();

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
            embedder,
            mcp,
            embed_cancel: Mutex::new(None),
            case,
            session_id,
            session_dir,
            model: Mutex::new(model),
            thinking: Mutex::new(settings.default_thinking.clone()),
            ollama_levels: Arc::new(Mutex::new(None)),
            default_window: Mutex::new(window),
        };
        backend.sync_summarizer();
        backend.fetch_ollama_levels();
        for text in [embed_problem, scenarios_problem].into_iter().flatten() {
            let _ = backend.ui_tx.send(UiEvent::Notice {
                kind: "warning",
                text,
            });
        }
        Ok(backend)
    }

    // -- context documents (ADR-0046) ----------------------------------------------------------

    fn notice(tx: &Sender<UiEvent>, kind: &'static str, text: impl Into<String>) {
        let _ = tx.send(UiEvent::Notice {
            kind,
            text: text.into(),
        });
    }

    /// Import files, folders and URLs into the case's corpus, then make the vectors they
    /// need. Runs in the background; the outcome arrives as notices.
    pub fn import(&self, sources: Vec<String>) {
        let (case, tx) = (self.case.clone(), self.ui_tx.clone());
        let this = self.embed_job();
        self.runtime.spawn(async move {
            let items = match crate::import::plan(&case, &sources) {
                Ok(items) if !items.is_empty() => items,
                Ok(_) => return Self::notice(&tx, "warning", "import: nothing to import there"),
                Err(e) => return Self::notice(&tx, "error", format!("import: {e}")),
            };
            Self::notice(
                &tx,
                "info",
                format!("importing {} source(s) in the background…", items.len()),
            );
            let summary = {
                let case = case.clone();
                tokio::task::spawn_blocking(move || {
                    crate::import::run(&case, &items, &mut |_, _, _| {})
                })
                .await
            };
            let Ok(summary) = summary else {
                return Self::notice(&tx, "error", "import: stopped unexpectedly");
            };
            let mut text = format!(
                "imported {} document(s), {} passages; {} unchanged",
                summary.stored, summary.passages, summary.unchanged
            );
            if summary.replaced > 0 {
                text.push_str(&format!(
                    ", {} replaced an earlier version",
                    summary.replaced
                ));
            }
            for (source, why) in summary.failures.iter().take(5) {
                text.push_str(&format!("\n! {source}: {why}"));
            }
            if summary.failures.len() > 5 {
                text.push_str(&format!(
                    "\n… and {} more failed",
                    summary.failures.len() - 5
                ));
            }
            let kind = if summary.failures.is_empty() {
                "info"
            } else {
                "warning"
            };
            Self::notice(&tx, kind, text);
            if summary.stored > 0 {
                this.run().await;
            }
        });
    }

    /// What a background embedding run needs, owned.
    fn embed_job(&self) -> EmbedJob {
        EmbedJob {
            case: self.case.clone(),
            slot: self.embedder.clone(),
            tx: self.ui_tx.clone(),
            cancel: {
                let token = CancellationToken::new();
                *self.embed_cancel.lock().unwrap() = Some(token.clone());
                token
            },
        }
    }

    /// Make the vectors the passages are missing, in the background.
    pub fn embed(&self) {
        if self.embedder.get().is_none() {
            return Self::notice(
                &self.ui_tx,
                "warning",
                "no embedding model is chosen: /embedding provider/name",
            );
        }
        let job = self.embed_job();
        self.runtime.spawn(async move { job.run().await });
    }

    /// Make every vector again with the chosen model: stop a run that is going, forget the
    /// model's vectors, and embed all passages afresh, in the background. Until it has
    /// finished the passages cannot be searched by meaning. The passages' text goes to the
    /// provider again.
    pub fn reembed_all(&self) {
        if self.embedder.get().is_none() {
            return Self::notice(
                &self.ui_tx,
                "warning",
                "no embedding model is chosen: /embedding provider/name",
            );
        }
        if let Some(running) = self.embed_cancel.lock().unwrap().take() {
            running.cancel();
        }
        let job = self.embed_job();
        self.runtime.spawn(async move {
            let Some(embedder) = job.slot.get() else {
                return;
            };
            // The run that was stopped lets go of the case after its current batch.
            let mut lock = None;
            for _ in 0..100 {
                match job.case.lock_embedding() {
                    Ok(Some(l)) => {
                        lock = Some(l);
                        break;
                    }
                    Ok(None) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
                    Err(e) => {
                        return Self::notice(&job.tx, "error", format!("re-embedding: {e}"));
                    }
                }
            }
            let Some(lock) = lock else {
                return Self::notice(
                    &job.tx,
                    "warning",
                    "re-embedding: another embedding run holds this case",
                );
            };
            let name = embedder.name();
            let db = job.case.db();
            let total = reactor_db::corpus::counts(db).map_or(0, |(_, passages)| passages);
            let cleared = reactor_db::corpus::models(db)
                .unwrap_or_default()
                .into_iter()
                .filter(|m| m.name == name)
                .try_for_each(|m| reactor_db::corpus::clear_vectors(db, m.id));
            drop(lock);
            if let Err(e) = cleared {
                return Self::notice(&job.tx, "error", format!("re-embedding: {e}"));
            }
            let _ = job
                .tx
                .send(UiEvent::Embedding(Some(reactor_db::corpus::Progress {
                    embedded: 0,
                    total,
                })));
            let _ = job.tx.send(UiEvent::Notice {
                kind: "info",
                text: format!("re-embedding {total} passage(s) with {name}"),
            });
            job.run().await;
        });
    }

    /// Choose the embedding model (`None` to go back to keyword search), remember it as the
    /// default, and start making the vectors.
    pub fn set_embedding_model(&self, spec: Option<&str>) -> std::result::Result<(), String> {
        let embedder = match spec {
            Some(spec) => Some(reactor_agent::embed::from_spec(spec).map_err(|e| e.to_string())?),
            None => None,
        };
        self.save_embedding(spec, self.min_similarity())?;
        self.embedder.set(embedder);
        if spec.is_some() {
            self.embed();
        }
        Ok(())
    }

    /// Write the embedding settings: the model (or none) and the threshold, which are one
    /// object in `settings.json` and so are saved together.
    fn save_embedding(
        &self,
        model: Option<&str>,
        min_similarity: f32,
    ) -> std::result::Result<(), String> {
        let mut embedding = serde_json::Map::new();
        if let Some(m) = model {
            embedding.insert("model".into(), m.into());
        }
        if min_similarity > 0.0 {
            embedding.insert("minSimilarity".into(), f64::from(min_similarity).into());
        }
        let mut patch = serde_json::Map::new();
        patch.insert("embedding".into(), embedding.into());
        Settings::patch(&self.paths, patch)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// How close in meaning a passage must be to count as a match (0: no threshold).
    pub fn min_similarity(&self) -> f32 {
        Settings::load(&self.paths).embedding.threshold()
    }

    /// Set the threshold, keeping the chosen model. It applies to the next search, the
    /// agent's and the window's.
    pub fn set_min_similarity(&self, value: f32) -> std::result::Result<(), String> {
        let model = Settings::load(&self.paths).embedding.model;
        self.save_embedding(model.as_deref(), value.clamp(0.0, 1.0))
    }

    /// One paragraph on the corpus and its vectors, for `/embedding`.
    pub fn corpus_status(&self) -> String {
        let db = self.case.db();
        let (docs, chunks) = reactor_db::corpus::counts(db).unwrap_or((0, 0));
        let mut out = vec![format!("{docs} document(s), {chunks} passages.")];
        match self.embedder.get() {
            Some(e) => out.push(format!(
                "Embedding model: {}. The text of imported documents is sent to its provider to be embedded.",
                e.name()
            )),
            None => out.push(
                "No embedding model chosen: documents are searched by keyword only. Choose one with /embedding provider/name (openai, gemini, or ollama for a model on this machine).".into(),
            ),
        }
        for m in reactor_db::corpus::models(db).unwrap_or_default() {
            if let Ok(p) = reactor_db::corpus::progress(db, m.id) {
                out.push(format!(
                    "  {} ({} dimensions): {}/{} passages{}",
                    m.name,
                    m.dim,
                    p.embedded,
                    p.total,
                    if p.complete() {
                        ""
                    } else {
                        " — in progress or interrupted"
                    }
                ));
            }
        }
        out.join("\n")
    }

    /// Search the case's documents as the agent's tool does, off the window's thread: the
    /// query may have to be embedded by a provider over the network.
    pub fn search_docs(
        &self,
        text: String,
        limit: usize,
        mode: reactor_agent::embed::Mode,
    ) -> tokio::task::JoinHandle<std::result::Result<reactor_agent::embed::Found, String>> {
        let (case, embedder) = (self.case.clone(), self.embedder.get());
        let min_similarity = self.min_similarity();
        self.runtime.spawn(async move {
            reactor_agent::embed::search_by(
                &case,
                embedder.as_deref(),
                &text,
                limit,
                mode,
                min_similarity,
            )
            .await
            .map_err(|e| e.to_string())
        })
    }

    /// Which sentences of a passage are closest in meaning to a query, by the model whose
    /// vectors the search uses. Asks the provider; only where meaning can be searched.
    pub fn explain_hit(
        &self,
        query: String,
        passage: String,
    ) -> tokio::task::JoinHandle<std::result::Result<Vec<reactor_agent::embed::Sentence>, String>>
    {
        let embedder = self.embedder.get();
        let ready = self.meaning_status();
        self.runtime.spawn(async move {
            ready?;
            let embedder = embedder.ok_or("no embedding model is chosen")?;
            reactor_agent::embed::explain(embedder.as_ref(), &query, &passage)
                .await
                .map_err(|e| e.to_string())
        })
    }

    /// Whether the documents can be searched by meaning: the model's name if so, else why
    /// not.
    pub fn meaning_status(&self) -> std::result::Result<String, String> {
        let Some(e) = self.embedder.get() else {
            return Err("No embedding model is chosen (Settings → Documents).".into());
        };
        let want = e.name();
        match reactor_db::corpus::usable_model(self.case.db(), Some(&want)) {
            Ok(Some(m)) if m.name == want => Ok(m.name),
            _ => Err(format!(
                "{want} has not made vectors for every passage yet: the ring on the rail shows how far it is."
            )),
        }
    }

    /// Whether this folder is still an unsaved state rather than a case (ADR-0045).
    /// What a session opened next in this window shares with this one.
    pub fn shared(&self) -> Shared {
        Shared {
            case: self.case.clone(),
            embedder: self.embedder.clone(),
            mcp: self.mcp.clone(),
        }
    }

    pub fn is_unsaved(&self) -> bool {
        !self.case.is_saved()
    }

    /// Whether anything was said in any session of the case — what decides if closing an
    /// unsaved state is worth a question.
    pub fn has_work(&self) -> bool {
        reactor_db::sessions::list(self.case.db())
            .map(|sessions| sessions.iter().any(|s| s.first_prompt.is_some()))
            .unwrap_or(true)
    }

    /// The memory switches in effect (ADR-0047): the session's layer over the default.
    pub fn memory(&self) -> MemorySettings {
        let global = Settings::load(&self.paths).memory;
        self.agent.session_state().memory.over(&global)
    }

    /// Set one or both switches for this session.
    pub fn set_session_memory(&self, change: MemorySettings) -> std::result::Result<(), String> {
        let store = self.store();
        let mut store = store.lock().unwrap();
        let layer = change.over(&SessionState::load(&store).memory);
        reactor_agent::context::save_state(
            &mut store,
            reactor_agent::context::KEY_MEMORY,
            serde_json::to_value(layer).unwrap(),
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
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

    /// The child agents this session's agent is running.
    pub fn running_children(&self) -> Vec<reactor_agent::tools::agent::ChildInfo> {
        self.agent.running_children()
    }

    /// Stop the running child agent of `session`, or all of them. How many.
    pub fn stop_children(&self, session: Option<&str>) -> usize {
        self.agent.stop_children(session)
    }

    /// Whether typed input is waiting in the log.
    pub fn has_waiting(&self) -> bool {
        !self.agent.pending_inbox().is_empty()
    }

    /// Admit input typed while a turn runs; it is in the log when this returns.
    pub fn admit(&self, class: reactor_agent::inbox::Class, text: &str) -> std::result::Result<(), String> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let item = format!(
            "{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        self.agent
            .admit(&item, class, text)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Run the oldest waiting input as a turn. The outcome arrives as [`UiEvent::TurnEnded`].
    pub fn prompt_waiting(&self) {
        let cancel = CancellationToken::new();
        *self.running.lock().unwrap() = Some(cancel.clone());
        let (agent, tx) = (self.agent.clone(), self.ui_tx.clone());
        self.runtime.spawn(async move {
            match agent.prompt_pending(cancel).await {
                Some(result) => {
                    let _ = tx.send(UiEvent::TurnEnded(result.map_err(|e| e.to_string())));
                }
                None => {
                    let _ = tx.send(UiEvent::TurnEnded(Ok(Default::default())));
                }
            }
        });
    }

    /// Take back the latest waiting input; its text.
    pub fn withdraw_latest(&self) -> Option<String> {
        self.agent.withdraw(None).ok().flatten()
    }

    /// Take back everything waiting, oldest first.
    pub fn withdraw_all(&self) -> Vec<String> {
        self.agent.withdraw_all().unwrap_or_default()
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
        let thinking = self.thinking();
        let options = provider_options(&Settings::load(&self.paths));
        let (llm, provider) = AnyLlm::from_spec_with(spec, thinking.as_deref(), &options)
            .map_err(|e| e.to_string())?;
        self.llm.set(llm);
        *self.model.lock().unwrap() = spec.to_string();
        *self.default_window.lock().unwrap() = default_window(&provider);
        self.sync_summarizer();
        self.fetch_ollama_levels();
        Ok(())
    }

    /// The session's thinking level, `None` when the model decides.
    pub fn thinking(&self) -> Option<String> {
        self.thinking.lock().unwrap().clone()
    }

    /// The levels the current model takes, in menu order (`Default` is implied). For an
    /// Ollama model they are what the server said for it; until it has, none are offered.
    pub fn thinking_levels(&self) -> Vec<String> {
        let model = self.model();
        let (provider, name) = model.split_once('/').unwrap_or_default();
        if provider == "ollama" {
            return match &*self.ollama_levels.lock().unwrap() {
                Some((spec, levels)) if *spec == model => levels.clone(),
                _ => Vec::new(),
            };
        }
        reactor_agent::provider::thinking_levels(provider, name)
            .iter()
            .map(|l| l.to_string())
            .collect()
    }

    /// Ask the Ollama server which thinking levels the model in use takes. Returns at once;
    /// the menu is redrawn when the answer is in.
    fn fetch_ollama_levels(&self) {
        let model = self.model();
        let Some(name) = model.strip_prefix("ollama/").map(str::to_string) else {
            return;
        };
        let (slot, tx) = (self.ollama_levels.clone(), self.ui_tx.clone());
        self.runtime.spawn(async move {
            let levels = reactor_agent::connection::ollama_thinking_levels(&name)
                .await
                .unwrap_or_default();
            *slot.lock().unwrap() = Some((model, levels));
            let _ = tx.send(UiEvent::Redraw);
        });
    }

    /// Choose the thinking level, taking effect on the next call. `None` is the model's own.
    pub fn set_thinking(&self, level: Option<&str>) -> std::result::Result<(), String> {
        if let Some(l) = level
            && !self.thinking_levels().iter().any(|v| v == l)
        {
            let takes = self.thinking_levels();
            return Err(if takes.is_empty() {
                "this model has no thinking levels to set".to_string()
            } else {
                format!("this model takes {}", takes.join(", "))
            });
        }
        *self.thinking.lock().unwrap() = level.map(str::to_string);
        let model = self.model();
        if !model.is_empty() {
            self.set_model(&model)?;
        }
        Ok(())
    }

    /// The summarizer follows the session's model unless a `summarizer` setting names another.
    pub fn sync_summarizer(&self) {
        let (global, session) = self.agent.context_layers();
        let spec = session.over(&global).summarizer;
        let options = provider_options(&Settings::load(&self.paths));
        match spec
            .as_deref()
            .map(|s| AnyLlm::from_spec_with(s, None, &options))
        {
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
                // What the request is made of, for the usage bar. If it cannot be taken
                // apart the bar is simply not drawn; the numbers above it still are.
                let preview = agent.context_preview().await.ok();
                let usage = preview.as_ref().map(|p| p.by_kind()).unwrap_or_default();
                let warnings = preview.map(|p| p.warnings).unwrap_or_default();
                let _ = tx.send(UiEvent::Context(context_view(
                    &agent, model, window, m.real, usage, warnings,
                )));
            }
        });
    }

    pub fn preview(&self, mode: Mode) {
        let (agent, tx) = (self.agent.clone(), self.ui_tx.clone());
        self.runtime.spawn(async move {
            let _ = tx.send(UiEvent::Preview(
                agent
                    .preview(mode)
                    .await
                    .map(|p| PlanView::of(&p, mode))
                    .map_err(|e| e.to_string()),
            ));
        });
    }

    /// Ask for the request as it would be sent now, in pieces.
    pub fn inspect(&self) {
        let (agent, tx) = (self.agent.clone(), self.ui_tx.clone());
        self.runtime.spawn(async move {
            let _ = tx.send(UiEvent::Inspect(
                agent.context_preview().await.map_err(|e| e.to_string()),
            ));
        });
    }

    pub fn reduce_now(&self, mode: Mode, focus: Option<String>) {
        let (agent, tx) = (self.agent.clone(), self.ui_tx.clone());
        self.runtime.spawn(async move {
            let _ = tx.send(UiEvent::Reduced(
                agent
                    .reduce_now(mode, focus.as_deref())
                    .await
                    .map_err(|e| e.to_string()),
            ));
        });
        self.refresh_context();
    }

    /// Hand the agent a text. It is in the context from the next request on.
    pub fn attach(
        &self,
        attachment: reactor_agent::entry::Attachment,
    ) -> std::result::Result<EntryId, String> {
        let id = self.agent.attach(attachment).map_err(|e| e.to_string())?;
        self.refresh_context();
        Ok(id)
    }

    /// Take an attachment out of the context again.
    pub fn detach(&self, attachment: EntryId) -> std::result::Result<(), String> {
        self.agent.detach(attachment).map_err(|e| e.to_string())?;
        self.refresh_context();
        Ok(())
    }

    /// What the agent has been given and still holds.
    pub fn attachments(&self) -> Vec<(EntryId, reactor_agent::entry::Attachment)> {
        self.agent.attachments()
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
        self.agent
            .set_session_context(&layer.over(&current))
            .map_err(|e| e.to_string())?;
        self.sync_summarizer();
        self.refresh_context();
        Ok(())
    }

    /// The two stored layers of the context settings: (the default's, this session's).
    pub fn context_layers(&self) -> (ContextSettings, ContextSettings) {
        self.agent.context_layers()
    }

    /// Store exactly this as the default's layer (`global`) or this session's.
    pub fn replace_context_layer(
        &self,
        global: bool,
        layer: ContextSettings,
    ) -> std::result::Result<(), String> {
        if global {
            self.agent
                .set_global_context(layer)
                .map_err(|e| e.to_string())?;
        } else {
            self.agent
                .set_session_context(&layer)
                .map_err(|e| e.to_string())?;
        }
        self.sync_summarizer();
        self.refresh_context();
        Ok(())
    }

    pub fn inherit_context(&self) -> std::result::Result<(), String> {
        self.agent
            .set_session_context(&ContextSettings::default())
            .map_err(|e| e.to_string())?;
        self.sync_summarizer();
        self.refresh_context();
        Ok(())
    }

    /// "Make this the default": this session's effective layer becomes the global one.
    pub fn make_context_default(&self) -> std::result::Result<(), String> {
        let (global, session) = self.agent.context_layers();
        self.agent
            .set_global_context(session.over(&global))
            .map_err(|e| e.to_string())?;
        self.agent
            .set_session_context(&ContextSettings::default())
            .map_err(|e| e.to_string())?;
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
        self.store()
            .lock()
            .unwrap()
            .set_head(entry)
            .map_err(|e| e.to_string())
    }

    pub fn session_state(&self) -> SessionState {
        self.agent.session_state()
    }

    // -- manifest and reporting settings -------------------------------------------------------

    /// Change the global manifest or reporting settings: read, edited, saved. The agent reads
    /// them afresh on every request, so the change is in force at once.
    pub fn edit_settings(
        &self,
        edit: impl FnOnce(&mut Settings),
    ) -> std::result::Result<(), String> {
        let mut settings = Settings::load(&self.paths);
        edit(&mut settings);
        settings.save(&self.paths).map_err(|e| e.to_string())
    }

    // -- identities ------------------------------------------------------------------------------

    /// The identities as the settings hold them: the default, and the saved ones.
    pub fn identity_settings(&self) -> reactor_context::settings::IdentitySettings {
        Settings::load(&self.paths).identity
    }

    /// Change the identity settings and write them: all or nothing.
    fn change_identities(
        &self,
        change: impl FnOnce(&mut reactor_context::settings::IdentitySettings) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut settings = Settings::load(&self.paths);
        change(&mut settings.identity)?;
        settings.save(&self.paths).map_err(|e| e.to_string())
    }

    /// Save a user identity (`old` is the name it had, when renaming or editing).
    pub fn save_identity(&self, old: Option<&str>, name: &str, text: &str) -> Result<(), String> {
        self.change_identities(|c| reactor_context::identity::save_user(c, old, name, text))
    }

    pub fn delete_identity(&self, name: &str) -> Result<(), String> {
        self.change_identities(|c| reactor_context::identity::delete_user(c, name))
    }

    pub fn set_default_identity(&self, name: &str) -> Result<(), String> {
        self.change_identities(|c| reactor_context::identity::set_default(c, name))
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
    let on_branch: std::collections::HashSet<EntryId> =
        store.branch().iter().map(|e| e.id).collect();
    let mut children: std::collections::HashMap<EntryId, Vec<EntryId>> =
        std::collections::HashMap::new();
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
                    let text = blocks
                        .iter()
                        .find_map(|b| match b {
                            Block::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .unwrap_or("");
                    format!("agent: {}", first_words(text, 60))
                }
                Kind::Reduction(r) => {
                    format!("context reduced ({:?}, {} entries)", r.mode, r.covers.len())
                }
                Kind::Label { text } => text.clone(),
                _ => String::new(),
            };
            rows.push(TreeRow {
                id,
                depth,
                label,
                on_branch: on_branch.contains(&id),
                is_head: id == head,
                can_switch: final_reply,
            });
        }
        // Children in log order; the first child continues at this depth, the others fork.
        for (i, k) in kids.iter().enumerate().rev() {
            stack.push((*k, if forks && i > 0 { depth + 1 } else { depth }));
        }
    }
    rows
}

pub(crate) fn first_words(text: &str, n: usize) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() <= n {
        line.to_string()
    } else {
        format!("{}…", line.chars().take(n - 1).collect::<String>())
    }
}

/// What the usage bar and the Budget chip show.
#[derive(Debug, Clone)]
pub struct ContextView {
    pub model: String,
    /// Tokens in the next request: the provider's last count plus an estimate of what has come
    /// since, else an estimate at the ratio learned from what it reported.
    pub used: u64,
    pub window: u64,
    pub hard: u64,
    pub trigger: u64,
    pub mode: String,
    pub pct: f64,
    pub keep: f64,
    pub mask_target: f64,
    pub protect: f64,
    /// Whether a summary is checked for what it left out.
    pub verify: bool,
    pub reserve: u64,
    pub summarizer: Option<String>,
    /// Things the person should know about the request: the fixed part of it being most of
    /// the window, say.
    pub warnings: Vec<String>,
    /// Where each setting comes from: this session, or the default.
    pub origins: Vec<(String, Origin)>,
    pub reductions: Vec<reactor_agent::agent::ReductionInfo>,
    /// Estimated tokens by kind, in the usage bar's order.
    pub usage: Vec<(reactor_agent::inspect::Kind, u64)>,
}

impl ContextView {
    pub fn percent(&self) -> u64 {
        (self.used * 100)
            .checked_div(self.hard)
            .map_or(0, |p| p.min(999))
    }
}

pub fn context_view(
    agent: &GuiAgent,
    model: String,
    _default_window: u64,
    used: u64,
    usage: Vec<(reactor_agent::inspect::Kind, u64)>,
    warnings: Vec<String>,
) -> ContextView {
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
        mask_target: b.mask_target,
        protect: b.protect,
        verify: b.verify,
        reserve: b.reserve,
        summarizer: session.over(&global).summarizer,
        origins: session
            .origins()
            .into_iter()
            .map(|(k, o)| (k.to_string(), o))
            .collect(),
        reductions: agent.reductions(),
        usage,
        warnings,
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

/// The sessions of `cwd`'s case (or of its unsaved state), newest first — the picker's
/// feed. A folder that is neither has none, and asking does not make it one.
pub fn list_sessions(paths: &Paths, cwd: &Path) -> Vec<SessionSummary> {
    if !Case::is_case(cwd) && !Case::has_unsaved(cwd, &paths.config_dir) {
        return vec![];
    }
    let Ok(case) = Case::open_any(cwd, &paths.config_dir) else {
        return vec![];
    };
    reactor_db::sessions::list(case.db())
        .unwrap_or_default()
        .into_iter()
        .map(|s| SessionSummary {
            id: s.id,
            first_prompt: s.first_prompt.map(|p| first_words(&p, 80)),
            created_at: s.created_at,
            entries: s.entries,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    pub id: String,
    pub first_prompt: Option<String>,
    /// RFC 3339 UTC.
    pub created_at: String,
    pub entries: usize,
}

/// One background embedding run, owned so that it can outlive the call that started it.
struct EmbedJob {
    case: Case,
    slot: reactor_agent::embed::EmbedderSlot,
    tx: Sender<UiEvent>,
    cancel: CancellationToken,
}

impl EmbedJob {
    async fn run(self) {
        let Some(embedder) = self.slot.get() else {
            return;
        };
        let tx = self.tx.clone();
        let result = reactor_agent::embed::embed_pending(
            &self.case,
            embedder.as_ref(),
            &self.cancel,
            &mut |p| {
                let _ = tx.send(UiEvent::Embedding(Some(p)));
            },
        )
        .await;
        let _ = self.tx.send(UiEvent::Embedding(None));
        let (kind, text) = match result {
            Ok(done) if done.now == 0 => return,
            Ok(done) => (
                "info",
                format!(
                    "embedded {} passage(s) with {}: {}/{} done",
                    done.now,
                    embedder.name(),
                    done.progress.embedded,
                    done.progress.total
                ),
            ),
            Err(e) => (
                "warning",
                format!("embedding stopped: {e}. It continues where it left off next time."),
            ),
        };
        let _ = self.tx.send(UiEvent::Notice { kind, text });
    }
}
