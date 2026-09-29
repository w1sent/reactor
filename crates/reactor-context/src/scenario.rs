//! Multi-phase analysis scenarios: a run of prose briefings, one per phase, that
//! advance on a tool result rather than on a marker or a heuristic (ADR-0009).
//! Port of `extensions/scenario/` (ADR-0017, ADR-0030).
//!
//! A scenario is a directory of `NN-name.md` files. Each has two frontmatter
//! fields, hand-parsed — `title:` and `toolset:` — and everything else about a
//! phase (which tools just became relevant, what not to start yet) is prose the
//! author writes into the body. Activating a phase's toolset is *additive and
//! advisory* (ADR-0007): the effect asks the caller to enable it, and a caller
//! that cannot still lets the phase advance.

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::notice::Notice;
use crate::text::js_trim;

pub const ENTRY_TYPE: &str = "reactor-scenario";
pub const TOOL_NAME: &str = "reactor_phase_complete";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub title: String,
    pub toolset: Option<String>,
    pub body: String,
}

/// The scenario in progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub scenario_id: String,
    /// Index into that scenario's steps — the phase currently in progress.
    pub step_index: usize,
    /// One per completed phase, parallel to `steps[0..step_index]`.
    pub summaries: Vec<String>,
}

// -- reading scenarios ----------------------------------------------------------

/// Scenario ids: the subdirectories of `dir`, sorted.
pub fn list_scenarios(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    ids.sort();
    ids
}

fn frontmatter_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)\A---\r?\n(.*?)\r?\n---\r?\n?(.*)\z").unwrap())
}

fn field_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // `.` in JS excludes the four line terminators, not just \n.
    RE.get_or_init(|| Regex::new(r"\A(\w+):\s*([^\r\n\u{2028}\u{2029}]*)\z").unwrap())
}

pub fn parse_step(raw: &str) -> Step {
    let Some(m) = frontmatter_re().captures(raw) else {
        return Step { title: String::new(), toolset: None, body: js_trim(raw).to_string() };
    };
    let (mut title, mut toolset) = (String::new(), String::new());
    for line in m[1].split('\n') {
        if let Some(kv) = field_re().captures(line) {
            let value = js_trim(&kv[2]).to_string();
            match &kv[1] {
                "title" => title = value,
                "toolset" => toolset = value,
                _ => {}
            }
        }
    }
    Step { title, toolset: (!toolset.is_empty()).then_some(toolset), body: js_trim(&m[2]).to_string() }
}

/// A scenario's phases, in file-name order. Unknown or unreadable is empty.
pub fn load_steps(dir: &Path, id: &str) -> Vec<Step> {
    let base = dir.join(id);
    let Ok(entries) = std::fs::read_dir(&base) else { return vec![] };
    let mut files: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| f.ends_with(".md"))
        .collect();
    files.sort();
    files
        .iter()
        .map(|f| parse_step(&std::fs::read_to_string(base.join(f)).unwrap_or_default()))
        .collect()
}

/// `## Phase 2/4: Static analysis`, then the phase's own body verbatim.
pub fn briefing(steps: &[Step], index: usize) -> String {
    let step = &steps[index];
    let title = if step.title.is_empty() { format!("step {}", index + 1) } else { step.title.clone() };
    format!("## Phase {}/{}: {}\n\n{}", index + 1, steps.len(), title, step.body)
}

// -- running one ------------------------------------------------------------------

/// What happens to the stored state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Persist {
    Nothing,
    Set(State),
    /// Store "no scenario" — a `stop` or a finish, so that replaying the entries
    /// ends with nothing active.
    Clear,
}

/// A message to put into the session; the model is prompted after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub content: String,
    pub trigger_turn: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effects {
    pub notices: Vec<Notice>,
    pub messages: Vec<Message>,
    pub persist: Persist,
    /// Toolsets to enable (advisory; failure must not stop the phase).
    pub activate_toolsets: Vec<String>,
}

impl Effects {
    fn none() -> Self {
        Effects { notices: vec![], messages: vec![], persist: Persist::Nothing, activate_toolsets: vec![] }
    }
    fn notice(n: Notice) -> Self {
        Effects { notices: vec![n], ..Effects::none() }
    }
}

/// The scenario runner: a state and a directory of scenarios.
#[derive(Debug, Clone)]
pub struct Scenario<'a> {
    pub dir: &'a Path,
    pub state: Option<State>,
}

/// The result of finishing a phase: what to tell the model, and what changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advance {
    pub text: String,
    pub persist: Persist,
    pub activate_toolset: Option<String>,
}

