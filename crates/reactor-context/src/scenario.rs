//! Multi-phase analysis scenarios: a run of prose briefings, one per phase, that
//! advance on a tool result rather than on a marker or a heuristic (ADR-0009).
//! Port of `extensions/scenario/` (ADR-0017, ADR-0030).
//!
//! A scenario is a directory of `NN-name.md` files, read by
//! [`reactor_core::scenarios`]: `title:`, `toolset:` and, in the first, `description:`
//! frontmatter, and everything else about a phase (which tools just became relevant,
//! what not to start yet) is prose the author writes into the body. Scenarios are
//! looked up in the case, the config directory and a checkout, in that order
//! ([`Library`], ADR-0065). Activating a phase's toolset is *additive and advisory*
//! (ADR-0007): the effect asks the caller to enable it, and a caller that cannot still
//! lets the phase advance.

use std::path::Path;

use serde::{Deserialize, Serialize};

pub use reactor_core::scenarios::{Library, Phase as Step};

use crate::notice::Notice;
use crate::text::js_trim;

pub const ENTRY_TYPE: &str = "reactor-scenario";
pub const TOOL_NAME: &str = "reactor_phase_complete";

/// The scenario in progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub scenario_id: String,
    /// Index into that scenario's steps — the phase currently in progress.
    pub step_index: usize,
    /// One per completed phase, parallel to `steps[0..step_index]`.
    pub summaries: Vec<String>,
    /// The summary of this completed phase is still to be written: the person moved the
    /// scenario on by hand, and the next prompt they send says what the phase concluded
    /// ([`record_summary`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_summary: Option<usize>,
    /// The phases left out when it was started: moving on passes over them, recording
    /// [`SKIPPED`] (ADR-0065).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skip: Vec<usize>,
}

/// The summary of a phase that was jumped over.
pub const SKIPPED: &str = "(skipped)";
/// The summary of a phase the person advanced past without saying what it concluded.
pub const MANUAL: &str = "(advanced manually)";
/// The longest a summary taken from a prompt is kept.
const SUMMARY_CHARS: usize = 600;

/// A person moved the scenario on by hand and then wrote a prompt: that prompt is what the
/// phase they left concluded. Returns whether `state` changed (and so must be stored).
pub fn record_summary(state: &mut State, prompt: &str) -> bool {
    let Some(at) = state.pending_summary.take() else {
        return false;
    };
    let text = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some(slot) = state.summaries.get_mut(at)
        && !text.is_empty()
    {
        *slot = if text.chars().count() > SUMMARY_CHARS {
            let cut: String = text.chars().take(SUMMARY_CHARS).collect();
            format!("{cut}…")
        } else {
            text
        };
    }
    true
}

// -- reading scenarios ----------------------------------------------------------

/// Scenario ids: the scenarios in `dir`, sorted.
pub fn list_scenarios(dir: &Path) -> Vec<String> {
    Library::single(dir).ids()
}

pub fn parse_step(raw: &str) -> Step {
    reactor_core::scenarios::parse_phase(raw)
}

/// A scenario's phases, in file-name order. Unknown or unreadable is empty.
pub fn load_steps(dir: &Path, id: &str) -> Vec<Step> {
    Library::single(dir).load(id)
}

/// `## Phase 2/4: Static analysis`, then the phase's own body verbatim.
pub fn briefing(steps: &[Step], index: usize) -> String {
    let step = &steps[index];
    let title = if step.title.is_empty() {
        format!("step {}", index + 1)
    } else {
        step.title.clone()
    };
    format!(
        "## Phase {}/{}: {}\n\n{}",
        index + 1,
        steps.len(),
        title,
        step.body
    )
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
    /// The question a scenario was started with: it becomes the session's goal.
    pub goal: Option<String>,
}

impl Effects {
    fn none() -> Self {
        Effects {
            notices: vec![],
            messages: vec![],
            persist: Persist::Nothing,
            activate_toolsets: vec![],
            goal: None,
        }
    }
    fn notice(n: Notice) -> Self {
        Effects {
            notices: vec![n],
            ..Effects::none()
        }
    }
}

/// The scenario runner: a state and the places scenarios are found.
#[derive(Debug, Clone)]
pub struct Scenario<'a> {
    pub lib: &'a Library,
    pub state: Option<State>,
}

