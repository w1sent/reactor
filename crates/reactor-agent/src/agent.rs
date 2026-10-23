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
    plan,
};
use crate::classify::StopKind;
use crate::context::{
    self, Item, KEY_IDENTITY, KEY_REPORTING, KEY_SCENARIO, Msg, SessionState, Source,
    estimate_tokens, msg_tokens, project, save_state,
};
use crate::deferral::{self, Candidate};
use crate::entry::{Attachment, EntryId, Kind, Mode, Trigger, Usage};
use crate::error::{Error, Result};
use crate::inbox;
use crate::inspect::ContextPreview;
use crate::llm::{Delta, Llm, LlmRequest, Reply, ToolSpec, backoff, complete_watched};
use crate::prompt::{self, RegistryView};
use crate::skills::{self, Skill};
use crate::store::Store;
use crate::tools::{ToolCtx, ToolOutput, Tools};
use crate::truncate;

/// Replies in a row cut off in the middle of a tool call before the turn stops.
const MAX_CUT_CALLS: usize = 3;

/// What the turn's tool calls have been doing lately: the last call with its result and how many
/// times in a row it has come out the same, and how many calls of one tool in a row have failed.
/// A call is "the same" only if its result is too, so polling something whose output changes is
/// not a loop ([ADR-0060](../../../docs/adr/0060-a-repeated-call-is-first-a-notice.md)).
#[derive(Default)]
struct Repeats {
    last: Option<u64>,
    count: usize,
    failing: Option<String>,
    failures: usize,
}

/// What [`Repeats::note`] counted for one settled call.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Seen {
    /// The same call with the same result, this many times in a row.
    same: usize,
    /// Calls of this tool in a row that failed.
    failures: usize,
}

