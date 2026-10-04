//! The system prompt, assembled from parts that are each a pure function of state.
//!
//! Nothing in it is remembered between requests: the manifest, identity, registry,
//! skills, reporting reminder and current scenario phase are re-rendered every time
//! from `reactor-context` and the catalogue, so they cannot be reclaimed by a context
//! reduction — which is a guarantee here, where in pi it held only by consequence of
//! sitting in the system prompt (ADR-0037). The order is fixed, so an unchanged session
//! sends byte-identical bytes and the provider's prompt cache holds (ADR-0006).

use std::path::Path;

use reactor_context::settings::Settings;
use reactor_context::{identity, manifest, reporting, scenario};

use crate::context::SessionState;
use crate::skills::{Skill, block as skills_block};

pub const DEFAULT_BASE: &str = "You are REactor, an agent for reverse engineering and malware analysis, working in the user's shell on their machine.

Work like an analyst: form a question, run the smallest command that answers it, read the result, and write down what you established. Prefer the tools listed below over writing a parser by hand, and read `<tool> --help` or the tool's skill before guessing flags. Verify static conclusions dynamically where you can, and label inference that you have not proven.

Your context is finite and is managed for you. Long tool output is cut to its head and tail, and older work is summarized or dropped from view as the session grows -- but nothing is ever deleted. Every entry has a #id; `history_index` lists them, `history_read` returns one in full, and `history_search` greps all of them, including what has left your context. If a summary or a note says something was dropped, that is where to get it back.

Do the work and report what you found. Do not ask permission for routine analysis; do stop and ask when an action is destructive or cannot be undone.";

/// What the agent is told about the case's graph (ADR-0047). The same text on every
/// request while the session is attached, so the prompt cache holds; absent while the
/// *Case graph* switch is off. It says nothing of what the graph holds: the user leads,
/// and the agent asks the graph when it needs to.
pub const CASE_GRAPH: &str = "## Case graph

This case has a graph: the shared record of what was seen, what is related and what was concluded. It outlives this session, and the user reads and edits it too. Keep it in step with the work, the way an analyst keeps notes -- as you go, not at the end.

- Record what matters to the case, not everything you looked at. Absence in the graph means \"not recorded\", never \"does not exist\".
- What a source says is an event (it happened) or an observation (it holds), added with `graph_add` and always with its source: a file or table in the case, or the session entry it came from. What you or the user conclude is an annotation, added with `graph_annotate`: a hypothesis, a judgement of a claim with a confidence, a note or question. Do not dress a conclusion up as an observation.
- Things that can show up in more than one place -- addresses, domains, hosts, accounts, hashes -- are entities. Give the type and the value as you found it; the same value always lands on the same node, which is how separate findings connect. Values that connect almost everything (a common port, 127.0.0.1, svchost.exe) are properties, not entities.
- For more than a handful of rows, write them to `reactor_data.sqlite` (or a CSV/JSONL file) with a script and bring the relevant ones in with `graph_pull`. Bulk data stays in its source.
- Before adding, look: `graph_query` shows what is already there and where else an entity shows up. When you look for something and find nothing, record that too (`search_negative`), with where you looked -- \"not seen\" and \"not checked\" must not look the same.
- Nothing is overwritten. Revise with a change (a new version, the old one kept); soft-delete what turned out wrong; revert only a slip nothing builds on.
- Register files you rely on or produce with `artifact_register`; they stay where they are.

The user decides what to pursue. Do not act on what the graph holds unless it bears on what you were asked.";

/// What the catalogue says right now (from `reactor registry`).
#[derive(Debug, Clone, Default)]
pub struct RegistryView {
    /// The `## Available RE tools` block.
    pub block: String,
    /// Fetched upstream skills of present, active tools.
    pub skill_dirs: Vec<String>,
    /// Ids of tools that are present and active.
    pub usable: std::collections::HashSet<String>,
}

pub struct Inputs<'a> {
    pub base: &'a str,
    pub settings: &'a Settings,
    pub state: &'a SessionState,
    pub registry: &'a RegistryView,
    pub skills: &'a [Skill],
    pub scenarios_dir: &'a Path,
}

/// Where a piece of the system prompt comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Base,
    Identity,
    Registry,
    Skills,
    CaseGraph,
    Manifest,
    Reporting,
    Scenario,
}

/// The system prompt's pieces, in the order they are sent, each with where it comes from.
pub fn system_parts(i: &Inputs) -> Vec<(Part, String)> {
    let mut parts: Vec<(Part, String)> = vec![(Part::Base, i.base.trim_end().to_string())];
    let mut push = |part: Part, b: Option<String>| {
        parts.extend(b.filter(|s| !s.is_empty()).map(|s| (part, s)))
    };

    push(
        Part::Identity,
        identity::block(&i.state.identity, &i.settings.identity),
    );
    push(
        Part::Registry,
        (!i.registry.block.is_empty()).then(|| i.registry.block.clone()),
    );
    push(Part::Skills, skills_block(i.skills));
    push(
        Part::CaseGraph,
        i.state.memory.case_graph().then(|| CASE_GRAPH.to_string()),
    );
    push(
        Part::Manifest,
        manifest::block(&i.state.manifest, &i.settings.manifest),
    );
    push(
        Part::Reporting,
        i.state
            .reporting
            .is_enabled()
            .then(|| reporting::block(&i.settings.reporting)),
    );
    push(Part::Scenario, scenario_block(i.state, i.scenarios_dir));
    parts
}

/// Pieces joined into the system prompt.
pub fn join_parts(parts: &[(Part, String)]) -> String {
    parts
        .iter()
        .map(|(_, s)| s.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The system prompt for one request.
pub fn system_prompt(i: &Inputs) -> String {
    join_parts(&system_parts(i))
}

/// The current phase's briefing, and what has been concluded so far.
fn scenario_block(state: &SessionState, dir: &Path) -> Option<String> {
    let s = state.scenario.as_ref()?;
    let steps = scenario::load_steps(dir, &s.scenario_id);
    if s.step_index >= steps.len() {
        return None;
    }
    let mut out = format!(
        "## Scenario: {}\n\n{}",
        s.scenario_id,
        scenario::briefing(&steps, s.step_index)
    );
    if !s.summaries.is_empty() {
        out.push_str("\n\nCompleted phases:");
        for (i, summary) in s.summaries.iter().enumerate() {
            let title = steps
                .get(i)
                .map(|st| st.title.as_str())
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("phase {}", i + 1));
            out.push_str(&format!("\n{}. {} -- {}", i + 1, title, summary));
        }
    }
    out.push_str("\n\nWhen this phase is genuinely finished, call reactor_phase_complete with a summary of what you concluded.");
    Some(out)
}