/// The first phase from `from` on that is not left out.
fn next_unskipped(skip: &[usize], from: usize) -> usize {
    (from..).find(|i| !skip.contains(i)).unwrap_or(from)
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
        let steps = self.lib.load(&state.scenario_id);
        let mut summaries = state.summaries.clone();
        summaries.push(summary.to_string());
        let next = next_unskipped(&state.skip, state.step_index + 1);
        summaries
            .extend((state.step_index + 1..next.min(steps.len())).map(|_| SKIPPED.to_string()));

        if next >= steps.len() {
            self.state = None;
            return Advance {
                text: format!(
                    "reactor: scenario \"{}\" complete -- {} phase(s) done.",
                    state.scenario_id,
                    steps.len()
                ),
                persist: Persist::Clear,
                activate_toolset: None,
            };
        }
        let new = State {
            scenario_id: state.scenario_id,
            step_index: next,
            summaries,
            pending_summary: None,
            skip: state.skip,
        };
        self.state = Some(new.clone());
        Advance {
            text: briefing(&steps, next),
            persist: Persist::Set(new),
            activate_toolset: steps[next].toolset.clone(),
        }
    }

    /// Move to phase `target` (0-based) by hand, forwards or backwards, without a turn: the
    /// briefing reaches the model through the system prompt, and the person's next prompt
    /// starts the phase. The phases jumped over are marked [`SKIPPED`]. Going forwards leaves
    /// the phase being left without a summary until that prompt arrives, unless `summary` says
    /// it now; going back drops the summaries of the phases that will be done again.
    fn go_to(
        &mut self,
        state: State,
        steps: &[Step],
        target: usize,
        summary: Option<String>,
    ) -> Effects {
        let at = state.step_index;
        if target == at {
            return Effects::notice(Notice::info(format!(
                "reactor: already at phase {}/{}",
                at + 1,
                steps.len()
            )));
        }
        let mut summaries = state.summaries.clone();
        let mut pending = None;
        let title = steps
            .get(target)
            .map(|s| s.title.as_str())
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("step {}", target + 1));
        let note = if target > at {
            summaries.truncate(at);
            if summary.is_none() {
                pending = Some(at);
            }
            summaries.push(summary.unwrap_or_else(|| MANUAL.to_string()));
            summaries.extend((at + 1..target).map(|_| SKIPPED.to_string()));
            if pending.is_some() {
                format!(
                    "reactor: phase {}/{}: {title} -- say in your next prompt, in a line or two, what phase {} concluded: it is recorded as its summary",
                    target + 1,
                    steps.len(),
                    at + 1
                )
            } else {
                format!("reactor: phase {}/{}: {title}", target + 1, steps.len())
            }
        } else {
            summaries.truncate(target);
            format!(
                "reactor: back at phase {}/{}: {title} -- your next prompt starts it",
                target + 1,
                steps.len()
            )
        };
        // Going to a phase that was left out takes it back in.
        let mut skip = state.skip;
        skip.retain(|&i| i != target);
        let new = State {
            scenario_id: state.scenario_id,
            step_index: target,
            summaries,
            pending_summary: pending,
            skip,
        };
        self.state = Some(new.clone());
        Effects {
            notices: vec![Notice::info(note)],
            persist: Persist::Set(new),
            activate_toolsets: steps[target].toolset.clone().into_iter().collect(),
            ..Effects::none()
        }
    }

    /// `/reactor-scenario [list|start <id> [skip=2,4] [question]|status|next [summary]|goto <n> [summary]|stop]`.
    pub fn command(&mut self, args: &str) -> Effects {
        let mut words = args.split(crate::text::js_space).filter(|w| !w.is_empty());
        let sub = words.next();
        let rest: Vec<&str> = words.collect();

        match sub.unwrap_or("status") {
            "list" => {
                let ids = self.lib.ids();
                Effects::notice(Notice::info(if ids.is_empty() {
                    "reactor: no scenarios found in prompts/scenarios/".to_string()
                } else {
                    format!("reactor: scenarios -- {}", ids.join(", "))
                }))
            }
            "start" => {
                let Some(start) = parse_start(args) else {
                    return Effects::notice(Notice::error(
                        "reactor-scenario: start needs a scenario id -- try `list`",
                    ));
                };
                let id = start.id;
                if let Some(s) = &self.state {
                    return Effects::notice(Notice::error(format!(
                        "reactor: \"{}\" is already running -- `stop` it first",
                        s.scenario_id
                    )));
                }
                let steps = self.lib.load(&id);
                if steps.is_empty() {
                    return Effects::notice(Notice::error(format!(
                        "reactor-scenario: unknown scenario \"{id}\" -- try `list`"
                    )));
                }
                let skip: Vec<usize> = start
                    .skip
                    .into_iter()
                    .filter(|&i| i < steps.len())
                    .collect();
                let first = next_unskipped(&skip, 0);
                if first >= steps.len() {
                    return Effects::notice(Notice::error(
                        "reactor-scenario: every phase is left out -- keep at least one",
                    ));
                }
                let new = State {
                    scenario_id: id.clone(),
                    step_index: first,
                    summaries: (0..first).map(|_| SKIPPED.to_string()).collect(),
                    pending_summary: None,
                    skip,
                };
                self.state = Some(new.clone());
                Effects {
                    notices: vec![Notice::info(format!(
                        "reactor: started \"{id}\" -- step {}/{}",
                        first + 1,
                        steps.len()
                    ))],
                    messages: vec![Message {
                        content: briefing(&steps, first),
                        trigger_turn: true,
                    }],
                    persist: Persist::Set(new),
                    activate_toolsets: steps[first].toolset.clone().into_iter().collect(),
                    goal: start.question,
                }
            }
            "status" => match &self.state {
                None => Effects::notice(Notice::info("reactor: no scenario active")),
                Some(s) => {
                    let steps = self.lib.load(&s.scenario_id);
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
                let Some(state) = self.state.clone() else {
                    return Effects::notice(Notice::error(
                        "reactor: no scenario active -- `start` one first",
                    ));
                };
                // The human is the better judge of whether a phase is genuinely finished
                // (ADR-0009). Moving on by hand does not start a turn: what the agent was
                // doing is over, and the person's next prompt starts the phase.
                let steps = self.lib.load(&state.scenario_id);
                let target = next_unskipped(&state.skip, state.step_index + 1);
                if target >= steps.len() {
                    let summary = if rest.is_empty() {
                        MANUAL.to_string()
                    } else {
                        rest.join(" ")
                    };
                    let adv = self.advance(&summary);
                    return Effects {
                        notices: vec![Notice::info(adv.text)],
                        persist: adv.persist,
                        ..Effects::none()
                    };
                }
                let summary = (!rest.is_empty()).then(|| rest.join(" "));
                self.go_to(state, &steps, target, summary)
            }
            "goto" => {
                let Some(state) = self.state.clone() else {
                    return Effects::notice(Notice::error(
                        "reactor: no scenario active -- `start` one first",
                    ));
                };
                let steps = self.lib.load(&state.scenario_id);
                let target = rest
                    .first()
                    .and_then(|n| n.parse::<usize>().ok())
                    .filter(|n| (1..=steps.len()).contains(n));
                let Some(target) = target else {
                    return Effects::notice(Notice::error(format!(
                        "reactor-scenario: goto needs a phase number from 1 to {}",
                        steps.len()
                    )));
                };
                let summary = (rest.len() > 1).then(|| rest[1..].join(" "));
                self.go_to(state, &steps, target - 1, summary)
            }
            "promote" => {
                let id = rest.join(" ");
                match self.lib.find(&id) {
                    Some((reactor_core::scenarios::Scope::Case, _)) => Effects {
                        messages: vec![Message {
                            content: promote_prompt(&id),
                            trigger_turn: true,
                        }],
                        ..Effects::none()
                    },
                    Some(_) => Effects::notice(Notice::error(format!(
                        "reactor-scenario: \"{id}\" is not a case scenario -- only those are promoted"
                    ))),
                    None => Effects::notice(Notice::error(format!(
                        "reactor-scenario: unknown scenario \"{id}\" -- try `list`"
                    ))),
                }
            }
            "stop" => match self.state.take() {
                None => Effects::notice(Notice::info("reactor: no scenario active")),
                Some(s) => Effects {
                    notices: vec![Notice::info(format!(
                        "reactor: stopped \"{}\"",
                        s.scenario_id
                    ))],
                    persist: Persist::Clear,
                    ..Effects::none()
                },
            },
            other => Effects::notice(Notice::error(format!(
                "reactor-scenario: unknown subcommand \"{other}\" -- try list, start <id>, status, next, goto <n>, stop or promote <id>"
            ))),
        }
    }
}

