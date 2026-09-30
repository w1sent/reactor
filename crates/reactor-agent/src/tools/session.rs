//! Tools that work on the session itself: reading its history back, keeping its
//! manifest, advancing its scenario. Each is a thin skin over `reactor-context` or
//! [`crate::history`]; the words the model reads come from there.

use reactor_context::manifest::{self, Step};
use reactor_context::scenario::{Persist, Scenario};
use reactor_context::settings::Settings;
use serde_json::{Value, json};

use super::{BoxFut, Tool, ToolCtx, ToolOutput, str_arg, usize_arg};
use crate::context::{KEY_MANIFEST, KEY_SCENARIO, SessionState, save_state};
use crate::history;
use crate::llm::ToolSpec;

pub struct HistoryIndex;

impl Tool for HistoryIndex {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "history_index".into(),
            description: "List the session log, one line per entry: #id, kind, size, a preview, and whether a context reduction currently hides it. Everything ever said or run is here, including what has left your context.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "from": { "type": "integer", "description": "First entry id." },
                    "to": { "type": "integer", "description": "Last entry id." }
                }
            }),
        }
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let store = ctx.store.lock().unwrap();
            ToolOutput::ok(history::index_listing(&store, usize_arg(&args, "from").map(|n| n as u64), usize_arg(&args, "to").map(|n| n as u64)))
        })
    }
}

pub struct HistoryRead;

impl Tool for HistoryRead {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "history_read".into(),
            description: "Read one session entry in full by its #id -- a tool output that was cut or dropped from your context is here whole. Page a large one with `offset` (1-based line) and `limit`.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer" },
                    "offset": { "type": "integer" },
                    "limit": { "type": "integer" }
                },
                "required": ["id"]
            }),
        }
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let Some(id) = usize_arg(&args, "id") else { return ToolOutput::err("history_read needs an integer `id`") };
            let store = ctx.store.lock().unwrap();
            match history::read(&store, id as u64, usize_arg(&args, "offset"), usize_arg(&args, "limit")) {
                Ok(t) => ToolOutput::ok(t),
                Err(e) => ToolOutput::err(e.to_string()),
            }
        })
    }
}

pub struct HistorySearch;

impl Tool for HistorySearch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "history_search".into(),
            description: "Search every line of every session entry -- including dropped ones and the whole of cut tool output -- with a regular expression. Returns #id:line matches. Optionally restrict to kinds: user, assistant, tool_result.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "kinds": { "type": "array", "items": { "type": "string" } },
                    "limit": { "type": "integer", "description": "Default 50." }
                },
                "required": ["pattern"]
            }),
        }
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let Some(pattern) = str_arg(&args, "pattern") else { return ToolOutput::err("history_search needs a `pattern`") };
            let kinds: Vec<String> = args.get("kinds").and_then(Value::as_array).map(|a| a.iter().filter_map(|k| k.as_str().map(str::to_string)).collect()).unwrap_or_default();
            let limit = usize_arg(&args, "limit").unwrap_or(50);
            let store = ctx.store.lock().unwrap();
            match history::search(&store, pattern, &kinds, limit) {
                Ok(hits) => ToolOutput::ok(history::render_hits(&hits, limit)),
                Err(e) => ToolOutput::err(e.to_string()),
            }
        })
    }
}

/// The manifest's own steps, unrelated to a scenario's phases.
pub struct UpdateSteps;

impl Tool for UpdateSteps {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "update_steps".into(),
            description: "Overwrite the step list of the session manifest -- the manifest's own durable steps, unrelated to REactor scenarios. Each step has a short conceptual summary (an investigative question or milestone, not a micro-action) and a 3-word status. Requires a session goal; called without one it returns instructions instead of acting.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "description": "Full replacement step list.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "summary": { "type": "string", "description": "Short conceptual summary (max ~80 chars)" },
                                "status": { "type": "string", "description": "3-word status, e.g. 'in progress' / 'done' / 'blocked on'." }
                            },
                            "required": ["summary", "status"]
                        }
                    }
                },
                "required": ["steps"]
            }),
        }
    }

    fn available(&self, state: &SessionState) -> bool {
        state.manifest.tool_usable()
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let steps: Vec<Step> = match args.get("steps").cloned().map(serde_json::from_value) {
                Some(Ok(s)) => s,
                _ => return ToolOutput::err("update_steps needs `steps`: a list of {summary, status}"),
            };
            let cfg = Settings::load(&ctx.paths).manifest;
            let mut store = ctx.store.lock().unwrap();
            let mut state = SessionState::load(&store).manifest;
            let out = manifest::update_steps(&mut state, &cfg, &steps);
            if out.persist {
                let _ = save_state(&mut store, KEY_MANIFEST, serde_json::to_value(&state).unwrap());
            }
            ToolOutput::ok(out.text)
        })
    }
}

/// Finish the current scenario phase and receive the next briefing.
pub struct PhaseComplete;

impl Tool for PhaseComplete {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: reactor_context::scenario::TOOL_NAME.into(),
            description: "Mark the current REactor scenario's phase complete and receive the next phase's briefing. Call it once a phase's work is genuinely finished, with a summary of what you concluded -- not before, and not to narrate progress mid-phase. Phases are unrelated to the session manifest's steps (update_steps).".into(),
            parameters: json!({
                "type": "object",
                "properties": { "summary": { "type": "string", "description": "What you concluded in this phase. Required before the scenario advances." } },
                "required": ["summary"]
            }),
        }
    }

    fn available(&self, state: &SessionState) -> bool {
        state.scenario.is_some()
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let Some(summary) = str_arg(&args, "summary") else { return ToolOutput::err("reactor_phase_complete needs a `summary`") };
            let adv = {
                let mut store = ctx.store.lock().unwrap();
                let mut sc = Scenario { dir: &ctx.scenarios_dir, state: SessionState::load(&store).scenario };
                let adv = sc.advance(summary);
                match &adv.persist {
                    Persist::Nothing => {}
                    Persist::Set(s) => {
                        let _ = save_state(&mut store, KEY_SCENARIO, serde_json::to_value(s).unwrap());
                    }
                    Persist::Clear => {
                        let _ = save_state(&mut store, KEY_SCENARIO, Value::Null);
                    }
                }
                adv
            };
            // Additive and advisory (ADR-0007): a phase advances whether or not the
            // registry can be told about its toolset.
            if let Some(t) = adv.activate_toolset {
                let _ = reactor_core::commands::set_activation(&ctx.paths, &[t], false, Some(true));
            }
            ToolOutput::ok(adv.text)
        })
    }
}
