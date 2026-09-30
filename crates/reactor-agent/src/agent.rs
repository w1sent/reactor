//! The loop.
//!
//! One turn is: append the person's message, then repeat — fit the context to the
//! budget, ask the model, record its reply, run the tools it called, record their
//! results — until it answers without calling one. Because this loop owns the message
//! list, three things that were fights with a host stop being fights:
//!
//! - **Reduction is proactive and intrinsic.** The budget is checked before every
//!   request *and after every tool result* ([ADR-0037]), so a single dump that would
//!   overflow the window inside one turn is reduced before the next request goes out.
//!   There is no turn to park and nothing to restart: no synthetic "continue" message
//!   is ever sent.
//! - **Two loop invariants** survive from ADR-0025 as rules rather than an extension's
//!   edge cases: reductions that do not get the context to fit stop the turn after a
//!   budget of attempts, and a *failed* reduction stops it at once — the context did
//!   not shrink, so the next request would overflow again.
//! - **The log stays valid whatever happens.** Every tool call in a recorded reply
//!   gets a recorded result, even if the turn is cancelled or a reduction fails
//!   mid-batch, so a session can always be resumed and its history always replays.
//!
//! [ADR-0037]: ../../../docs/adr/0037-context-reduction-is-one-budget-manager.md

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use reactor_context::settings::Settings;
use reactor_context::{identity, manifest, reporting, scenario};
use reactor_core::Paths;
use reactor_core::probe::Status;
use serde_json::Value;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;

use crate::budget::{self, BudgetConfig, Plan, Summarizer, context_tokens, needs_reduction, plan};
use crate::context::{
    Item, KEY_IDENTITY, KEY_MANIFEST, KEY_REPORTING, KEY_SCENARIO, Msg, SessionState, estimate_tokens, project, save_state,
};
use crate::entry::{EntryId, Kind, Mode, Trigger, Usage};
use crate::error::{Error, Result};
use crate::llm::{Delta, Llm, LlmRequest};
use crate::prompt::{self, RegistryView};
use crate::skills::{self, Skill};
use crate::store::Store;
use crate::tools::{ToolCtx, ToolOutput, Tools};
use crate::truncate;

/// What a frontend watches. Sent as things happen; never required for correctness.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Appended(EntryId),
    Text(String),
    Thinking(String),
    ToolCallStarted { name: String },
    ToolStart { id: String, name: String, args: Value },
    /// Live tool output, as it arrives.
    ToolOutput { id: String, chunk: String },
    ToolEnd { id: String, entry: EntryId, is_error: bool },
    Usage(Usage),
    Reduced { entry: EntryId, mode: Mode, trigger: Trigger, before_tokens: u64, after_tokens: u64 },
    /// Something a person should read that is not an error.
    Notice(String),
    Finished,
}

#[derive(Clone)]
pub struct AgentConfig {
    pub cwd: PathBuf,
    pub paths: Paths,
    pub scenarios_dir: PathBuf,
    /// REactor's own authored skills (a checkout's `skills/`).
    pub skills_dir: Option<PathBuf>,
    pub base_prompt: String,
    pub budget: BudgetConfig,
    /// How many reductions in a row may fail to make the context fit before the turn stops.
    pub max_reductions_in_a_row: usize,
    /// A backstop on model round trips in one turn.
    pub max_rounds: usize,
    pub max_tokens: Option<u64>,
    /// What the catalogue says now. Replaceable, so the loop can be tested without probing.
    pub registry: Arc<dyn Fn() -> RegistryView + Send + Sync>,
}

impl AgentConfig {
    pub fn new(cwd: PathBuf, paths: Paths, window: u64) -> Self {
        let registry = registry_from_core(paths.clone());
        AgentConfig {
            cwd,
            paths,
            scenarios_dir: PathBuf::from("prompts/scenarios"),
            skills_dir: None,
            base_prompt: prompt::DEFAULT_BASE.to_string(),
            budget: BudgetConfig::new(window),
            max_reductions_in_a_row: 3,
            max_rounds: 200,
            max_tokens: None,
            registry,
        }
    }
}

/// The registry as `reactor-core` reports it, with the normal probe cache.
pub fn registry_from_core(paths: Paths) -> Arc<dyn Fn() -> RegistryView + Send + Sync> {
    Arc::new(move || match reactor_core::commands::registry(&paths, Default::default()) {
        Ok(done) => RegistryView {
            block: done.report.block.clone(),
            skill_dirs: done.report.skill_paths.clone(),
            usable: done.report.tools.iter().filter(|t| t.active && t.status == Status::Present).map(|t| t.id.clone()).collect(),
        },
        Err(_) => RegistryView::default(),
    })
}