/// What the agent is asked to do to make a case scenario global: rewrite it without the
/// case in it, then save it (ADR-0065).
pub fn promote_prompt(id: &str) -> String {
    format!(
        "Promote the case scenario `{id}` to a global one, for every case. Read it with scenario_list (id `{id}`). Rewrite each phase so it holds for any case of this kind: take out this case's host names, paths, user and account names, indicators, dates and other specifics, and keep the method. Then save it with scenario_save, scope global, id `{id}`, and tell me in a few lines what you took out or generalised."
    )
}

/// `start <id> [skip=2,4] [question…]`: the question keeps its line breaks.
struct Start {
    id: String,
    /// 0-based.
    skip: Vec<usize>,
    question: Option<String>,
}

fn next_word(s: &str) -> (&str, &str) {
    let s = js_trim(s);
    match s.find(crate::text::js_space) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    }
}

fn parse_start(args: &str) -> Option<Start> {
    let (_, rest) = next_word(args);
    let (id, mut rest) = next_word(rest);
    if id.is_empty() {
        return None;
    }
    let mut skip = Vec::new();
    let (word, after) = next_word(rest);
    if let Some(list) = word.strip_prefix("skip=") {
        skip = list
            .split(',')
            .filter_map(|n| n.trim().parse::<usize>().ok())
            .filter(|&n| n >= 1)
            .map(|n| n - 1)
            .collect();
        rest = after;
    }
    let question = js_trim(rest);
    Some(Start {
        id: id.to_string(),
        skip,
        question: (!question.is_empty()).then(|| question.to_string()),
    })
}