/// The arguments as text with every object's keys in order, so that `{a, b}` and `{b, a}` are
/// one call (the JSON is kept in the order the model wrote it).
fn canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for k in keys {
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                canonical(&m[k], out);
                out.push(',');
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for x in a {
                canonical(x, out);
                out.push(',');
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

impl Repeats {
    /// Note a call, what it gave, and whether that was an error.
    fn note(&mut self, name: &str, args: &Value, result: &str, is_error: bool) -> Seen {
        use std::hash::{Hash, Hasher};
        let mut text = String::new();
        canonical(args, &mut text);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (name, text, result).hash(&mut h);
        let sig = h.finish();
        if self.last == Some(sig) {
            self.count += 1;
        } else {
            self.last = Some(sig);
            self.count = 1;
        }
        if is_error && self.failing.as_deref() == Some(name) {
            self.failures += 1;
        } else if is_error {
            self.failing = Some(name.to_string());
            self.failures = 1;
        } else {
            self.failing = None;
            self.failures = 0;
        }
        Seen {
            same: self.count,
            failures: self.failures,
        }
    }
}

/// The line added to a result when a threshold is crossed: what happened, and a way on that is
/// the model's to take. At most one per result; the repeat is the more specific.
fn advisory(name: &str, seen: Seen, cfg: &AgentConfig) -> Option<String> {
    if cfg.repeat_notice > 0 && seen.same == cfg.repeat_notice {
        return Some(format!(
            "[reactor] This is the {} identical call to `{name}` with the same result. If you are waiting for something to change, say what; otherwise try something different.",
            ordinal(seen.same)
        ));
    }
    if cfg.failure_notice > 0 && seen.failures == cfg.failure_notice {
        return Some(format!(
            "[reactor] `{name}` has failed {} times in a row. Say what you think is going wrong before the next attempt.",
            seen.failures
        ));
    }
    None
}

fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
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
    /// The same call with the same result this many times in a row adds a line to the result
    /// asking the model to say what it is waiting for or to try something else. Zero turns it off
    /// ([ADR-0060](../../../docs/adr/0060-a-repeated-call-is-first-a-notice.md)).
    pub repeat_notice: usize,
    /// The same tool failing this many calls in a row adds a line to the result. Zero turns it off.
    pub failure_notice: usize,
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
            max_repeats: 8,
            repeat_notice: 3,
            failure_notice: 3,
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
    /// What the provider would count: its last count plus an estimate of what has come since,
    /// else the estimate at the learned ratio.
    pub real: u64,
    /// Real tokens per estimated token (1.0 until a provider reports).
    pub calibration: f64,
}

/// What one request is made from.
struct Gathered {
    /// The case's scenarios first, then the config directory's and the checkout's.
    scenarios: reactor_core::scenarios::Library,
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
    /// The warning that the fixed part of a request is most of the window has been given.
    warned_fixed: std::sync::atomic::AtomicBool,
    /// What starts a child agent, when this session has the `agent` tool.
    children: Option<Arc<dyn crate::tools::agent::Children>>,
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
                warned_fixed: std::sync::atomic::AtomicBool::new(false),
                children: None,
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

    /// This session's scenarios: the case's own, then the config directory's and the
    /// checkout's (ADR-0065).
    fn scenarios_of(&self, store: &Store) -> reactor_core::scenarios::Library {
        crate::tools::scenario::library(
            &self.cfg.paths,
            store.case().dir(),
            &self.cfg.scenarios_dir,
        )
    }

    /// The scenarios this session can start, as the chooser lists them.
    pub fn scenarios(&self) -> reactor_core::scenarios::Library {
        self.scenarios_of(&self.store.lock().unwrap())
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
        let (mut state, items, scenarios) = {
            let store = self.store.lock().unwrap();
            (
                SessionState::load(&store),
                project(&store),
                self.scenarios_of(&store),
            )
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
            scenarios,
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
            self.defer_threshold(&g.settings),
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
            scenarios: &g.scenarios,
            cwd: Some(&self.cfg.cwd),
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
                prompt::Part::Workdir => (Origin::Base, "Working directory"),
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
        // What every request carries whatever the work: the prompt and the tool definitions.
        let fixed = system_tokens
            + segments
                .iter()
                .filter(|s| s.section == Section::Tools)
                .map(|s| s.tokens)
                .sum::<u64>();
        let warnings = budget::fixed_warning(fixed, &self.scaled_budget())
            .into_iter()
            .collect();
        Ok(ContextPreview {
            segments,
            total_tokens,
            calibration: *self.ratio.lock().unwrap(),
            warnings,
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
            verify: eff.verify.unwrap_or(base.verify),
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

    /// What the request costs in real tokens: the count the provider last gave for the start of
    /// it, plus an estimate of what has come since (and of the fixed part having changed), if
    /// nothing has changed the start since; else an estimate of all of it, at the ratio learned
    /// from what the providers reported. The count is closer than the estimate, which is off by
    /// what the tokenizer does to hex and code, and it is there from the first request of a
    /// resumed session ([ADR-0058]).
    pub fn real_tokens(&self, fixed: u64, items: &[Item]) -> u64 {
        let ema = *self.ratio.lock().unwrap();
        let anchor = context::usage_anchor(&self.store.lock().unwrap(), items, &self.llm.name());
        match anchor {
            Some(a) if a.index < items.len() => {
                let trailing: u64 = items[a.index + 1..]
                    .iter()
                    .map(|i| msg_tokens(&i.msg))
                    .sum();
                // The fixed part may have changed since: tools switched on, a block grown.
                let drift = if a.fixed > 0 {
                    fixed as i64 - a.fixed as i64
                } else {
                    0
                };
                a.tokens + ((trailing as i64 + drift).max(0) as f64 * ema) as u64
            }
            _ => (context_tokens(fixed, items) as f64 * ema) as u64,
        }
    }

    /// Real tokens per estimated token for this request, from [`Agent::real_tokens`].
    fn ratio_for(&self, fixed: u64, items: &[Item]) -> f64 {
        let est = context_tokens(fixed, items).max(1);
        (self.real_tokens(fixed, items) as f64 / est as f64).clamp(0.25, 4.0)
    }

    /// The budget in the units this request's estimates are in.
    fn scaled_budget_for(&self, fixed: u64, items: &[Item]) -> BudgetConfig {
        self.effective_budget().scaled(self.ratio_for(fixed, items))
    }

    /// A person asking to reduce wants *more* gone than the budget would take: the keep
    /// window shrinks to the latest group of work. Their last message stays in view.
    fn manual_budget(&self, fixed: u64, items: &[Item]) -> BudgetConfig {
        BudgetConfig {
            keep: 0.0,
            ..self.scaled_budget_for(fixed, items)
        }
    }

    /// What a reduction would do right now, without doing it.
    pub async fn preview(&self, mode: Mode) -> Result<Plan> {
        let (_, fixed, items) = self.build().await?;
        plan(fixed, &items, &self.manual_budget(fixed, &items), mode)
            .map_err(|n| Error::Reduction(n.to_string()))
    }

    /// Reduce now, at a person's request. A manual reduction means *summarize* in every
    /// mode but `fade` (a manual fade exists for a burst of dumps that should just go).
    ///
    /// `focus` is what the person wants the summary to keep ("the C2 findings"); a fade has no
    /// summary and ignores it.
    pub async fn reduce_now(&self, mode: Mode, focus: Option<&str>) -> Result<EntryId> {
        let (base, fixed, items) = self.build().await?;
        let mut p = plan(fixed, &items, &self.manual_budget(fixed, &items), mode)
            .map_err(|n| Error::Reduction(n.to_string()))?;
        p.focus = focus
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(str::to_string);
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
                // A person who asked for a reduction by hand gets the second look.
                req.verify = trigger == Trigger::Manual || self.effective_budget().verify;
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
            let cfg = self.scaled_budget_for(fixed, &items);
            // Whether it is time is a question of real tokens against the real threshold: no
            // ratio in between, which is clamped and could hide a count far from the estimate.
            if self.real_tokens(fixed, &items) <= self.effective_budget().trigger() {
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

    /// What a group of tools may cost before it is left for `tool_search`: the person's setting,
    /// or a hundredth of the window (ADR-0058).
    fn defer_threshold(&self, settings: &Settings) -> usize {
        settings
            .tool_search
            .for_window(self.effective_budget().window)
    }

    /// The same, for a frontend to say.
    pub fn deferral_threshold(&self) -> usize {
        self.defer_threshold(&self.settings())
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

    /// Say, once, that the fixed part of a request is most of what the window leaves. The work
    /// goes on: a small window should work, with less room.
    fn note_cramped(&self, fixed: u64) {
        if let Some(w) = budget::fixed_warning(fixed, &self.scaled_budget())
            && !self
                .warned_fixed
                .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            self.emit(Event::Notice(w));
        }
    }

    /// Set a request's reply limit and window. Returns whether the limit had to be cut to
    /// the room the window leaves, which a reduction can widen.
    fn size_request(&self, req: &mut LlmRequest, fixed: u64, items: &[Item], boost: bool) -> bool {
        let b = self.effective_budget();
        let used = self.real_tokens(fixed, items);
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
        let mut cfg = self.scaled_budget_for(fixed, &items);
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
    ) -> Result<(Reply, u64, u64)> {
        let (mut overflowed, mut lengthened, mut tries) = (false, false, 0usize);
        loop {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            self.ensure_fits(in_a_row, outcome, cancel).await?;
            let (mut req, fixed, items) = self.build().await?;
            let cramped = self.size_request(&mut req, fixed, &items, lengthened);
            // What the request is estimated at, tools and all, to be set against what the
            // provider says it was.
            let estimated: u64 = context_tokens(fixed, &items);
            self.note_cramped(fixed);

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
                    // The provider's filter (or a model's refusal) stopped the reply before it
                    // said or did anything. There is nothing to keep, and the same request would
                    // be stopped again, so it is not sent again: like a blocked request.
                    if reply.stop_kind() == StopKind::Filtered && !reply.has_output() {
                        return Err(Error::Blocked(
                            "the provider's filter stopped the reply before it said anything. The same request would be stopped again, so it was not retried. Rephrase it, or choose another model.".into(),
                        ));
                    }
                    return Ok((reply, estimated, fixed));
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
        self.guarded(self.prompt_from(text, None, cancel)).await
    }

    /// Run one turn on the oldest input waiting in the inbox, whatever its class: the turn it was
    /// waiting for is over. `None` when nothing waits
    /// ([ADR-0061](../../../docs/adr/0061-typed-input-is-a-fact-in-the-log-before-it-is-sent.md)).
    pub async fn prompt_pending(&self, cancel: CancellationToken) -> Option<Result<Outcome>> {
        let first = inbox::pending(&self.store.lock().unwrap()).into_iter().next()?;
        Some(
            self.guarded(self.prompt_from(&first.text, Some(first.item.as_str()), cancel))
                .await,
        )
    }

    /// Run a turn so that a panic in it ends the turn with an error instead of the thread: the
    /// frontend hears that it ended, calls left without a result are answered, and the log is
    /// as it was. Without this a turn that panics never reports, and looks like one that never
    /// finishes.
    async fn guarded<F>(&self, turn: F) -> Result<Outcome>
    where
        F: std::future::Future<Output = Result<Outcome>>,
    {
        use futures::FutureExt;
        match std::panic::AssertUnwindSafe(turn).catch_unwind().await {
            Ok(result) => result,
            Err(panic) => {
                let why = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "a panic".into());
                let _ = self.answer_open_calls();
                Err(Error::Model(format!(
                    "internal error: {why}. The turn was ended and the log is intact; this is a bug in REactor."
                )))
            }
        }
    }

    /// Answer the calls of the last reply that have no result, so the log stays replayable.
    fn answer_open_calls(&self) -> Result<()> {
        let open: Vec<(String, String, Value)> = {
            let store = self
                .store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let branch = store.branch();
            let Some(at) = branch
                .iter()
                .rposition(|e| matches!(e.kind, Kind::Assistant { .. }))
            else {
                return Ok(());
            };
            let answered: std::collections::HashSet<&str> = branch[at..]
                .iter()
                .filter_map(|e| match &e.kind {
                    Kind::ToolResult { call_id, .. } => Some(call_id.as_str()),
                    _ => None,
                })
                .collect();
            match &branch[at].kind {
                Kind::Assistant { blocks, .. } => blocks
                    .iter()
                    .filter_map(|b| match b {
                        crate::entry::Block::ToolCall {
                            id,
                            name,
                            arguments,
                        } if !answered.contains(id.as_str()) => {
                            Some((id.clone(), name.clone(), arguments.clone()))
                        }
                        _ => None,
                    })
                    .collect(),
                _ => vec![],
            }
        };
        self.fill_results(&open, "not run: an internal error ended the turn")
    }

    /// Admit typed input. `false` if `item` was admitted already.
    pub fn admit(&self, item: &str, class: inbox::Class, text: &str) -> Result<bool> {
        let before = self.store.lock().unwrap().head();
        let added = inbox::admit(&mut self.store.lock().unwrap(), item, class, text)?;
        if added {
            let head = self.store.lock().unwrap().head();
            if head != before {
                self.emit(Event::Appended(head));
            }
        }
        Ok(added)
    }

    /// What waits, oldest first.
    pub fn pending_inbox(&self) -> Vec<inbox::Pending> {
        inbox::pending(&self.store.lock().unwrap())
    }

    /// Take back one item, or the latest when `item` is `None`. Its text, if it was waiting.
    pub fn withdraw(&self, item: Option<&str>) -> Result<Option<String>> {
        let mut store = self.store.lock().unwrap();
        let waiting = inbox::pending(&store);
        let Some(p) = (match item {
            Some(i) => waiting.into_iter().find(|p| p.item == i),
            None => waiting.into_iter().next_back(),
        }) else {
            return Ok(None);
        };
        let id = inbox::finish(&mut store, &p.item, inbox::End::Cancelled)?;
        drop(store);
        self.emit(Event::Appended(id));
        Ok(Some(p.text))
    }

    /// Take back everything that waits, oldest first.
    pub fn withdraw_all(&self) -> Result<Vec<String>> {
        let mut texts = Vec::new();
        for p in self.pending_inbox() {
            if let Some(t) = self.withdraw(Some(&p.item))? {
                texts.push(t);
            }
        }
        Ok(texts)
    }

    /// Deliver the steering input that is waiting: each becomes a user message and the inbox
    /// says so. How many were delivered.
    async fn deliver_steers(&self) -> Result<usize> {
        let steers: Vec<_> = inbox::pending(&self.store.lock().unwrap())
            .into_iter()
            .filter(|p| p.class == inbox::Class::Steer)
            .collect();
        for p in &steers {
            self.attach_references(&p.text).await?;
            let user = self.append(Kind::User {
                text: p.text.clone(),
            })?;
            let end = inbox::finish(
                &mut self.store.lock().unwrap(),
                &p.item,
                inbox::End::Delivered(user),
            )?;
            self.emit(Event::Appended(end));
        }
        Ok(steers.len())
    }

    async fn prompt_from(
        &self,
        text: &str,
        from_inbox: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<Outcome> {
        let mut from_inbox = from_inbox;
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
            let state = SessionState::load(&store);
            if let Some(mut sc) = state.scenario.clone() {
                let at = sc.pending_summary;
                if scenario::record_summary(&mut sc, text) {
                    save_state(&mut store, KEY_SCENARIO, serde_json::to_value(&sc).unwrap())?;
                    if let Some((at, summary)) = at.and_then(|i| Some((i, sc.summaries.get(i)?)))
                    {
                        let title = self
                            .scenarios_of(&store)
                            .load(&sc.scenario_id)
                            .get(at)
                            .map(|p| p.title.clone())
                            .unwrap_or_default();
                        crate::tools::scenario::keep_summary(
                            &self.cfg.paths,
                            &state,
                            store.case(),
                            store.session(),
                            store.head(),
                            Author::analyst(),
                            &sc.scenario_id,
                            at,
                            &title,
                            summary,
                        );
                    }
                }
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
            let user = self.append(Kind::User { text: next.clone() })?;
            // Input from the inbox is delivered once its message is in the log.
            if let Some(item) = from_inbox.take() {
                let end = inbox::finish(
                    &mut self.store.lock().unwrap(),
                    item,
                    inbox::End::Delivered(user),
                )?;
                self.emit(Event::Appended(end));
            }

            loop {
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                let (reply, estimated, fixed) =
                    self.ask_model(&cancel, &mut in_a_row, &mut outcome).await?;
                outcome.rounds += 1;
                // The fixed part is kept with the count, so a later count can allow for it having
                // changed.
                let usage = reply.usage.map(|mut u| {
                    u.fixed_estimate = fixed;
                    u
                });
                if let Some(u) = usage {
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
                    usage,
                    stop: reply.stop.clone(),
                })?;

                // A reply the output limit cut off, or the provider's filter stopped, may hold a
                // tool call whose arguments are unfinished: none of its calls is run. Each is
                // answered, so the log stays valid, and the model is told why and asks again.
                let stopped = reply.stop_kind();
                let by_filter = stopped == StopKind::Filtered;
                if matches!(stopped, StopKind::Length | StopKind::Filtered) && !calls.is_empty() {
                    cut_calls += 1;
                    self.fill_results(
                        &calls,
                        if by_filter {
                            "not run: the provider's filter stopped your reply before this call was complete. Take a different approach to the step."
                        } else {
                            "not run: your reply was cut off by the output limit before this call was complete. Make a smaller call, or split the work over several."
                        },
                    )?;
                    if cut_calls >= MAX_CUT_CALLS {
                        return Err(if by_filter {
                            Error::Blocked(format!(
                                "stopped: {MAX_CUT_CALLS} replies in a row were stopped by the provider's filter in the middle of a tool call. Rephrase the request, or choose another model."
                            ))
                        } else {
                            Error::Model(format!(
                                "stopped: {MAX_CUT_CALLS} replies in a row were cut off by the output limit in the middle of a tool call. Raise the output limit, or ask for a smaller step."
                            ))
                        });
                    }
                    self.emit(Event::Notice(
                        if by_filter {
                            "the provider's filter stopped the reply in the middle of a tool call; the call was not run"
                        } else {
                            "the reply was cut off by the output limit in the middle of a tool call; the call was not run"
                        }
                        .into(),
                    ));
                    continue;
                }
                cut_calls = 0;
                if stopped == StopKind::Length {
                    self.emit(Event::Notice(
                        "the reply was cut off by the output limit".into(),
                    ));
                } else if by_filter {
                    // What it said before the filter stopped it is kept.
                    self.emit(Event::Notice(
                        "the provider's filter stopped the reply; what it said is kept".into(),
                    ));
                }

                if calls.is_empty() {
                    // Input typed while the reply streamed is the next step, not the next turn.
                    if self.deliver_steers().await? > 0 {
                        continue;
                    }
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
                                // Input still waiting goes with the turn to the new branch.
                                let waiting = inbox::pending(&store);
                                let last_user = store
                                    .branch()
                                    .iter()
                                    .rev()
                                    .find(|e| matches!(e.kind, Kind::User { .. }))
                                    .map(|e| e.id);
                                if let Some(id) = last_user {
                                    store.set_head(id)?;
                                    for p in &waiting {
                                        inbox::admit(&mut store, &p.item, p.class, &p.text)?;
                                    }
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
                // A step boundary: the calls have settled and the next request has not been made.
                self.deliver_steers().await?;
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

    /// Run one tool call: its tool, or an MCP server's. The limit says how much of the output
    /// the model will see.
    async fn call_tool(
        &self,
        id: &str,
        name: &str,
        args: &Value,
        cancel: &CancellationToken,
        default: truncate::Limit,
    ) -> (ToolOutput, truncate::Limit) {
        // Our tools take a limit; an MCP tool's parameters are its server's.
        let ours = self.tools.find(name).is_some();
        let limit = if ours {
            truncate::Limit::for_call(args, default)
        } else {
            default
        };
        let events = self.events.clone();
        let call_id = id.to_string();
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
            children: self.children.clone(),
        };
        let out = match (self.tools.find(name), &self.cfg.mcp) {
            (Some(tool), _) => tool.call(args.clone(), &ctx).await,
            // Active or not: deactivating only stops a tool being advertised (ADR-0007).
            (None, Some(host)) if host.knows(name) => host.call(name, args.clone(), cancel).await,
            _ => ToolOutput::err(format!("unknown tool `{name}`")),
        };
        (out, limit)
    }

    /// The `agent` calls of a reply that run together, when the settings allow it: the indexes
    /// of those calls, and how many at once. Children that are started in the background are
    /// not among them: they return at once.
    fn parallel_children(&self, calls: &[(String, String, Value)]) -> (Vec<usize>, usize) {
        let settings = self.settings().children;
        if !settings.parallel || self.children.is_none() {
            return (vec![], 1);
        }
        let which: Vec<usize> = calls
            .iter()
            .enumerate()
            .filter(|(_, (_, name, args))| {
                name == crate::tools::agent::TOOL
                    && !args
                        .get("background")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
            })
            .map(|(i, _)| i)
            .collect();
        if which.len() < 2 {
            return (vec![], 1);
        }
        (which, settings.max_parallel.max(1))
    }

    async fn run_tools(
        &self,
        calls: &[(String, String, Value)],
        cancel: &CancellationToken,
        in_a_row: &mut usize,
        outcome: &mut Outcome,
        repeats: &mut Repeats,
    ) -> Result<()> {
        use futures::StreamExt;
        let default = self.output_limit(&self.settings());

        // Children asked for together run together; their results are entered in the order they
        // were asked for, like every other call's (ADR-0063).
        let (together, at_once) = self.parallel_children(calls);
        let mut finished: std::collections::HashMap<usize, (ToolOutput, truncate::Limit)> =
            Default::default();
        if !together.is_empty() && !cancel.is_cancelled() {
            for &i in &together {
                let (id, name, args) = &calls[i];
                self.emit(Event::ToolStart {
                    id: id.clone(),
                    name: name.clone(),
                    args: args.clone(),
                });
            }
            // Built first and run after: a stream over a borrowing iterator makes the compiler
            // lose track of lifetimes through the child agent's own turn.
            let running: Vec<_> = together
                .iter()
                .map(|&i| {
                    let (id, name, args) = &calls[i];
                    async move { (i, self.call_tool(id, name, args, cancel, default).await) }
                })
                .collect();
            finished = futures::stream::iter(running)
                .buffer_unordered(at_once)
                .collect()
                .await;
        }

        for (i, (id, name, args)) in calls.iter().enumerate() {
            if cancel.is_cancelled() {
                self.fill_results(&calls[i..], "not run: the turn was cancelled")?;
                return Err(Error::Cancelled);
            }
            let (out, limit) = match finished.remove(&i) {
                Some(done) => done,
                None => {
                    self.emit(Event::ToolStart {
                        id: id.clone(),
                        name: name.clone(),
                        args: args.clone(),
                    });
                    self.call_tool(id, name, args, cancel, default).await
                }
            };
            let is_error = out.is_error;
            let kind = self.finalize(id, name, out, limit)?;
            let mut kind = kind;
            let seen = match &mut kind {
                Kind::ToolResult { content, .. } => {
                    let seen = repeats.note(name, args, content, is_error);
                    // Appended after the result was cut, so it is never cut itself.
                    if let Some(line) = advisory(name, seen, &self.cfg) {
                        content.push_str("\n\n");
                        content.push_str(&line);
                        self.emit(Event::Notice(line));
                    }
                    seen
                }
                _ => Seen::default(),
            };
            let entry = self.append(kind)?;
            self.emit(Event::ToolEnd {
                id: id.clone(),
                entry,
                is_error,
            });

            // The same call, giving the same result, again and again: nothing is going to change.
            if self.cfg.max_repeats > 0 && seen.same >= self.cfg.max_repeats {
                self.fill_results(
                    &calls[i + 1..],
                    "not run: the turn was stopped because the same call kept giving the same result",
                )?;
                return Err(Error::Model(format!(
                    "stopped: `{name}` was called {} times in a row with the same arguments ({}) and gave the same result each time. Say what to try instead, or ask for something different.",
                    seen.same,
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
                let Some(e) = manifest::command(&mut m, name, args) else {
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
                let lib = self.scenarios_of(&store);
                let mut sc = scenario::Scenario {
                    lib: &lib,
                    state: state.scenario.clone(),
                };
                let mut e = sc.command(args);
                // The question a scenario is started with is the session's goal (ADR-0065).
                if let Some(goal) = &e.goal {
                    let mut m = state.manifest.clone();
                    if let Some(g) = manifest::command(&mut m, "goal", goal)
                        && g.persist
                    {
                        context::save_manifest(&mut store, &m, &Author::analyst())?;
                    }
                }
                // A scenario that starts reads what earlier ones concluded first.
                if args.split_whitespace().next() == Some("start")
                    && let Some(m) = e.messages.iter_mut().find(|m| m.trigger_turn)
                {
                    m.content.push_str(&crate::tools::scenario::earlier_conclusions(
                        &self.cfg.paths,
                        &state,
                        store.case(),
                    ));
                }
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
            "agents" => {
                let words: Vec<&str> = args.split_whitespace().collect();
                match words.as_slice() {
                    [] => {
                        let running = self.running_children();
                        if running.is_empty() {
                            result
                                .notices
                                .push(reactor_context::Notice::info("no agent is running"));
                        }
                        for c in running {
                            result.notices.push(reactor_context::Notice::info(format!(
                                "session {}{} · {}s · {}",
                                c.session,
                                if c.background { " (background)" } else { "" },
                                c.started.elapsed().as_secs(),
                                c.task.lines().next().unwrap_or("").chars().take(100).collect::<String>()
                            )));
                        }
                    }
                    ["stop"] | ["stop", "all"] => {
                        let n = self.stop_children(None);
                        result.notices.push(reactor_context::Notice::info(format!(
                            "stopping {n} agent{}",
                            if n == 1 { "" } else { "s" }
                        )));
                    }
                    ["stop", session] => {
                        let session = session.trim_start_matches('#');
                        let n = self.stop_children(Some(session));
                        result.notices.push(if n == 0 {
                            reactor_context::Notice::warning(format!(
                                "no agent of session {session} is running"
                            ))
                        } else {
                            reactor_context::Notice::info(format!("stopping the agent of session {session}"))
                        });
                    }
                    _ => return Err(Error::Store("agents: | stop [all|<session>]".into())),
                }
            }
            "children" => {
                let c = &mut settings.children;
                let words: Vec<&str> = args.split_whitespace().collect();
                let mut changed = true;
                match words.as_slice() {
                    [] => changed = false,
                    ["parallel"] | ["parallel", "on"] => c.parallel = true,
                    ["serial"] | ["parallel", "off"] => c.parallel = false,
                    ["max", n] => match n.parse::<usize>() {
                        Ok(n) if n >= 1 => {
                            c.max_parallel = n.min(reactor_context::settings::ChildrenSettings::MAX)
                        }
                        _ => {
                            return Err(Error::Store(
                                "children max needs a number of at least 1".into(),
                            ));
                        }
                    },
                    _ => {
                        return Err(Error::Store(
                            "children: parallel [on|off] | serial | max <n>".into(),
                        ));
                    }
                }
                let line = format!(
                    "children: {}, at most {} together",
                    if c.parallel {
                        "asked for together, they run together"
                    } else {
                        "they run one after another"
                    },
                    c.max_parallel
                );
                if changed {
                    settings.save(&self.cfg.paths)?;
                }
                result.notices.push(reactor_context::Notice::info(line));
            }
            "cache" => result
                .notices
                .push(reactor_context::Notice::info(
                    context::cache_report(&store).as_text(),
                )),
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
            real: self.real_tokens(fixed, &items),
            calibration: self.ratio_for(fixed, &items),
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


// -- children (ADR-0062, ADR-0063) ---------------------------------------------------------------

impl<L: Llm, S: Summarizer> Agent<L, S> {
    /// The child agents running now, oldest first.
    pub fn running_children(&self) -> Vec<crate::tools::agent::ChildInfo> {
        self.children.as_ref().map(|c| c.list()).unwrap_or_default()
    }

    /// Stop the running child of `session`, or all of them. How many were stopped.
    pub fn stop_children(&self, session: Option<&str>) -> usize {
        self.children.as_ref().map_or(0, |c| c.stop(session))
    }
}

impl<L: Llm + Clone + 'static, S: Summarizer + Clone + 'static> Agent<L, S> {
    /// Give this agent the `agent` and `wait` tools. A child is built like its parent, from the
    /// same model and summarizer, and has the tools the parent has now, without those two: a
    /// child does not start children.
    pub fn enable_children(&mut self) {
        let tools = self
            .tools
            .without(crate::tools::agent::TOOL)
            .without(crate::tools::agent::WAIT);
        self.children = Some(Arc::new(Spawner {
            llm: self.llm.clone(),
            summarizer: self.summarizer.clone(),
            tools: tools.clone(),
            cfg: self.cfg.clone(),
            running: Arc::new(Running::default()),
            live: Arc::new(Mutex::new(Vec::new())),
        }));
        self.tools = tools
            .with(crate::tools::agent::AgentTool)
            .with(crate::tools::agent::WaitTool);
    }
}

/// The children started in the background: which are running, and the reports of those that
/// finished and have not been waited for.
#[derive(Default)]
struct Running {
    state: Mutex<RunningState>,
    changed: tokio::sync::Notify,
}

#[derive(Default)]
struct RunningState {
    running: Vec<String>,
    done: std::collections::VecDeque<(String, ToolOutput)>,
}

struct Spawner<L, S> {
    llm: L,
    summarizer: S,
    tools: Tools,
    cfg: AgentConfig,
    running: Arc<Running>,
    /// Every child that is running, foreground or not, with the means to stop it.
    live: Arc<Mutex<Vec<Live>>>,
}

struct Live {
    info: crate::tools::agent::ChildInfo,
    cancel: CancellationToken,
}

/// Takes a child off the list when it ends, however it ends.
struct OnList {
    live: Arc<Mutex<Vec<Live>>>,
    session: String,
}

impl Drop for OnList {
    fn drop(&mut self) {
        if let Ok(mut l) = self.live.lock() {
            l.retain(|c| c.info.session != self.session);
        }
    }
}

/// The origin of a session started by another: a state entry of the child's own log.
pub const KEY_ORIGIN: &str = "origin";

/// A child that has a session and tools and has not started.
struct Prepared {
    store: Store,
    session: String,
    tools: Tools,
    unknown: Vec<String>,
}

type Emit = Arc<dyn Fn(&str) + Send + Sync>;

impl<L: Llm + Clone + 'static, S: Summarizer + Clone + 'static> Spawner<L, S> {
    fn list_child(
        &self,
        session: &str,
        task: &str,
        background: bool,
        cancel: CancellationToken,
    ) -> OnList {
        self.live.lock().unwrap().push(Live {
            info: crate::tools::agent::ChildInfo {
                session: session.to_string(),
                task: task.to_string(),
                background,
                started: std::time::Instant::now(),
            },
            cancel,
        });
        OnList {
            live: self.live.clone(),
            session: session.to_string(),
        }
    }

    fn prepare(
        &self,
        request: &crate::tools::agent::ChildRequest,
        ctx: &ToolCtx,
    ) -> std::result::Result<Prepared, ToolOutput> {
        let (case, parent) = {
            let s = ctx.store.lock().unwrap();
            (s.case().clone(), s.session().to_string())
        };
        let mut unknown = Vec::new();
        let tools = match &request.tools {
            Some(names) => {
                let (kept, missing) = self.tools.only(names);
                unknown = missing;
                if kept.is_empty() {
                    return Err(ToolOutput::err(format!(
                        "none of those tools exist here: {}",
                        names.join(", ")
                    )));
                }
                kept
            }
            None => self.tools.clone(),
        };
        let mut store = Store::create_child(&case, &parent)
            .map_err(|e| ToolOutput::err(format!("could not start a session for it: {e}")))?;
        store
            .append(Kind::Custom {
                key: KEY_ORIGIN.into(),
                data: serde_json::json!({ "parent": parent, "task": request.task }),
            })
            .map_err(|e| ToolOutput::err(format!("could not start a session for it: {e}")))?;
        Ok(Prepared {
            session: store.session().to_string(),
            store,
            tools,
            unknown,
        })
    }

    /// Run a prepared child to the end and make its report. Owns everything it needs, so it can
    /// be run in the background.
    fn drive(
        &self,
        prepared: Prepared,
        task: String,
        emit: Emit,
        cancel: CancellationToken,
    ) -> impl std::future::Future<Output = ToolOutput> + Send + 'static {
        let (llm, summarizer, mut cfg) =
            (self.llm.clone(), self.summarizer.clone(), self.cfg.clone());
        // A child that keeps going is stopped, and what it had said is returned.
        let rounds = Settings::load(&cfg.paths).children.max_rounds;
        if rounds > 0 {
            cfg.max_rounds = cfg.max_rounds.min(rounds);
        }
        async move {
            let Prepared {
                store,
                session,
                tools,
                unknown,
            } = prepared;
            let (child, mut events) = Agent::new(llm, summarizer, store, tools, cfg);
            // What the child is doing shows on the parent's tool card as it happens.
            let progress = tokio::spawn(async move {
                while let Some(e) = events.recv().await {
                    match e {
                        Event::ToolStart { name, .. } => emit(&format!("[agent] {name}\n")),
                        Event::Notice(n) => emit(&format!("[agent] {n}\n")),
                        _ => {}
                    }
                }
            });
            let result = child.prompt(&task, cancel).await;
            let (report, usage) = {
                let store = child.store();
                let store = store.lock().unwrap();
                let report = store.branch().iter().rev().find_map(|e| match &e.kind {
                    Kind::Assistant { blocks, .. } => {
                        let text: String = blocks
                            .iter()
                            .filter_map(|b| match b {
                                crate::entry::Block::Text { text } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        (!text.trim().is_empty()).then_some(text)
                    }
                    _ => None,
                });
                let mut usage = Usage::default();
                for e in store.all() {
                    if let Kind::Assistant { usage: Some(u), .. } = &e.kind {
                        usage.input_tokens += u.input_tokens;
                        usage.output_tokens += u.output_tokens;
                        usage.cached_input_tokens += u.cached_input_tokens;
                    }
                }
                (report, usage)
            };
            drop(child);
            let _ = progress.await;
            let footer = |rounds: usize, calls: usize| {
                format!(
                    "[agent session {session}: {rounds} rounds, {calls} tool calls, {} input ({} cached) and {} output tokens]{}",
                    usage.input_tokens,
                    usage.cached_input_tokens,
                    usage.output_tokens,
                    if unknown.is_empty() {
                        String::new()
                    } else {
                        format!(" Not tools here, left out: {}.", unknown.join(", "))
                    }
                )
            };
            match result {
                Ok(o) => match report {
                    Some(text) => {
                        ToolOutput::ok(format!("{text}\n\n{}", footer(o.rounds, o.tool_calls)))
                    }
                    None => ToolOutput::err(format!(
                        "it finished without a report. {}",
                        footer(o.rounds, o.tool_calls)
                    )),
                },
                Err(Error::Cancelled) => ToolOutput::err(format!(
                    "cancelled before it finished; what it did is in session {session}.{} {}",
                    report
                        .map(|r| format!("\nWhat it had said last:\n{r}\n"))
                        .unwrap_or_default(),
                    footer(0, 0)
                )),
                Err(e) => ToolOutput::err(format!(
                    "it stopped: {e}.{} {}",
                    report
                        .map(|r| format!("\nWhat it had said last:\n{r}\n"))
                        .unwrap_or_default(),
                    footer(0, 0)
                )),
            }
        }
    }
}

impl<L: Llm + Clone + 'static, S: Summarizer + Clone + 'static> crate::tools::agent::Children
    for Spawner<L, S>
{
    fn run<'a>(
        &'a self,
        request: crate::tools::agent::ChildRequest,
        ctx: &'a ToolCtx,
    ) -> crate::tools::BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let max = Settings::load(&self.cfg.paths).children.max_parallel;
            if request.background && self.running.state.lock().unwrap().running.len() >= max {
                return ToolOutput::err(format!(
                    "{max} agents are running in the background already. Wait for one first."
                ));
            }
            let prepared = match self.prepare(&request, ctx) {
                Ok(p) => p,
                Err(e) => return e,
            };
            // Foreground or not, a child can be listed and stopped on its own. The parent's cancel
            // is still the child's: this one is derived from it.
            let cancel = ctx.cancel.child_token();
            let on_list = self.list_child(
                &prepared.session,
                &request.task,
                request.background,
                cancel.clone(),
            );
            if !request.background {
                let out = self
                    .drive(prepared, request.task, ctx.emit.clone(), cancel)
                    .await;
                drop(on_list);
                return out;
            }
            // In the background it has no tool card to report to: its progress is its session.
            let session = prepared.session.clone();
            self.running
                .state
                .lock()
                .unwrap()
                .running
                .push(session.clone());
            let work = self.drive(
                prepared,
                request.task,
                Arc::new(|_: &str| {}),
                cancel,
            );
            let running = self.running.clone();
            let id = session.clone();
            tokio::spawn(async move {
                let out = work.await;
                drop(on_list);
                {
                    let mut s = running.state.lock().unwrap();
                    s.running.retain(|r| *r != id);
                    s.done.push_back((id, out));
                }
                running.changed.notify_waiters();
            });
            ToolOutput::ok(format!(
                "started in the background as session {session}. Collect its report with `wait`."
            ))
        })
    }

    fn list(&self) -> Vec<crate::tools::agent::ChildInfo> {
        self.live
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.info.clone())
            .collect()
    }

    fn stop(&self, session: Option<&str>) -> usize {
        let live = self.live.lock().unwrap();
        let mut n = 0;
        for c in live.iter() {
            if session.is_none_or(|s| s == c.info.session) {
                c.cancel.cancel();
                n += 1;
            }
        }
        n
    }

    fn wait<'a>(
        &'a self,
        session: Option<String>,
        ctx: &'a ToolCtx,
    ) -> crate::tools::BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            loop {
                // Ask to be woken before looking, so a child that finishes in between is not missed.
                let woken = self.running.changed.notified();
                tokio::pin!(woken);
                woken.as_mut().enable();
                {
                    let mut s = self.running.state.lock().unwrap();
                    let at = s.done.iter().position(|(id, _)| match &session {
                        Some(want) => id == want,
                        None => true,
                    });
                    if let Some(at) = at {
                        return s.done.remove(at).map(|(_, out)| out).unwrap();
                    }
                    let any = match &session {
                        Some(want) => s.running.contains(want),
                        None => !s.running.is_empty(),
                    };
                    if !any {
                        return ToolOutput::err(match &session {
                            Some(want) => format!(
                                "no agent `{want}` is running or waiting to be collected here"
                            ),
                            None => "no agent is running in the background".to_string(),
                        });
                    }
                }
                tokio::select! {
                    _ = &mut woken => {}
                    _ = ctx.cancel.cancelled() => {
                        return ToolOutput::err("cancelled while waiting; the agents keep their sessions");
                    }
                }
            }
        })
    }
}
