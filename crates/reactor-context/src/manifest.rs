//! The session manifest: the user's goal, session guidelines, and the steps the
//! agent maintains itself as short conceptual summaries with a three-word
//! status. Port of `extensions/goal-setting/` (ADR-0024) with the host removed.
//!
//! The block goes into the **system prompt**, and only on content, so a session
//! with nothing set gets nothing and an untouched session's prompt stays
//! byte-identical — which is what the prompt cache needs. `update_steps` is
//! usable while the switch is on **and** a goal is set; the same predicate
//! decides whether it is advertised, so the gate message and the tool's
//! visibility can never disagree (ADR-0030).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::notice::Notice;
use crate::settings::ManifestSettings;
use crate::text::{js_trim, truncate, truncate_words};

/// The session-entry type this state was stored under in pi, for anything that
/// wants to read a pi session's manifest.
pub const ENTRY_TYPE: &str = "pi-goal-setting";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub summary: String,
    pub status: String,
}

/// Per-session manifest state. Serializes the way the pi entry did: unset
/// fields are absent, not null.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct State {
    /// The `/manifest` switch. Absent reads as on — the gate is what keeps
    /// `update_steps` out of the way until a goal exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidelines: Option<String>,
    pub steps: Vec<Step>,
}

impl State {
    /// Read a stored entry, keeping only what has the right shape.
    pub fn normalize(raw: &Value) -> State {
        let mut s = State::default();
        let Some(r) = raw.as_object() else { return s };
        s.enabled = r.get("enabled").and_then(Value::as_bool);
        s.goal = r.get("goal").and_then(Value::as_str).map(str::to_string);
        s.guidelines = r.get("guidelines").and_then(Value::as_str).map(str::to_string);
        if let Some(Value::Array(steps)) = r.get("steps") {
            s.steps = steps
                .iter()
                .filter_map(|st| {
                    let summary = st.get("summary")?.as_str()?.to_string();
                    let status = st.get("status").and_then(Value::as_str).unwrap_or("").to_string();
                    Some(Step { summary, status })
                })
                .collect();
        }
        s
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    pub fn has_goal(&self) -> bool {
        self.goal.as_deref().is_some_and(|g| !js_trim(g).is_empty())
    }

    pub fn has_guidelines(&self) -> bool {
        self.guidelines.as_deref().is_some_and(|g| !js_trim(g).is_empty())
    }

    /// Whether the manifest's row has anything to show.
    pub fn has_content(&self) -> bool {
        self.has_goal() || self.has_guidelines() || !self.steps.is_empty()
    }

    /// Whether `update_steps` may act — and, by the same predicate, whether it
    /// is advertised.
    pub fn tool_usable(&self) -> bool {
        self.is_enabled() && self.has_goal()
    }

    /// `update_steps` answers instead of acting unless both halves hold.
    pub fn tool_gate_message(&self) -> Option<&'static str> {
        if !self.is_enabled() {
            Some("goal-setting is off. Run /manifest on to enable it.")
        } else if !self.has_goal() {
            Some("update_steps is inactive until a session goal is set. Set one with /goal <text>.")
        } else {
            None
        }
    }
}

/// What a handler did to the world, for the caller to carry out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effects {
    /// Shown to the person.
    pub notices: Vec<Notice>,
    /// The state changed and must be stored as a session entry.
    pub persist: bool,
}

impl Effects {
    fn note(n: Notice) -> Self {
        Effects { notices: vec![n], persist: false }
    }
    fn changed(n: Notice) -> Self {
        Effects { notices: vec![n], persist: true }
    }
}

// -- the block ----------------------------------------------------------------

/// The manifest block: goal and steps (the manifest proper), plus the session
/// guidelines, each on its own content.
pub fn block(state: &State, cfg: &ManifestSettings) -> Option<String> {
    // The switch pauses the whole thing: block and tool both go quiet, while the
    // goal/guidelines/steps are preserved for `/manifest on`.
    if !state.is_enabled() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    if state.has_goal() {
        parts.push("## Session Manifest".into());
        parts.push(String::new());
        parts.push(format!("Goal: {}", js_trim(state.goal.as_deref().unwrap())));
        parts.push(String::new());
        parts.push(format!("Steps ({}/{}):", state.steps.len(), cfg.soft_step_limit));
        if state.steps.is_empty() {
            parts.push("(none yet)".into());
        } else {
            parts.push(step_lines(&state.steps, "").join("\n"));
        }
        parts.push(String::new());
        parts.push(
            "Steps are the session's durable memory: record progress as conceptual summaries (an investigative question or milestone, not a micro-action), each with a 3-word status; overwrite the full list with update_steps; consolidate past the soft limit. If a past decision or finding matters, it belongs in the steps -- not only in messages that may later be compacted or faded away."
                .into(),
        );
    }
    if state.has_guidelines() {
        parts.push("## Session Guidelines".into());
        parts.push(String::new());
        parts.push(js_trim(state.guidelines.as_deref().unwrap()).to_string());
    }
    if parts.is_empty() { None } else { Some(parts.join("\n\n")) }
}

