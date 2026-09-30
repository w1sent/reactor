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

/// The system prompt for one request.
pub fn system_prompt(i: &Inputs) -> String {
    let mut parts: Vec<String> = vec![i.base.trim_end().to_string()];
    let mut push = |b: Option<String>| parts.extend(b.filter(|s| !s.is_empty()));

    push(identity::block(&i.state.identity, &i.settings.identity));
    push((!i.registry.block.is_empty()).then(|| i.registry.block.clone()));
    push(skills_block(i.skills));
    push(manifest::block(&i.state.manifest, &i.settings.manifest));
    push(i.state.reporting.is_enabled().then(|| reporting::block(&i.settings.reporting)));
    push(scenario_block(i.state, i.scenarios_dir));
    parts.join("\n\n")
}

/// The current phase's briefing, and what has been concluded so far.
fn scenario_block(state: &SessionState, dir: &Path) -> Option<String> {
    let s = state.scenario.as_ref()?;
    let steps = scenario::load_steps(dir, &s.scenario_id);
    if s.step_index >= steps.len() {
        return None;
    }
    let mut out = format!("## Scenario: {}\n\n{}", s.scenario_id, scenario::briefing(&steps, s.step_index));
    if !s.summaries.is_empty() {
        out.push_str("\n\nCompleted phases:");
        for (i, summary) in s.summaries.iter().enumerate() {
            let title = steps.get(i).map(|st| st.title.as_str()).filter(|t| !t.is_empty()).map(str::to_string).unwrap_or_else(|| format!("phase {}", i + 1));
            out.push_str(&format!("\n{}. {} -- {}", i + 1, title, summary));
        }
    }
    out.push_str("\n\nWhen this phase is genuinely finished, call reactor_phase_complete with a summary of what you concluded.");
    Some(out)
}