impl Scenario<'_> {
    /// Whether `reactor_phase_complete` is advertised: exactly while a scenario runs.
    pub fn tool_advertised(&self) -> bool {
        self.state.is_some()
    }

    /// Shared by the tool and `/reactor-scenario next`: record the summary, move
    /// to the next phase (or end), and say what the model should see.
    pub fn advance(&mut self, summary: &str) -> Advance {
        let Some(state) = self.state.clone() else {
            return Advance {
                text: "reactor: no scenario is active -- start one with `/reactor-scenario start <id>`.".into(),
                persist: Persist::Nothing,
                activate_toolset: None,
            };
        };
        let steps = load_steps(self.dir, &state.scenario_id);
        let next = state.step_index + 1;
        let mut summaries = state.summaries.clone();
        summaries.push(summary.to_string());

        if next >= steps.len() {
            self.state = None;
            return Advance {
                text: format!("reactor: scenario \"{}\" complete -- {} phase(s) done.", state.scenario_id, steps.len()),
                persist: Persist::Clear,
                activate_toolset: None,
            };
        }
        let new = State { scenario_id: state.scenario_id, step_index: next, summaries };
        self.state = Some(new.clone());
        Advance {
            text: briefing(&steps, next),
            persist: Persist::Set(new),
            activate_toolset: steps[next].toolset.clone(),
        }
    }

    /// `/reactor-scenario [list|start <id>|status|next [summary]|stop]`.
    pub fn command(&mut self, args: &str) -> Effects {
        let mut words = args.split(crate::text::js_space).filter(|w| !w.is_empty());
        let sub = words.next();
        let rest: Vec<&str> = words.collect();

        match sub.unwrap_or("status") {
            "list" => {
                let ids = list_scenarios(self.dir);
                Effects::notice(Notice::info(if ids.is_empty() {
                    "reactor: no scenarios found in prompts/scenarios/".to_string()
                } else {
                    format!("reactor: scenarios -- {}", ids.join(", "))
                }))
            }
            "start" => {
                let id = rest.join(" ");
                if id.is_empty() {
                    return Effects::notice(Notice::error("reactor-scenario: start needs a scenario id -- try `list`"));
                }
                if let Some(s) = &self.state {
                    return Effects::notice(Notice::error(format!(
                        "reactor: \"{}\" is already running -- `stop` it first",
                        s.scenario_id
                    )));
                }
                let steps = load_steps(self.dir, &id);
                if steps.is_empty() {
                    return Effects::notice(Notice::error(format!(
                        "reactor-scenario: unknown scenario \"{id}\" -- try `list`"
                    )));
                }
                let new = State { scenario_id: id.clone(), step_index: 0, summaries: vec![] };
                self.state = Some(new.clone());
                Effects {
                    notices: vec![Notice::info(format!("reactor: started \"{id}\" -- step 1/{}", steps.len()))],
                    messages: vec![Message { content: briefing(&steps, 0), trigger_turn: true }],
                    persist: Persist::Set(new),
                    activate_toolsets: steps[0].toolset.clone().into_iter().collect(),
                }
            }
            "status" => match &self.state {
                None => Effects::notice(Notice::info("reactor: no scenario active")),
                Some(s) => {
                    let steps = load_steps(self.dir, &s.scenario_id);
                    let title = steps
                        .get(s.step_index)
                        .map(|st| st.title.clone())
                        .filter(|t| !t.is_empty())
                        .unwrap_or_else(|| format!("step {}", s.step_index + 1));
                    Effects::notice(Notice::info(format!(
                        "reactor: \"{}\" -- step {}/{}: {}",
                        s.scenario_id,
                        s.step_index + 1,
                        steps.len(),
                        title
                    )))
                }
            },
            "next" => {
                if self.state.is_none() {
                    return Effects::notice(Notice::error("reactor: no scenario active -- `start` one first"));
                }
                // The human is the better judge of whether a phase is genuinely
                // finished (ADR-0009); this bypasses the model entirely.
                let summary = if rest.is_empty() { "(advanced manually)".to_string() } else { rest.join(" ") };
                let adv = self.advance(&summary);
                Effects {
                    notices: vec![],
                    messages: vec![Message { content: adv.text, trigger_turn: true }],
                    persist: adv.persist,
                    activate_toolsets: adv.activate_toolset.into_iter().collect(),
                }
            }
            "stop" => match self.state.take() {
                None => Effects::notice(Notice::info("reactor: no scenario active")),
                Some(s) => Effects {
                    notices: vec![Notice::info(format!("reactor: stopped \"{}\"", s.scenario_id))],
                    persist: Persist::Clear,
                    ..Effects::none()
                },
            },
            other => Effects::notice(Notice::error(format!(
                "reactor-scenario: unknown subcommand \"{other}\" -- try list, start <id>, status, next, or stop"
            ))),
        }
    }
}