fn step_lines(steps: &[Step], indent: &str) -> Vec<String> {
    steps.iter().enumerate().map(|(i, s)| format!("{indent}{}. [{}] {}", i + 1, s.status, s.summary)).collect()
}

/// `/frame`: the manifest as plain text.
pub fn frame_view(state: &State, cfg: &ManifestSettings) -> String {
    let mut out = vec![
        format!("goal: {}", state.goal.as_deref().unwrap_or("(none)")),
        format!("guidelines: {}", state.guidelines.as_deref().unwrap_or("(none)")),
        format!("steps ({}/{}):", state.steps.len(), cfg.soft_step_limit),
    ];
    if state.steps.is_empty() {
        out.push("  (none)".into());
    } else {
        out.push(step_lines(&state.steps, "  ").join("\n"));
    }
    out.push(format!("switch: {}", if state.is_enabled() { "on" } else { "off" }));
    out.join("\n")
}

// -- commands -----------------------------------------------------------------

/// `/goal`, `/guidelines`, `/manifest`, `/frame`. `derive` is separate: it needs
/// a model, which this crate does not have — see [`apply_derivation`].
///
/// Returns `None` for a command this module does not own.
pub fn command(state: &mut State, cfg: &ManifestSettings, name: &str, args: &str) -> Option<Effects> {
    let text = js_trim(args);
    Some(match name {
        "goal" => {
            if text.eq_ignore_ascii_case("clear") {
                state.goal = None;
                Effects::changed(Notice::info("goal cleared"))
            } else if text.is_empty() {
                Effects::note(Notice::warning("usage: /goal <text> | /goal clear"))
            } else {
                state.goal = Some(text.to_string());
                Effects::changed(Notice::info(format!("goal set: {text}")))
            }
        }
        "guidelines" => {
            if text.eq_ignore_ascii_case("clear") {
                state.guidelines = None;
                Effects::changed(Notice::info("guidelines cleared"))
            } else if text.is_empty() {
                Effects::note(Notice::warning("usage: /guidelines <text> | /guidelines clear"))
            } else {
                state.guidelines = Some(text.to_string());
                Effects::changed(Notice::info(format!("guidelines set: {text}")))
            }
        }
        "manifest" => {
            let arg = text.to_lowercase();
            if arg == "clear" {
                state.goal = None;
                state.guidelines = None;
                state.steps.clear();
                return Some(Effects::changed(Notice::info("manifest cleared -- goal, guidelines and steps are gone")));
            }
            let next = match arg.as_str() {
                "on" => true,
                "off" => false,
                "" => !state.is_enabled(),
                other => {
                    return Some(Effects::note(Notice::warning(format!(
                        "manifest: unknown argument \"{other}\" -- try on, off or clear"
                    ))));
                }
            };
            state.enabled = Some(next);
            let msg = format!("goal-setting {}", if next { "enabled" } else { "disabled" });
            Effects::changed(if next { Notice::info(msg) } else { Notice::warning(msg) })
        }
        "frame" => Effects::note(Notice::info(frame_view(state, cfg))),
        _ => return None,
    })
}

/// The `update_steps` tool: overwrite the step list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutcome {
    pub text: String,
    /// The state changed and must be stored.
    pub persist: bool,
}

pub fn update_steps(state: &mut State, cfg: &ManifestSettings, steps: &[Step]) -> ToolOutcome {
    if let Some(gate) = state.tool_gate_message() {
        return ToolOutcome { text: gate.to_string(), persist: false };
    }
    let clamped: Vec<Step> = steps
        .iter()
        .map(|s| Step { summary: truncate(&s.summary, cfg.max_description), status: truncate_words(&s.status, cfg.status_words) })
        .collect();
    let n = clamped.len();
    state.steps = clamped;
    let mut text = format!("Steps updated: {n} step(s).");
    if n > cfg.soft_step_limit {
        text.push_str(&format!(
            "\n\nWARNING: step count ({n}) exceeds the soft limit ({}). Consolidate or mark finished steps complete and rewrite the list via update_steps.",
            cfg.soft_step_limit
        ));
    }
    ToolOutcome { text, persist: true }
}

// -- derive -------------------------------------------------------------------

/// Derive the manifest from a transcript by asking a model. This crate does not
/// ask; it supplies the words to ask with and reads the answer.
pub const DERIVE_SYSTEM: &str = "You derive a session manifest from a transcript. Respond with ONLY the requested JSON object -- no markdown fences, no commentary.";