/// How a turn ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    pub rounds: usize,
    pub reductions: usize,
    pub tool_calls: usize,
    /// Level-2 reporting reverted the turn this many times.
    pub reverts: usize,
}

/// What a session command did, for the frontend to show and act on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommandResult {
    pub notices: Vec<reactor_context::Notice>,
    /// Text that should now be sent to the model as a turn (a scenario's opening briefing).
    pub trigger_turn: Option<String>,
}

pub struct Agent<L: Llm, S: Summarizer> {
    llm: L,
    summarizer: S,
    store: Arc<Mutex<Store>>,
    tools: Tools,
    cfg: AgentConfig,
    events: UnboundedSender<Event>,
    tracker: Mutex<reporting::Tracker>,
    /// Real tokens per estimated token, learned from what the provider reports.
    ratio: Mutex<f64>,
    authored_skills: Vec<Skill>,
}

impl<L: Llm, S: Summarizer> Agent<L, S> {
    pub fn new(llm: L, summarizer: S, store: Store, tools: Tools, cfg: AgentConfig) -> (Self, UnboundedReceiver<Event>) {
        let (events, rx) = unbounded_channel();
        let authored_skills = cfg.skills_dir.as_deref().map(skills::discover).unwrap_or_default();
        (
            Agent {
                llm,
                summarizer,
                store: Arc::new(Mutex::new(store)),
                tools,
                cfg,
                events,
                tracker: Mutex::new(reporting::Tracker::new()),
                ratio: Mutex::new(1.0),
                authored_skills,
            },
            rx,
        )
    }

    pub fn store(&self) -> Arc<Mutex<Store>> {
        self.store.clone()
    }

    pub fn config(&self) -> &AgentConfig {
        &self.cfg
    }

    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    fn append(&self, kind: Kind) -> Result<EntryId> {
        let id = self.store.lock().unwrap().append(kind)?;
        self.emit(Event::Appended(id));
        Ok(id)
    }

    fn settings(&self) -> Settings {
        Settings::load(&self.cfg.paths)
    }

    // -- building a request --------------------------------------------------------------

    /// The request as it would be sent now, and what it costs.
    async fn build(&self) -> Result<(LlmRequest, u64, Vec<Item>)> {
        let registry = self.cfg.registry.clone();
        let view = tokio::task::spawn_blocking(move || registry()).await.map_err(|e| Error::Store(e.to_string()))?;
        let settings = self.settings();
        let (state, items) = {
            let store = self.store.lock().unwrap();
            (SessionState::load(&store), project(&store))
        };
        let offered = skills::offered(&self.authored_skills, &view.usable, &view.skill_dirs);
        let system = prompt::system_prompt(&prompt::Inputs {
            base: &self.cfg.base_prompt,
            settings: &settings,
            state: &state,
            registry: &view,
            skills: &offered,
            scenarios_dir: &self.cfg.scenarios_dir,
        });
        let tools = self.tools.specs(&state);
        let fixed = estimate_tokens(&system)
            + tools.iter().map(|t| estimate_tokens(&t.name) + estimate_tokens(&t.description) + estimate_tokens(&t.parameters.to_string())).sum::<u64>();
        let mut messages: Vec<Msg> = items.iter().map(|i| i.msg.clone()).collect();

        // Level 1 reporting: a reminder on every model call until the folder changes.
        // Ephemeral by design -- never stored, so it cannot pile up in a long session.
        if let Some(nag) = self.tracker.lock().unwrap().nag(&state.reporting, &settings.reporting) {
            messages.push(Msg::User { text: nag });
        }
        Ok((LlmRequest { system, messages, tools, max_tokens: self.cfg.max_tokens }, fixed, items))
    }

    // -- the budget ------------------------------------------------------------------------

    fn scaled_budget(&self) -> BudgetConfig {
        self.cfg.budget.scaled(*self.ratio.lock().unwrap())
    }

    /// A person asking to reduce wants *more* gone than the budget would take: the keep
    /// window shrinks to the latest work (the last message and what followed it).
    fn manual_budget(&self) -> BudgetConfig {
        BudgetConfig { keep: 0.0, ..self.scaled_budget() }
    }

