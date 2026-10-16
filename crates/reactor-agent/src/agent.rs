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
//! - **Reduction is a ladder** ([ADR-0058]): old tool output is cut down to stubs when that
//!   alone is enough; else the older work is summarized; if the summary cannot be had, it is
//!   faded with no model; and if nothing older is left, the biggest results are cut. Each
//!   stage writes one entry and loses nothing.
//! - **Two loop invariants** survive from ADR-0025 as rules rather than an extension's
//!   edge cases: reductions that do not get the context to fit stop the turn after a
//!   budget of attempts, and a *failed* reduction stops it at once — the context did
//!   not shrink, so the next request would overflow again.
//! - **The log stays valid whatever happens.** Every tool call in a recorded reply
//!   gets a recorded result, even if the turn is cancelled or a reduction fails
//!   mid-batch, so a session can always be resumed and its history always replays.
//!
//! [ADR-0037]: ../../../docs/adr/0037-context-reduction-is-one-budget-manager.md
//! [ADR-0058]: ../../../docs/adr/0058-reduction-is-a-ladder-and-the-budget-follows-the-window.md
//! [ADR-0057]: ../../../docs/adr/0057-a-turn-recovers-from-a-rejected-or-cut-off-request.md

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reactor_context::settings::Settings;
use reactor_context::{identity, manifest, reporting, scenario};
use reactor_core::Paths;
use reactor_core::probe::Status;
use reactor_db::manifest::Author;
use serde_json::Value;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;

use crate::budget::{
    self, BudgetConfig, Plan, Replay, Summarizer, SummaryRequest, context_tokens, default_output,
    needs_reduction, plan,
};
use crate::classify::StopKind;
use crate::context::{
    self, Item, KEY_IDENTITY, KEY_REPORTING, KEY_SCENARIO, Msg, SessionState, Source,
    estimate_tokens, msg_tokens, project, save_state,
};
use crate::deferral::{self, Candidate};
use crate::entry::{Attachment, EntryId, Kind, Mode, Trigger, Usage};
use crate::error::{Error, Result};
use crate::inspect::ContextPreview;
use crate::llm::{Delta, Llm, LlmRequest, Reply, ToolSpec, backoff, complete_watched};
use crate::prompt::{self, RegistryView};
use crate::skills::{self, Skill};
use crate::store::Store;
use crate::tools::{ToolCtx, ToolOutput, Tools};
use crate::truncate;

/// Replies in a row cut off in the middle of a tool call before the turn stops.
const MAX_CUT_CALLS: usize = 3;

/// The last tool call of a turn, with its result, and how many times in a row it has come out
/// the same. A call is "the same" only if its result is too, so polling something whose output
/// changes is not a loop.
#[derive(Default)]
struct Repeats {
    last: Option<u64>,
    count: usize,
}

impl Repeats {
    /// Note a call and what it gave; how many times in a row that now is.
    fn note(&mut self, name: &str, args: &Value, result: &str) -> usize {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (name, args.to_string(), result).hash(&mut h);
        let sig = h.finish();
        if self.last == Some(sig) {
            self.count += 1;
        } else {
            self.last = Some(sig);
            self.count = 1;
        }
        self.count
    }
}

/// What a frontend watches. Sent as things happen; never required for correctness.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Appended(EntryId),
    Text(String),
    Thinking(String),
    ToolCallStarted {
        name: String,
    },
    ToolStart {
        id: String,
        name: String,
        args: Value,
    },
    /// Live tool output, as it arrives.
    ToolOutput {
        id: String,
        chunk: String,
    },
    ToolEnd {
        id: String,
        entry: EntryId,
        is_error: bool,
    },
    Usage(Usage),
    Reduced {
        entry: EntryId,
        mode: Mode,
        trigger: Trigger,
        before_tokens: u64,
        after_tokens: u64,
    },
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
    /// The same tool call, with the same result, this many times in a row ends the turn: the
    /// model is going round in circles. Zero turns it off.
    pub max_repeats: usize,
    /// The longest a reply may be. Unset, it is a quarter of the window, at most 32k, and
    /// always cut to the room the window has left ([ADR-0057]).
    pub max_tokens: Option<u64>,
    /// How long the model may say nothing before the call is given up on (zero: never).
    pub idle_timeout: Duration,
    /// How many times a failure worth another go (a busy server, a dropped stream) is retried.
    pub transient_retries: usize,
    /// The wait before the first retry; each doubles it.
    pub retry_delay: Duration,
    /// What the catalogue says now. Replaceable, so the loop can be tested without probing.
    pub registry: Arc<dyn Fn() -> RegistryView + Send + Sync>,
    /// The user's embedding model, if they chose one: it lets `docs_search` ask by meaning.
    pub embedder: crate::embed::EmbedderSlot,
    /// The MCP servers' tools, when there is a host for them (shared by every session of a
    /// process). Without one the model is offered no MCP tools.
    pub mcp: Option<Arc<crate::mcp::McpHost>>,
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
            max_repeats: 5,
            max_tokens: None,
            idle_timeout: Duration::from_secs(300),
            transient_retries: 3,
            retry_delay: Duration::from_secs(1),
            registry,
            embedder: Default::default(),
            mcp: None,
        }
    }
}

/// The registry as `reactor-core` reports it, with the normal probe cache.
pub fn registry_from_core(paths: Paths) -> Arc<dyn Fn() -> RegistryView + Send + Sync> {
    Arc::new(
        move || match reactor_core::commands::registry(&paths, Default::default()) {
            Ok(done) => RegistryView {
                block: done.report.block.clone(),
                skill_dirs: done.report.skill_paths.clone(),
                usable: done
                    .report
                    .tools
                    .iter()
                    .filter(|t| t.active && t.status == Status::Present)
                    .map(|t| t.id.clone())
                    .collect(),
                mcp: done
                    .report
                    .tools
                    .iter()
                    .filter(|t| t.mcp.is_some() && t.active && t.status == Status::Present)
                    .map(|t| t.id.clone())
                    .collect(),
            },
            Err(_) => RegistryView::default(),
        },
    )
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

/// What the context costs, in estimated tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measure {
    /// System prompt and tool specs.
    pub fixed: u64,
    /// The projected messages.
    pub messages: u64,
    pub total: u64,
    /// Real tokens per estimated token, as learned from the provider (1.0 until it reports).
    pub calibration: f64,
}