pub const DERIVE_SCOPES: [&str; 4] = ["all", "goal", "guidelines", "steps"];

/// The user message that precedes the transcript tail.
pub fn derive_task(scope: &str) -> String {
    let shape = match scope {
        "goal" => r#"{"goal": "<one sentence: what this session is trying to achieve>"}"#,
        "guidelines" => r#"{"guidelines": "<standing constraints the work implies, or "" if none>"}"#,
        "steps" => r#"{"steps": [{"summary": "<conceptual step, not a micro-action>", "status": "<3 words>"}]}"#,
        _ => r#"{"goal": "<one sentence>", "guidelines": "<standing constraints, or "" if none>", "steps": [{"summary": "<conceptual step>", "status": "<3 words>"}]}"#,
    };
    let what = match scope {
        "goal" => "the session goal",
        "guidelines" => "standing guidelines",
        "steps" => "the steps (covering the REMAINING work)",
        _ => "the goal, guidelines and steps",
    };
    format!(
        "Based on the transcript below, derive {what} for this session. Respond with ONLY a JSON object of exactly this shape: {shape}. Steps cover what remains, not history; statuses are 3 words each.\n\n--- session transcript (tail) ---\n"
    )
}

/// `/derive [scope]`: the scope asked for, or the warning that says it is not one.
pub fn derive_scope(args: &str) -> Result<String, Notice> {
    let sub = js_trim(args).to_lowercase();
    let sub = if sub.is_empty() { "all".to_string() } else { sub };
    if DERIVE_SCOPES.contains(&sub.as_str()) {
        Ok(sub)
    } else {
        Err(Notice::warning(format!("derive: unknown scope \"{sub}\" -- try all, goal, guidelines or steps")))
    }
}

/// The first JSON object in a model's answer, fences and prose notwithstanding.
pub fn extract_json(text: &str) -> Option<Value> {
    let stripped = crate::text::js_trim(&strip_fences(text)).to_string();
    let start = stripped.find('{')?;
    let end = stripped.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&stripped[start..=end]).ok()
}

/// `text.replace(/```(?:json)?/g, "")`.
fn strip_fences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("```") {
        out.push_str(&rest[..i]);
        rest = &rest[i + 3..];
        rest = rest.strip_prefix("json").unwrap_or(rest);
    }
    out.push_str(rest);
    out
}

/// What a derivation carries; `None` fields were not asked for or not answered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Derived {
    pub goal: Option<String>,
    pub guidelines: Option<String>,
    pub steps: Option<Vec<Step>>,
}

pub fn parse_derivation(text: &str, scope: &str, cfg: &ManifestSettings) -> Option<Derived> {
    let parsed = extract_json(text)?;
    let obj = parsed.as_object()?;
    let wants = |s: &str| scope == "all" || scope == s;
    let mut parts = Derived::default();
    if wants("goal")
        && let Some(g) = obj.get("goal").and_then(Value::as_str).filter(|g| !js_trim(g).is_empty())
    {
        parts.goal = Some(js_trim(g).to_string());
    }
    if wants("guidelines")
        && let Some(g) = obj.get("guidelines").and_then(Value::as_str)
    {
        parts.guidelines = Some(js_trim(g).to_string());
    }
    if wants("steps")
        && let Some(Value::Array(steps)) = obj.get("steps")
    {
        parts.steps = Some(
            steps
                .iter()
                .filter_map(|st| {
                    let summary = st.get("summary")?.as_str().filter(|s| !js_trim(s).is_empty())?;
                    let status = st.get("status").and_then(Value::as_str).unwrap_or("");
                    Some(Step {
                        summary: truncate(summary, cfg.max_description),
                        status: truncate_words(status, cfg.status_words),
                    })
                })
                .collect(),
        );
    }
    if parts == Derived::default() { None } else { Some(parts) }
}

/// Apply a model's answer to the manifest, exactly as `/goal` etc. would.
/// `None` for the response means the model produced nothing usable and the
/// notice says so.
pub fn apply_derivation(state: &mut State, cfg: &ManifestSettings, scope: &str, response: &str) -> Effects {
    let Some(parts) = parse_derivation(response, scope, cfg) else {
        return Effects::note(Notice::warning("derive: the response was not the requested JSON -- nothing applied"));
    };
    let mut applied: Vec<String> = Vec::new();
    if let Some(goal) = parts.goal {
        applied.push(format!("goal: {goal}"));
        state.goal = Some(goal);
    }
    if let Some(g) = parts.guidelines {
        state.guidelines = Some(g);
        applied.push("guidelines".into());
    }
    if let Some(steps) = parts.steps {
        applied.push(format!("steps: {}", steps.len()));
        state.steps = steps;
    }
    Effects::changed(Notice::info(format!("derive: {} -- /frame to review", applied.join(", "))))
}