    /// What a reduction would do right now, without doing it.
    pub async fn preview(&self, mode: Mode) -> Result<Plan> {
        let (_, fixed, items) = self.build().await?;
        plan(fixed, &items, &self.manual_budget(), mode).map_err(|n| Error::Reduction(n.to_string()))
    }

    /// Reduce now, at a person's request. A manual reduction means *summarize* in every
    /// mode but `fade` (a manual fade exists for a burst of dumps that should just go).
    pub async fn reduce_now(&self, mode: Mode) -> Result<EntryId> {
        let (_, fixed, items) = self.build().await?;
        let p = plan(fixed, &items, &self.manual_budget(), mode).map_err(|n| Error::Reduction(n.to_string()))?;
        self.carry_out(&items, &p, Trigger::Manual).await
    }

    async fn carry_out(&self, items: &[Item], p: &Plan, trigger: Trigger) -> Result<EntryId> {
        let prepared = budget::prepare(&self.store.lock().unwrap(), items, p, trigger, self.cfg.budget.summary_tokens)?;
        let summary = match &prepared.request {
            Some(req) => Some(budget::summarize(&self.summarizer, req.clone()).await?),
            None => None,
        };
        let id = budget::commit(&mut self.store.lock().unwrap(), prepared, summary)?;
        if let Some(Kind::Reduction(r)) = self.store.lock().unwrap().get(id).map(|e| e.kind.clone()) {
            self.emit(Event::Reduced { entry: id, mode: r.mode, trigger, before_tokens: r.before_tokens, after_tokens: r.after_tokens });
        }
        self.emit(Event::Appended(id));
        Ok(id)
    }

    /// Reduce until the context fits. Called before every request and after every tool
    /// result. `in_a_row` counts reductions that did not by themselves get it to fit.
    async fn ensure_fits(&self, in_a_row: &mut usize, outcome: &mut Outcome) -> Result<()> {
        loop {
            let (_, fixed, items) = self.build().await?;
            let cfg = self.scaled_budget();
            if !needs_reduction(fixed, &items, &cfg) {
                *in_a_row = 0;
                return Ok(());
            }
            let p = match plan(fixed, &items, &cfg, self.cfg.budget.mode) {
                Ok(p) => p,
                Err(nothing) => {
                    // Over the trigger with nothing older to reduce: fine while there is
                    // still room, fatal once there is not.
                    let used = context_tokens(fixed, &items);
                    if used > cfg.hard() {
                        return Err(Error::Reduction(format!("the context is full ({used} of {} tokens) and {nothing}", cfg.hard())));
                    }
                    *in_a_row = 0;
                    return Ok(());
                }
            };
            if *in_a_row >= self.cfg.max_reductions_in_a_row {
                return Err(Error::ReductionBudget { attempts: *in_a_row });
            }
            *in_a_row += 1;
            self.carry_out(&items, &p, Trigger::Budget).await?;
            outcome.reductions += 1;
        }
    }

    // -- one turn ------------------------------------------------------------------------------