/// What one request is made from.
struct Gathered {
    view: RegistryView,
    settings: Settings,
    state: SessionState,
    items: Vec<Item>,
    offered: Vec<Skill>,
}

/// The attachments of the current branch that are not detached.
pub fn attachments_of(store: &Store) -> Vec<(EntryId, Attachment)> {
    let branch = store.branch();
    let detached: std::collections::HashSet<EntryId> = branch
        .iter()
        .filter_map(|e| match &e.kind {
            Kind::Detach { attachment } => Some(*attachment),
            _ => None,
        })
        .collect();
    branch
        .iter()
        .filter_map(|e| match &e.kind {
            Kind::Attachment(a) if !detached.contains(&e.id) => Some((e.id, a.clone())),
            _ => None,
        })
        .collect()
}

fn entry_label(source: Source) -> String {
    match source {
        Source::Entry(id) | Source::Reduction(id) => format!("#{id}"),
    }
}

/// A reduction in force, for a frontend to list and offer to undo.
#[derive(Debug, Clone, PartialEq)]
pub struct ReductionInfo {
    pub entry: EntryId,
    pub mode: Mode,
    pub trigger: Trigger,
    pub covers: usize,
    pub before_tokens: u64,
    pub after_tokens: u64,
    pub summary: Option<String>,
    pub stubs: usize,
    /// Tool results this reduction shows as stubs in place.
    pub masks: usize,
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
    /// What the last request left for `tool_search`: the tool needs it to answer, and it is
    /// what that request's model call was shown.
    deferred: Mutex<Arc<Vec<ToolSpec>>>,
}

impl<L: Llm, S: Summarizer> Agent<L, S> {
    pub fn new(
        llm: L,
        summarizer: S,
        store: Store,
        tools: Tools,
        cfg: AgentConfig,
    ) -> (Self, UnboundedReceiver<Event>) {
        let (events, rx) = unbounded_channel();
        let authored_skills = cfg
            .skills_dir
            .as_deref()
            .map(skills::discover)
            .unwrap_or_default();
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
                deferred: Mutex::new(Arc::new(Vec::new())),
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

    /// What a request is made from: the catalogue, the settings, the session's state and the
    /// projected messages.
    async fn gather(&self) -> Result<Gathered> {
        let registry = self.cfg.registry.clone();
        let view = tokio::task::spawn_blocking(move || registry())
            .await
            .map_err(|e| Error::Store(e.to_string()))?;
        let settings = self.settings();
        let (mut state, items) = {
            let store = self.store.lock().unwrap();
            (SessionState::load(&store), project(&store))
        };
        // What is advertised and said about the case follows the switches in effect:
        // the session's layer over the default (ADR-0038).
        state.memory = state.memory.over(&settings.memory);
        if state.docs {
            let case = self.store.lock().unwrap().case().clone();
            let embedder = self.cfg.embedder.get();
            state.meaning = tokio::task::spawn_blocking(move || {
                crate::embed::meaning_ready(&case, embedder.as_deref())
            })
            .await
            .unwrap_or(false);
        }
        let offered = skills::offered(&self.authored_skills, &view.usable, &view.skill_dirs);
        Ok(Gathered {
            view,
            settings,
            state,
            items,
            offered,
        })
    }

    /// The tool definitions of this request: ours, then those of the MCP tools that are
    /// active, less the groups left for `tool_search` (ADR-0055).
    fn definitions(&self, g: &Gathered) -> deferral::Plan {
        let mut candidates: Vec<Candidate> = self
            .tools
            .specs_limited(&g.state, self.output_limit(&g.settings))
            .into_iter()
            .map(|spec| Candidate {
                origin: crate::inspect::Origin::ToolDefinition,
                group: self
                    .tools
                    .find(&spec.name)
                    .and_then(|t| t.group())
                    .map(str::to_string),
                spec,
            })
            .collect();
        if let Some(host) = &self.cfg.mcp {
            candidates.extend(host.grouped(&g.view.mcp).into_iter().map(|(server, spec)| {
                Candidate {
                    origin: crate::inspect::Origin::McpDefinition,
                    group: Some(format!("mcp:{server}")),
                    spec,
                }
            }));
        }
        deferral::plan(
            candidates,
            &g.state.loaded,
            g.settings.tool_search.defer_over_tokens,
        )
    }

    fn system_parts(&self, g: &Gathered, plan: &deferral::Plan) -> Vec<(prompt::Part, String)> {
        // The rules of the tools that are in the request, once each, in the request's order.
        let mut rules: Vec<String> = Vec::new();
        for (_, spec) in &plan.advertised {
            for rule in self
                .tools
                .find(&spec.name)
                .map_or(&[][..], |t| t.guidelines())
            {
                if !rules.iter().any(|r| r == rule) {
                    rules.push((*rule).to_string());
                }
            }
        }
        let deferred: Vec<String> = plan.deferred.iter().map(|t| t.name.clone()).collect();
        prompt::system_parts(&prompt::Inputs {
            rules: &rules,
            deferred: &deferred,
            base: &self.cfg.base_prompt,
            settings: &g.settings,
            state: &g.state,
            registry: &g.view,
            skills: &g.offered,
            scenarios_dir: &self.cfg.scenarios_dir,
        })
    }

    /// The request as it would be sent now, and what it costs.
    async fn build(&self) -> Result<(LlmRequest, u64, Vec<Item>)> {
        let g = self.gather().await?;
        let plan = self.definitions(&g);
        let system = prompt::join_parts(&self.system_parts(&g, &plan));
        *self.deferred.lock().unwrap() = Arc::new(plan.deferred.clone());
        let tools: Vec<ToolSpec> = plan.advertised.into_iter().map(|(_, s)| s).collect();
        let Gathered {
            settings,
            state,
            items,
            ..
        } = g;
        let fixed = estimate_tokens(&system)
            + tools
                .iter()
                .map(|t| {
                    estimate_tokens(&t.name)
                        + estimate_tokens(&t.description)
                        + estimate_tokens(&t.parameters.to_string())
                })
                .sum::<u64>();
        let mut messages: Vec<Msg> = items.iter().map(|i| i.msg.clone()).collect();

        // Level 1 reporting: a reminder on every model call until the folder changes.
        // Ephemeral by design -- never stored, so it cannot pile up in a long session.
        if let Some(nag) = self
            .tracker
            .lock()
            .unwrap()
            .nag(&state.reporting, &settings.reporting)
        {
            messages.push(Msg::User { text: nag });
        }
        Ok((
            LlmRequest {
                system,
                messages,
                tools,
                max_tokens: self.cfg.max_tokens,
                num_ctx: None,
                no_tools: false,
            },
            fixed,
            items,
        ))
    }

    /// The request as it would be sent now, taken apart: every piece with where it comes
    /// from. The same pieces [`Agent::build`] sends, so what this shows is what the model gets.
    pub async fn context_preview(&self) -> Result<ContextPreview> {
        use crate::inspect::{Origin, Section, Segment};
        let g = self.gather().await?;
        let mut segments = Vec::new();

        let plan = self.definitions(&g);
        let parts = self.system_parts(&g, &plan);
        // The budget counts the joined prompt, so the total does too; each piece's own
        // estimate is rounded separately and may differ from its share by a token.
        let system_tokens = estimate_tokens(&prompt::join_parts(&parts));
        for (part, text) in parts {
            let (origin, label) = match part {
                prompt::Part::Base => (Origin::Base, "Base prompt"),
                prompt::Part::Identity => (Origin::Identity, "Identity"),
                prompt::Part::Registry => (Origin::Registry, "Available tools"),
                prompt::Part::Rules => (Origin::Base, "Tool rules"),
                prompt::Part::Skills => (Origin::Skills, "Skills"),
                prompt::Part::CaseGraph => (Origin::CaseGraph, "Case graph"),
                prompt::Part::Manifest => (Origin::Manifest, "Manifest"),
                prompt::Part::Reporting => (Origin::Reporting, "Reporting"),
                prompt::Part::Scenario => (Origin::Scenario, "Scenario phase"),
            };
            segments.push(Segment {
                section: Section::System,
                origin,
                label: label.to_string(),
                tokens: estimate_tokens(&text),
                text,
                entry: None,
                parts: None,
            });
        }

        // What the request carries, in its order (tool_search among them when a group is
        // left for it).
        let defs = plan.advertised;
        for (origin, t) in defs {
            let text = format!(
                "{}\n\n{}",
                t.description,
                serde_json::to_string_pretty(&t.parameters).unwrap_or_default()
            );
            let tokens = estimate_tokens(&t.name)
                + estimate_tokens(&t.description)
                + estimate_tokens(&t.parameters.to_string());
            segments.push(Segment {
                section: Section::Tools,
                origin,
                label: t.name,
                text,
                tokens,
                entry: None,
                parts: None,
            });
        }

        for item in &g.items {
            let (origin, label, text) = match (&item.msg, item.source) {
                (Msg::User { text }, Source::Reduction(id)) => (
                    Origin::Reduction,
                    format!("#{id} reduction stand-in"),
                    text.clone(),
                ),
                (Msg::User { text }, Source::Entry(id)) => {
                    let given = match self.store.lock().unwrap().get(id).map(|e| &e.kind) {
                        Some(Kind::Attachment(a)) => Some(a.label.clone()),
                        _ => None,
                    };
                    match given {
                        Some(label) => (
                            Origin::Document,
                            format!("#{id} given: {label}"),
                            text.clone(),
                        ),
                        None => (Origin::User, format!("#{id} you"), text.clone()),
                    }
                }
                (Msg::Assistant { blocks }, source) => {
                    let text = blocks
                        .iter()
                        .map(|b| match b {
                            crate::entry::Block::Text { text } => text.clone(),
                            crate::entry::Block::Thinking { text, .. } => {
                                format!("[thinking]\n{text}")
                            }
                            crate::entry::Block::ToolCall {
                                name, arguments, ..
                            } => format!("[tool call] {name} {arguments}"),
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    (
                        Origin::Assistant,
                        format!("{} assistant", entry_label(source)),
                        text,
                    )
                }
                (
                    Msg::ToolResult {
                        name,
                        content,
                        is_error,
                        ..
                    },
                    source,
                ) => (
                    Origin::ToolResult,
                    format!(
                        "{} {name}{}",
                        entry_label(source),
                        if *is_error { " (error)" } else { "" }
                    ),
                    content.clone(),
                ),
            };
            let entry = match item.source {
                Source::Entry(id) => Some(id),
                Source::Reduction(_) => None,
            };
            let parts = match &item.msg {
                Msg::Assistant { blocks } => Some(crate::context::assistant_parts(blocks)),
                _ => None,
            };
            segments.push(Segment {
                section: Section::Messages,
                origin,
                label,
                tokens: msg_tokens(&item.msg),
                text,
                entry,
                parts,
            });
        }

        if let Some(nag) = self
            .tracker
            .lock()
            .unwrap()
            .nag(&g.state.reporting, &g.settings.reporting)
        {
            segments.push(Segment {
                section: Section::Messages,
                origin: Origin::Reminder,
                label: "reporting reminder".into(),
                tokens: estimate_tokens(&nag) + 8,
                text: nag,
                entry: None,
                parts: None,
            });
        }
        let total_tokens = system_tokens
            + segments
                .iter()
                .filter(|s| s.section != Section::System)
                .map(|s| s.tokens)
                .sum::<u64>();
        Ok(ContextPreview {
            segments,
            total_tokens,
            calibration: *self.ratio.lock().unwrap(),
        })
    }

    // -- the budget ------------------------------------------------------------------------

    /// The budget in force now: the built-in baseline (`cfg.budget`, whose window comes
    /// from the model), then the global `settings.json`, then this session's own layer
    /// (ADR-0038). Read every time, so a change takes effect on the next request.
    pub fn effective_budget(&self) -> BudgetConfig {
        let global = self.settings().context;
        let session = SessionState::load(&self.store.lock().unwrap()).context;
        let eff = session.over(&global);
        let base = match eff.window {
            // A reserve that was only the default follows the window it was the default of.
            Some(w) if w != self.cfg.budget.window => self.cfg.budget.rebased(w),
            _ => self.cfg.budget,
        };
        BudgetConfig {
            window: eff.window.unwrap_or(base.window),
            reserve: eff.reserve.unwrap_or(base.reserve),
            pct: eff.pct.unwrap_or(base.pct),
            keep: eff.keep.unwrap_or(base.keep),
            mode: match eff.mode.as_deref() {
                Some("fade") => Mode::Fade,
                Some("compact") => Mode::Compact,
                Some("auto") => Mode::Auto,
                _ => base.mode,
            },
            summary_tokens: base.summary_tokens,
            mask_target: eff.mask_target.unwrap_or(base.mask_target),
            protect: eff.protect.unwrap_or(base.protect),
        }
    }

    /// The two stored layers of the context settings: (global, this session's).
    pub fn context_layers(
        &self,
    ) -> (
        reactor_context::settings::ContextSettings,
        reactor_context::settings::ContextSettings,
    ) {
        (
            self.settings().context,
            SessionState::load(&self.store.lock().unwrap()).context,
        )
    }

    /// Set this session's layer of the context settings.
    pub fn set_session_context(
        &self,
        layer: &reactor_context::settings::ContextSettings,
    ) -> Result<()> {
        save_state(
            &mut self.store.lock().unwrap(),
            context::KEY_CONTEXT,
            serde_json::to_value(layer).unwrap(),
        )?;
        Ok(())
    }

    /// Make a layer the global default (`settings.json`).
    pub fn set_global_context(
        &self,
        layer: reactor_context::settings::ContextSettings,
    ) -> Result<()> {
        let mut s = self.settings();
        s.context = layer;
        s.save(&self.cfg.paths)?;
        Ok(())
    }

    fn scaled_budget(&self) -> BudgetConfig {
        self.effective_budget().scaled(*self.ratio.lock().unwrap())
    }

    /// A person asking to reduce wants *more* gone than the budget would take: the keep
    /// window shrinks to the latest group of work. Their last message stays in view.
    fn manual_budget(&self) -> BudgetConfig {
        BudgetConfig {
            keep: 0.0,
            ..self.scaled_budget()
        }
    }

    /// What a reduction would do right now, without doing it.
    pub async fn preview(&self, mode: Mode) -> Result<Plan> {
        let (_, fixed, items) = self.build().await?;
        plan(fixed, &items, &self.manual_budget(), mode)
            .map_err(|n| Error::Reduction(n.to_string()))
    }

    /// Reduce now, at a person's request. A manual reduction means *summarize* in every
    /// mode but `fade` (a manual fade exists for a burst of dumps that should just go).
    pub async fn reduce_now(&self, mode: Mode) -> Result<EntryId> {
        let (base, fixed, items) = self.build().await?;
        let p = plan(fixed, &items, &self.manual_budget(), mode)
            .map_err(|n| Error::Reduction(n.to_string()))?;
        self.carry_out(
            &base,
            &items,
            &p,
            Trigger::Manual,
            &CancellationToken::new(),
        )
        .await
    }

    /// Say how big the summarizer's window is, and how much transcript fits in it.
    fn size_summary(&self, req: &mut SummaryRequest) {
        req.window = self.effective_budget().window;
        // In estimated tokens, leaving a quarter for the instructions, the notes and the
        // estimate being low.
        req.input_cap = (self.scaled_budget().window as f64 * 0.75) as u64;
    }

    async fn carry_out(
        &self,
        base: &LlmRequest,
        items: &[Item],
        p: &Plan,
        trigger: Trigger,
        cancel: &CancellationToken,
    ) -> Result<EntryId> {
        let prepared = budget::prepare(
            &self.store.lock().unwrap(),
            items,
            p,
            trigger,
            p.summary_tokens,
        )?;
        // Nothing is written until the summary is in, so giving up on it midway leaves the
        // log as it was.
        let summary = match &prepared.request {
            Some(req) => {
                let mut req = req.clone();
                self.size_summary(&mut req);
                // What the main model was sent up to the end of the range, for a summarizer
                // that is the main model to begin its request the same way.
                req.replay = Some(Replay {
                    system: base.system.clone(),
                    tools: base.tools.clone(),
                    messages: items[..p.keep_from].iter().map(|i| i.msg.clone()).collect(),
                    model: self.llm.name(),
                });
                tokio::select! {
                    _ = cancel.cancelled() => return Err(Error::Cancelled),
                    r = budget::summarize(&self.summarizer, req) => Some(r?),
                }
            }
            None => None,
        };
        let id = budget::commit(&mut self.store.lock().unwrap(), prepared, summary)?;
        if let Some(Kind::Reduction(r)) = self.store.lock().unwrap().get(id).map(|e| e.kind.clone())
        {
            self.emit(Event::Reduced {
                entry: id,
                mode: r.mode,
                trigger,
                before_tokens: r.before_tokens,
                after_tokens: r.after_tokens,
            });
        }
        self.emit(Event::Appended(id));
        Ok(id)
    }

    /// Count one more reduction in a row, or stop: they are not getting the context to fit.
    fn count_attempt(&self, in_a_row: &mut usize) -> Result<()> {
        if *in_a_row >= self.cfg.max_reductions_in_a_row {
            return Err(Error::ReductionBudget {
                attempts: *in_a_row,
            });
        }
        *in_a_row += 1;
        Ok(())
    }

    /// Which tool results a masking pass would show as stubs.
    fn masks(
        &self,
        fixed: u64,
        items: &[Item],
        opts: &budget::MaskOptions,
    ) -> Option<budget::MaskPlan> {
        budget::plan_mask(&self.store.lock().unwrap(), fixed, items, opts)
    }

    /// Show those tool results as stubs. One entry in the log; the results stay in it, whole.
    fn apply_mask(&self, m: budget::MaskPlan, trigger: Trigger) -> Result<EntryId> {
        let (before, after) = (m.before_tokens, m.after_tokens);
        let id = budget::commit_mask(&mut self.store.lock().unwrap(), m, trigger)?;
        self.emit(Event::Reduced {
            entry: id,
            mode: Mode::Fade,
            trigger,
            before_tokens: before,
            after_tokens: after,
        });
        self.emit(Event::Appended(id));
        Ok(id)
    }

    /// The summarizer could not be had: drop the older entries to stubs instead. That needs
    /// no model, and shrinks the context just the same, so the turn can go on (ADR-0058).
    async fn fade_instead(
        &self,
        base: &LlmRequest,
        items: &[Item],
        fixed: u64,
        cfg: &BudgetConfig,
        cancel: &CancellationToken,
        why: Error,
    ) -> Result<()> {
        let Ok(p) = plan(fixed, items, cfg, Mode::Fade) else {
            return Err(why);
        };
        self.emit(Event::Notice(format!(
            "summarizing failed ({}); the older entries were dropped to stubs instead. Nothing is lost: `history_read` reaches every one.",
            why.to_string().lines().next().unwrap_or("").chars().take(160).collect::<String>()
        )));
        match self
            .carry_out(base, items, &p, Trigger::Budget, cancel)
            .await
        {
            Ok(_) => Ok(()),
            Err(Error::Cancelled) => Err(Error::Cancelled),
            // The summarizer's failure is the one to report: it is why this happened.
            Err(_) => Err(why),
        }
    }

    /// Reduce until the context fits. Called before every request and after every tool
    /// result. `in_a_row` counts reductions that did not by themselves get it to fit.
    ///
    /// A ladder, cheapest first (ADR-0058): mask old tool output when that alone is enough;
    /// else summarize (and fade the tool output); if the summary cannot be had, fade the
    /// range with no model; and if nothing older is left, cut the biggest results, the
    /// newest too.
    async fn ensure_fits(
        &self,
        in_a_row: &mut usize,
        outcome: &mut Outcome,
        cancel: &CancellationToken,
    ) -> Result<()> {
        loop {
            let (base, fixed, items) = self.build().await?;
            let cfg = self.scaled_budget();
            if !needs_reduction(fixed, &items, &cfg) {
                *in_a_row = 0;
                return Ok(());
            }

            // 1. Mask. Only in `auto`: `compact` asks for summaries and `fade` for the
            // range to go.
            if cfg.mode == Mode::Auto
                && let Some(m) = self.masks(fixed, &items, &budget::MaskOptions::proactive(&cfg))
            {
                self.count_attempt(in_a_row)?;
                self.apply_mask(m, Trigger::Budget)?;
                outcome.reductions += 1;
                continue;
            }

            let p = match plan(fixed, &items, &cfg, cfg.mode) {
                Ok(p) => p,
                Err(nothing) => {
                    // Over the trigger with nothing older to reduce: fine while there is
                    // still room, and once there is not, cut what is big.
                    let used = context_tokens(fixed, &items);
                    if used > cfg.hard() {
                        // 4. The last resort.
                        if let Some(m) =
                            self.masks(fixed, &items, &budget::MaskOptions::last_resort(&cfg))
                        {
                            self.count_attempt(in_a_row)?;
                            self.apply_mask(m, Trigger::Budget)?;
                            outcome.reductions += 1;
                            continue;
                        }
                        return Err(Error::Reduction(format!(
                            "the context is full ({used} of {} tokens) and {nothing}; nothing in it is big enough to cut down",
                            cfg.hard()
                        )));
                    }
                    *in_a_row = 0;
                    return Ok(());
                }
            };
            self.count_attempt(in_a_row)?;
            // 2. Summarize; 3. if that cannot be done, fade.
            match self
                .carry_out(&base, &items, &p, Trigger::Budget, cancel)
                .await
            {
                Ok(_) => {}
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(why) if p.needs_summarizer() => {
                    self.fade_instead(&base, &items, fixed, &cfg, cancel, why)
                        .await?;
                }
                Err(e) => return Err(e),
            }
            outcome.reductions += 1;
        }
    }

    /// The output limit a call gets when it does not ask for one: the person's setting, or
    /// a share of the window (ADR-0058).
    fn output_limit(&self, settings: &Settings) -> truncate::Limit {
        truncate::Limit::new(
            settings
                .tool_output
                .for_window(self.effective_budget().window),
        )
    }

    // -- asking the model -----------------------------------------------------------------------

    /// Set a request's reply limit and window. Returns whether the limit had to be cut to
    /// the room the window leaves, which a reduction can widen.
    fn size_request(&self, req: &mut LlmRequest, used_estimate: u64, boost: bool) -> bool {
        let b = self.effective_budget();
        let used = (used_estimate as f64 * *self.ratio.lock().unwrap()) as u64;
        let mut want = self
            .cfg
            .max_tokens
            .unwrap_or_else(|| default_output(b.window));
        if boost {
            want = (want + want / 2).min(60_000).max(want);
        }
        let room = b.window.saturating_sub(used + 512 + b.window / 50).max(256);
        req.max_tokens = Some(want.min(room));
        req.num_ctx = Some(b.window);
        room < want
    }

    /// Reduce now, down to a smaller keep window than the budget aims at. For the moment a
    /// request has been rejected or has no room for its reply: the budget's own estimate was
    /// not enough. Whether anything older could be reduced.
    async fn force_reduce(&self, cancel: &CancellationToken, keep_scale: f64) -> Result<bool> {
        let (base, fixed, items) = self.build().await?;
        let mut cfg = self.scaled_budget();
        cfg.keep *= keep_scale;
        match plan(fixed, &items, &cfg, cfg.mode) {
            Ok(p) => {
                self.carry_out(&base, &items, &p, Trigger::Budget, cancel)
                    .await?;
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }

    /// One answer from the model, recovering from what can be recovered from
    /// ([ADR-0057]): a request the provider found too long is reduced and sent again, a
    /// busy or unreachable server is tried again after a pause, a reply that ran out of room
    /// before saying anything is given more. Each is tried once or a few times, never for ever.
    /// Returns the reply and the estimate of the request it answers.
    async fn ask_model(
        &self,
        cancel: &CancellationToken,
        in_a_row: &mut usize,
        outcome: &mut Outcome,
    ) -> Result<(Reply, u64)> {
        let (mut overflowed, mut lengthened, mut tries) = (false, false, 0usize);
        loop {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            self.ensure_fits(in_a_row, outcome, cancel).await?;
            let (mut req, fixed, items) = self.build().await?;
            let cramped = self.size_request(&mut req, context_tokens(fixed, &items), lengthened);
            let estimated: u64 = context_tokens(0, &project(&self.store.lock().unwrap()))
                + estimate_tokens(&req.system);

            let on_delta = |d: Delta| {
                self.emit(match d {
                    Delta::Text(t) => Event::Text(t),
                    Delta::Thinking(t) => Event::Thinking(t),
                    Delta::ToolCall { name } => Event::ToolCallStarted { name },
                })
            };
            let sent = tokio::select! {
                _ = cancel.cancelled() => return Err(Error::Cancelled),
                r = complete_watched(&self.llm, req, self.cfg.idle_timeout, on_delta) => r,
            };
            match sent {
                Ok(reply) => {
                    // Out of room before it said or did anything: thinking took it all.
                    if reply.stop_kind() == StopKind::Length && !reply.has_output() {
                        if lengthened {
                            return Err(Error::Model(
                                "the model ran out of output room while thinking, twice. Raise the output limit, lower the thinking level, or ask for a smaller step.".into(),
                            ));
                        }
                        lengthened = true;
                        self.emit(Event::Notice(
                            "the model ran out of output room while thinking; trying again with more room".into(),
                        ));
                        if cramped {
                            self.force_reduce(cancel, 0.6).await?;
                        }
                        continue;
                    }
                    return Ok((reply, estimated));
                }
                Err(Error::Overflow(why)) => {
                    let window = self.effective_budget().window;
                    if overflowed {
                        return Err(Error::Overflow(format!(
                            "{why}\n\nReducing the context and asking again did not help. The model's real window may be smaller than the {window} tokens REactor assumes: set it in Settings → Context."
                        )));
                    }
                    overflowed = true;
                    // The estimate was low, and will be again: believe it less from now on.
                    {
                        let mut r = self.ratio.lock().unwrap();
                        *r = (*r * 1.25).min(4.0);
                    }
                    self.emit(Event::Notice(
                        "the provider rejected the request as too long; reducing the context and trying again".into(),
                    ));
                    if !self.force_reduce(cancel, 0.6).await? {
                        return Err(Error::Overflow(format!(
                            "{why}\n\nThere is nothing older to reduce: the system prompt, the tools and the current work alone are too big for the window. Set the real window in Settings → Context, switch some tools off, or use a model with a larger window."
                        )));
                    }
                }
                Err(Error::Transient(why)) => {
                    if tries >= self.cfg.transient_retries {
                        return Err(Error::Transient(format!(
                            "{why} (gave up after {tries} retries)"
                        )));
                    }
                    tries += 1;
                    let wait = backoff(self.cfg.retry_delay, tries);
                    self.emit(Event::Notice(format!(
                        "the model failed ({}); trying again in {}s ({tries}/{})",
                        why.chars().take(120).collect::<String>(),
                        wait.as_secs().max(1),
                        self.cfg.transient_retries
                    )));
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(Error::Cancelled),
                        _ = tokio::time::sleep(wait) => {}
                    }
                }
                Err(Error::Blocked(why)) => {
                    return Err(Error::Blocked(format!(
                        "the provider blocked this request: {why}\n\nThe same request would be blocked again, so it was not retried. Rephrase it, or choose another model."
                    )));
                }
                Err(e) => return Err(e),
            }
        }
    }

    // -- one turn ------------------------------------------------------------------------------

    /// Run one turn on `text`.
    pub async fn prompt(&self, text: &str, cancel: CancellationToken) -> Result<Outcome> {
        let mut outcome = Outcome::default();
        let mut in_a_row = 0usize;
        // Replies in a row that were cut off by the output limit in the middle of a tool call.
        let mut cut_calls = 0usize;
        let mut repeats = Repeats::default();
        let mut next: String = text.to_string();

        // A scenario moved on by hand is waiting to hear what the phase it left concluded:
        // this prompt is that (the person was asked to say it in their next prompt).
        {
            let mut store = self.store.lock().unwrap();
            if let Some(mut sc) = SessionState::load(&store).scenario
                && scenario::record_summary(&mut sc, text)
            {
                save_state(&mut store, KEY_SCENARIO, serde_json::to_value(&sc).unwrap())?;
            }
        }

        'turn: loop {
            let settings = self.settings();
            {
                let state = SessionState::load(&self.store.lock().unwrap());
                self.tracker.lock().unwrap().before_agent_start(
                    &state.reporting,
                    &settings.reporting,
                    &next,
                );
            }
            // What the message names goes along with it, ahead of it, as attachments.
            self.attach_references(&next).await?;
            self.append(Kind::User { text: next.clone() })?;

            loop {
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                let (reply, estimated) =
                    self.ask_model(&cancel, &mut in_a_row, &mut outcome).await?;
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
                let calls: Vec<(String, String, Value)> = reply
                    .tool_calls()
                    .map(|(i, n, a)| (i.to_string(), n.to_string(), a.clone()))
                    .collect();
                self.append(Kind::Assistant {
                    blocks: reply.blocks.clone(),
                    model: Some(self.llm.name()),
                    usage: reply.usage,
                    stop: reply.stop.clone(),
                })?;

                // A reply the output limit cut off may hold a tool call whose arguments are
                // unfinished: none of its calls is run. Each is answered, so the log stays
                // valid, and the model is told why and asks again, smaller.
                if reply.stop_kind() == StopKind::Length && !calls.is_empty() {
                    cut_calls += 1;
                    self.fill_results(
                        &calls,
                        "not run: your reply was cut off by the output limit before this call was complete. Make a smaller call, or split the work over several.",
                    )?;
                    if cut_calls >= MAX_CUT_CALLS {
                        return Err(Error::Model(format!(
                            "stopped: {MAX_CUT_CALLS} replies in a row were cut off by the output limit in the middle of a tool call. Raise the output limit, or ask for a smaller step."
                        )));
                    }
                    self.emit(Event::Notice(
                        "the reply was cut off by the output limit in the middle of a tool call; the call was not run".into(),
                    ));
                    continue;
                }
                cut_calls = 0;
                if reply.stop_kind() == StopKind::Length {
                    self.emit(Event::Notice(
                        "the reply was cut off by the output limit".into(),
                    ));
                }

                if calls.is_empty() {
                    // The turn has settled. Level-2 reporting may send it back.
                    let (state, settings) = (
                        SessionState::load(&self.store.lock().unwrap()),
                        self.settings(),
                    );
                    let verdict = self
                        .tracker
                        .lock()
                        .unwrap()
                        .settled(&state.reporting, &settings.reporting);
                    match verdict {
                        reporting::Settled::Revert => {
                            outcome.reverts += 1;
                            // Fork from the last user message: the reverted turn stays in
                            // the log as a dead branch, recoverable like everything else.
                            {
                                let mut store = self.store.lock().unwrap();
                                let last_user = store
                                    .branch()
                                    .iter()
                                    .rev()
                                    .find(|e| matches!(e.kind, Kind::User { .. }))
                                    .map(|e| e.id);
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
                    return Err(Error::Model(format!(
                        "stopped after {} model round trips in one turn",
                        self.cfg.max_rounds
                    )));
                }
                outcome.tool_calls += calls.len();
                self.run_tools(&calls, &cancel, &mut in_a_row, &mut outcome, &mut repeats)
                    .await?;
            }
        }
        self.emit(Event::Finished);
        Ok(outcome)
    }

    /// Attach what the `#` references in `text` name. One that names nothing is left as the
    /// text it is, and a notice says so.
    async fn attach_references(&self, text: &str) -> Result<()> {
        let found = crate::references::find(text);
        if found.is_empty() {
            return Ok(());
        }
        let (cwd, case) = (
            self.cfg.cwd.clone(),
            self.store.lock().unwrap().case().clone(),
        );
        let resolved = tokio::task::spawn_blocking(move || {
            found
                .into_iter()
                .map(|r| match &r.target {
                    crate::references::Target::File(path) => crate::references::file(&cwd, path),
                    crate::references::Target::Doc { name, section } => {
                        crate::references::document(case.db(), name, section.as_deref())
                    }
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| Error::Store(e.to_string()))?;
        let mut seen: Vec<(String, String)> = Vec::new();
        for r in resolved {
            match r {
                Ok(a) => {
                    // The same thing named twice is sent once.
                    let key = (a.source.clone(), a.part.clone());
                    if seen.contains(&key) {
                        continue;
                    }
                    seen.push(key);
                    self.append(Kind::Attachment(a))?;
                }
                Err(why) => self.emit(Event::Notice(why)),
            }
        }
        Ok(())
    }

    /// Hand the agent a text. It is in the context from the next request on.
    pub fn attach(&self, attachment: Attachment) -> Result<EntryId> {
        self.append(Kind::Attachment(attachment))
    }

    /// Take an attachment out of the context.
    pub fn detach(&self, attachment: EntryId) -> Result<()> {
        self.append(Kind::Detach { attachment }).map(|_| ())
    }

    /// The attachments in the context now, oldest first.
    pub fn attachments(&self) -> Vec<(EntryId, Attachment)> {
        let store = self.store.lock().unwrap();
        attachments_of(&store)
    }

    /// Give every call in `calls` a recorded result, so the log stays replayable.
    fn fill_results(&self, calls: &[(String, String, Value)], why: &str) -> Result<()> {
        for (id, name, _) in calls {
            self.append(Kind::ToolResult {
                call_id: id.clone(),
                name: name.clone(),
                content: why.to_string(),
                is_error: true,
                blob: None,
            })?;
        }
        Ok(())
    }

    async fn run_tools(
        &self,
        calls: &[(String, String, Value)],
        cancel: &CancellationToken,
        in_a_row: &mut usize,
        outcome: &mut Outcome,
        repeats: &mut Repeats,
    ) -> Result<()> {
        let default = self.output_limit(&self.settings());
        for (i, (id, name, args)) in calls.iter().enumerate() {
            if cancel.is_cancelled() {
                self.fill_results(&calls[i..], "not run: the turn was cancelled")?;
                return Err(Error::Cancelled);
            }
            self.emit(Event::ToolStart {
                id: id.clone(),
                name: name.clone(),
                args: args.clone(),
            });

            // Our tools take a limit; an MCP tool's parameters are its server's.
            let ours = self.tools.find(name).is_some();
            let limit = if ours {
                truncate::Limit::for_call(args, default)
            } else {
                default
            };
            let events = self.events.clone();
            let call_id = id.clone();
            let ctx = ToolCtx {
                cwd: self.cfg.cwd.clone(),
                cancel: cancel.clone(),
                emit: Arc::new(move |chunk: &str| {
                    let _ = events.send(Event::ToolOutput {
                        id: call_id.clone(),
                        chunk: chunk.to_string(),
                    });
                }),
                store: self.store.clone(),
                paths: self.cfg.paths.clone(),
                scenarios_dir: self.cfg.scenarios_dir.clone(),
                model: self.llm.name(),
                embedder: self.cfg.embedder.get(),
                limit,
                deferred: self.deferred.lock().unwrap().clone(),
            };
            let out = match (self.tools.find(name), &self.cfg.mcp) {
                (Some(tool), _) => tool.call(args.clone(), &ctx).await,
                // Active or not: deactivating only stops a tool being advertised (ADR-0007).
                (None, Some(host)) if host.knows(name) => {
                    host.call(name, args.clone(), cancel).await
                }
                _ => ToolOutput::err(format!("unknown tool `{name}`")),
            };
            let is_error = out.is_error;
            let kind = self.finalize(id, name, out, limit)?;
            let seen = match &kind {
                Kind::ToolResult { content, .. } => repeats.note(name, args, content),
                _ => 0,
            };
            let entry = self.append(kind)?;
            self.emit(Event::ToolEnd {
                id: id.clone(),
                entry,
                is_error,
            });

            // The same call, giving the same result, again and again: nothing is going to change.
            if self.cfg.max_repeats > 0 && seen >= self.cfg.max_repeats {
                self.fill_results(
                    &calls[i + 1..],
                    "not run: the turn was stopped because the same call kept giving the same result",
                )?;
                return Err(Error::Model(format!(
                    "stopped: `{name}` was called {seen} times in a row with the same arguments ({}) and gave the same result each time. Say what to try instead, or ask for something different.",
                    args.to_string().chars().take(120).collect::<String>()
                )));
            }

            // Reporting asks the filesystem, after every tool call, whether anything was written up.
            {
                let (state, settings) = (
                    SessionState::load(&self.store.lock().unwrap()),
                    self.settings(),
                );
                let snap = reporting::take_snapshot(&self.cfg.cwd.join(&settings.reporting.folder));
                self.tracker
                    .lock()
                    .unwrap()
                    .tool_end(&state.reporting, snap);
            }

            // The boundary check: a dump can overflow the window inside one turn.
            if let Err(e) = self.ensure_fits(in_a_row, outcome, cancel).await {
                self.fill_results(
                    &calls[i + 1..],
                    "not run: context reduction failed, so the turn stopped",
                )?;
                return Err(e);
            }
        }
        Ok(())
    }

    /// Apply the truncation policy: the model sees at most `limit` of the output, and the
    /// whole of it is kept and addressable.
    fn finalize(
        &self,
        call_id: &str,
        name: &str,
        out: ToolOutput,
        limit: truncate::Limit,
    ) -> Result<Kind> {
        let total = match &out.full {
            Some(p) => std::fs::metadata(p)
                .map(|m| m.len())
                .unwrap_or(out.text.len() as u64),
            None => out.text.len() as u64,
        };
        if !limit.needs_cut(total) {
            let text = out.text.replace(truncate::CUT, "");
            return Ok(Kind::ToolResult {
                call_id: call_id.into(),
                name: name.into(),
                content: text,
                is_error: out.is_error,
                blob: None,
            });
        }
        let lines = match &out.full {
            Some(p) => truncate::count_lines_in(p)
                .unwrap_or_else(|_| truncate::count_lines(out.text.as_bytes())),
            None => truncate::count_lines(out.text.as_bytes()),
        };
        let mut store = self.store.lock().unwrap();
        let entry = store.len() as EntryId;
        let blob = match &out.full {
            Some(path) => store.adopt_blob(path)?,
            None => store.write_blob(out.text.as_bytes())?,
        };
        let whole = truncate::Whole { entry, lines };
        let content = truncate::view(&out.text, whole, limit);
        Ok(Kind::ToolResult {
            call_id: call_id.into(),
            name: name.into(),
            content,
            is_error: out.is_error,
            blob: Some(blob),
        })
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
                let Some(e) = manifest::command(&mut m, &settings.manifest, name, args) else {
                    unreachable!()
                };
                if e.persist {
                    // A person typed this, so the case records them as its author.
                    context::save_manifest(&mut store, &m, &Author::analyst())?;
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
                let e = reporting::command(
                    &mut s,
                    &mut self.tracker.lock().unwrap(),
                    &mut settings.reporting,
                    args,
                );
                if e.persist {
                    save_state(&mut store, KEY_REPORTING, serde_json::to_value(s).unwrap())?;
                }
                if e.save_folder.is_some() {
                    settings.save(&self.cfg.paths)?;
                }
                result.notices = e.notices;
            }
            "reactor-scenario" => {
                let mut sc = scenario::Scenario {
                    dir: &self.cfg.scenarios_dir,
                    state: state.scenario.clone(),
                };
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
                    let _ = reactor_core::commands::set_activation(
                        &self.cfg.paths,
                        std::slice::from_ref(t),
                        false,
                        Some(true),
                    );
                }
                result.notices = e.notices;
                result.trigger_turn = e
                    .messages
                    .into_iter()
                    .find(|m| m.trigger_turn)
                    .map(|m| m.content);
                return Ok(result);
            }
            other => return Err(Error::Store(format!("unknown command `{other}`"))),
        }
        Ok(result)
    }

    /// What the context costs right now: the fixed part (system prompt, tool specs) and the
    /// projected messages, in estimated tokens.
    pub async fn measure(&self) -> Result<Measure> {
        let (_, fixed, items) = self.build().await?;
        let messages = context_tokens(0, &items);
        Ok(Measure {
            fixed,
            messages,
            total: fixed + messages,
            calibration: *self.ratio.lock().unwrap(),
        })
    }

    /// The reductions in force on this branch, oldest first.
    pub fn reductions(&self) -> Vec<ReductionInfo> {
        let store = self.store.lock().unwrap();
        context::active_reductions(&store)
            .into_iter()
            .filter_map(|id| match store.get(id).map(|e| &e.kind) {
                Some(Kind::Reduction(r)) => Some(ReductionInfo {
                    entry: id,
                    mode: r.mode,
                    trigger: r.trigger,
                    covers: r.covers.len(),
                    before_tokens: r.before_tokens,
                    after_tokens: r.after_tokens,
                    summary: r.summary.clone(),
                    stubs: r.stubs.len(),
                    masks: r.masks.len(),
                }),
                _ => None,
            })
            .collect()
    }

    /// Undo a reduction: an append, and the originals are back in the next request.
    pub fn restore(&self, reduction: EntryId) -> Result<()> {
        if !context::active_reductions(&self.store.lock().unwrap()).contains(&reduction) {
            return Err(Error::Reduction(format!(
                "#{reduction} is not a reduction in force"
            )));
        }
        self.append(Kind::Restore { reduction })?;
        Ok(())
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