    /// Run one turn on `text`.
    pub async fn prompt(&self, text: &str, cancel: CancellationToken) -> Result<Outcome> {
        let mut outcome = Outcome::default();
        let mut in_a_row = 0usize;
        let mut next: String = text.to_string();

        'turn: loop {
            let settings = self.settings();
            {
                let state = SessionState::load(&self.store.lock().unwrap());
                self.tracker.lock().unwrap().before_agent_start(&state.reporting, &settings.reporting, &next);
            }
            self.append(Kind::User { text: next.clone() })?;

            loop {
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                self.ensure_fits(&mut in_a_row, &mut outcome).await?;
                let (req, _, _) = self.build().await?;
                let estimated: u64 = context_tokens(0, &project(&self.store.lock().unwrap())) + estimate_tokens(&req.system);

                let mut on_delta = |d: Delta| {
                    self.emit(match d {
                        Delta::Text(t) => Event::Text(t),
                        Delta::Thinking(t) => Event::Thinking(t),
                        Delta::ToolCall { name } => Event::ToolCallStarted { name },
                    })
                };
                let reply = tokio::select! {
                    _ = cancel.cancelled() => return Err(Error::Cancelled),
                    r = self.llm.complete(req, &mut on_delta) => r?,
                };
                outcome.rounds += 1;
                if let Some(u) = reply.usage {
                    self.emit(Event::Usage(u));
                    if u.input_tokens > 0 && estimated > 0 {
                        // Fold in what the provider actually counted.
                        let observed = u.input_tokens as f64 / estimated as f64;
                        let mut r = self.ratio.lock().unwrap();
                        *r = (0.7 * *r + 0.3 * observed).clamp(0.25, 4.0);
                    }
                }
                let calls: Vec<(String, String, Value)> = reply.tool_calls().map(|(i, n, a)| (i.to_string(), n.to_string(), a.clone())).collect();
                self.append(Kind::Assistant { blocks: reply.blocks.clone(), model: Some(self.llm.name()), usage: reply.usage, stop: reply.stop.clone() })?;

                if calls.is_empty() {
                    // The turn has settled. Level-2 reporting may send it back.
                    let (state, settings) = (SessionState::load(&self.store.lock().unwrap()), self.settings());
                    let verdict = self.tracker.lock().unwrap().settled(&state.reporting, &settings.reporting);
                    match verdict {
                        reporting::Settled::Revert => {
                            outcome.reverts += 1;
                            // Fork from the last user message: the reverted turn stays in
                            // the log as a dead branch, recoverable like everything else.
                            {
                                let mut store = self.store.lock().unwrap();
                                let last_user = store.branch().iter().rev().find(|e| matches!(e.kind, Kind::User { .. })).map(|e| e.id);
                                if let Some(id) = last_user {
                                    store.set_head(id)?;
                                }
                            }
                            next = self.tracker.lock().unwrap().demand(&settings.reporting);
                            continue 'turn;
                        }
                        reporting::Settled::GaveUp(n) => self.emit(Event::Notice(n.message)),
                        reporting::Settled::Nothing => {}
                    }
                    break 'turn;
                }

                if outcome.rounds > self.cfg.max_rounds {
                    self.fill_results(&calls, "not run: the turn hit its round limit")?;
                    return Err(Error::Model(format!("stopped after {} model round trips in one turn", self.cfg.max_rounds)));
                }
                outcome.tool_calls += calls.len();
                self.run_tools(&calls, &cancel, &mut in_a_row, &mut outcome).await?;
            }
        }
        self.emit(Event::Finished);
        Ok(outcome)
    }

    /// Give every call in `calls` a recorded result, so the log stays replayable.
    fn fill_results(&self, calls: &[(String, String, Value)], why: &str) -> Result<()> {
        for (id, name, _) in calls {
            self.append(Kind::ToolResult { call_id: id.clone(), name: name.clone(), content: why.to_string(), is_error: true, blob: None })?;
        }
        Ok(())
    }

    async fn run_tools(&self, calls: &[(String, String, Value)], cancel: &CancellationToken, in_a_row: &mut usize, outcome: &mut Outcome) -> Result<()> {
        for (i, (id, name, args)) in calls.iter().enumerate() {
            if cancel.is_cancelled() {
                self.fill_results(&calls[i..], "not run: the turn was cancelled")?;
                return Err(Error::Cancelled);
            }
            self.emit(Event::ToolStart { id: id.clone(), name: name.clone(), args: args.clone() });

            let events = self.events.clone();
            let call_id = id.clone();
            let ctx = ToolCtx {
                cwd: self.cfg.cwd.clone(),
                cancel: cancel.clone(),
                emit: Arc::new(move |chunk: &str| {
                    let _ = events.send(Event::ToolOutput { id: call_id.clone(), chunk: chunk.to_string() });
                }),
                store: self.store.clone(),
                paths: self.cfg.paths.clone(),
                scenarios_dir: self.cfg.scenarios_dir.clone(),
            };
            let out = match self.tools.find(name) {
                Some(tool) => tool.call(args.clone(), &ctx).await,
                None => ToolOutput::err(format!("unknown tool `{name}`")),
            };
            let is_error = out.is_error;
            let kind = self.finalize(id, name, out)?;
            let entry = self.append(kind)?;
            self.emit(Event::ToolEnd { id: id.clone(), entry, is_error });

            // Reporting asks the filesystem, after every tool call, whether anything was written up.
            {
                let (state, settings) = (SessionState::load(&self.store.lock().unwrap()), self.settings());
                let snap = reporting::take_snapshot(&self.cfg.cwd.join(&settings.reporting.folder));
                self.tracker.lock().unwrap().tool_end(&state.reporting, snap);
            }

            // The boundary check: a dump can overflow the window inside one turn.
            if let Err(e) = self.ensure_fits(in_a_row, outcome).await {
                self.fill_results(&calls[i + 1..], "not run: context reduction failed, so the turn stopped")?;
                return Err(e);
            }
        }
        Ok(())
    }

    /// Apply the truncation policy: the model sees a bounded view, and the whole output
    /// is kept and addressable.
    fn finalize(&self, call_id: &str, name: &str, out: ToolOutput) -> Result<Kind> {
        let total = match &out.full {
            Some(p) => std::fs::metadata(p).map(|m| m.len()).unwrap_or(out.text.len() as u64),
            None => out.text.len() as u64,
        };
        if !truncate::needs_cut(total) {
            let text = out.text.replace(truncate::CUT, "");
            return Ok(Kind::ToolResult { call_id: call_id.into(), name: name.into(), content: text, is_error: out.is_error, blob: None });
        }
        let mut store = self.store.lock().unwrap();
        let entry_id = store.len() as EntryId;
        let label = format!("{name}-{entry_id}");
        let blob = match &out.full {
            Some(path) => store.adopt_blob(&label, path)?,
            None => store.write_blob(&label, out.text.as_bytes())?,
        };
        let content = truncate::view(&out.text, total, entry_id);
        Ok(Kind::ToolResult { call_id: call_id.into(), name: name.into(), content, is_error: out.is_error, blob: Some(blob) })
    }

    // -- session commands ---------------------------------------------------------------------------

    /// `/goal`, `/guidelines`, `/manifest`, `/frame`, `/identity`, `/report`,
    /// `/reactor-scenario`: what a frontend's command palette calls. State changes are
    /// stored in the session; the notices are for the person.
    pub fn command(&self, name: &str, args: &str) -> Result<CommandResult> {
        let mut settings = self.settings();
        let mut store = self.store.lock().unwrap();
        let state = SessionState::load(&store);
        let mut result = CommandResult::default();

        match name {
            "goal" | "guidelines" | "manifest" | "frame" => {
                let mut m = state.manifest.clone();
                let Some(e) = manifest::command(&mut m, &settings.manifest, name, args) else { unreachable!() };
                if e.persist {
                    save_state(&mut store, KEY_MANIFEST, serde_json::to_value(&m).unwrap())?;
                }
                result.notices = e.notices;
            }
            "identity" => {
                let mut s = state.identity.clone();
                let e = identity::command(&mut s, &mut settings.identity, args);
                if e.persist {
                    save_state(&mut store, KEY_IDENTITY, serde_json::to_value(&s).unwrap())?;
                }
                if e.save_settings {
                    settings.save(&self.cfg.paths)?;
                }
                result.notices = e.notices;
            }
            "report" => {
                let mut s = state.reporting;
                let e = reporting::command(&mut s, &mut self.tracker.lock().unwrap(), &mut settings.reporting, args);
                if e.persist {
                    save_state(&mut store, KEY_REPORTING, serde_json::to_value(s).unwrap())?;
                }
                if e.save_folder.is_some() {
                    settings.save(&self.cfg.paths)?;
                }
                result.notices = e.notices;
            }
            "reactor-scenario" => {
                let mut sc = scenario::Scenario { dir: &self.cfg.scenarios_dir, state: state.scenario.clone() };
                let e = sc.command(args);
                match &e.persist {
                    scenario::Persist::Nothing => {}
                    scenario::Persist::Set(s) => {
                        save_state(&mut store, KEY_SCENARIO, serde_json::to_value(s).unwrap())?;
                    }
                    scenario::Persist::Clear => {
                        save_state(&mut store, KEY_SCENARIO, Value::Null)?;
                    }
                }
                drop(store);
                // Additive and advisory: a phase starts whether or not the toolset can be enabled.
                for t in &e.activate_toolsets {
                    let _ = reactor_core::commands::set_activation(&self.cfg.paths, std::slice::from_ref(t), false, Some(true));
                }
                result.notices = e.notices;
                result.trigger_turn = e.messages.into_iter().find(|m| m.trigger_turn).map(|m| m.content);
                return Ok(result);
            }
            other => return Err(Error::Store(format!("unknown command `{other}`"))),
        }
        Ok(result)
    }

    /// Everything the session's state modules hold, as of now.
    pub fn session_state(&self) -> SessionState {
        SessionState::load(&self.store.lock().unwrap())
    }

    /// The projected messages, as the next request would carry them.
    pub fn context(&self) -> Vec<Item> {
        project(&self.store.lock().unwrap())
    }
}
