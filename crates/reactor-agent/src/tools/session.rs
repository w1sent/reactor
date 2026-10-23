//! Tools that work on the session itself: reading its history back, keeping its
//! manifest. The scenario's tools are in [`super::scenario`]. Each is a thin skin over `reactor-context` or
//! [`crate::history`]; the words the model reads come from there.

use reactor_context::manifest::{self, Step};
use reactor_context::settings::Settings;
use serde_json::{Value, json};

use super::{BoxFut, Tool, ToolCtx, ToolOutput, str_arg, usize_arg};
use crate::context::{SessionState, save_manifest};
use crate::history;
use crate::llm::ToolSpec;
use reactor_db::manifest::Author;

/// The answer when a tool is asked to look outside this session while the
/// session-history switch is off (ADR-0047). It explains rather than blocks silently
/// (ADR-0030).
fn other_sessions_off(ctx: &ToolCtx) -> Option<ToolOutput> {
    let global = Settings::load(&ctx.paths).memory;
    (!ctx.state().memory.over(&global).session_history()).then(|| {
        ToolOutput::err(
            "this session does not draw on the case's other sessions (the session-history switch is off). Only this session's own history is available.",
        )
    })
}

pub struct HistoryIndex;

impl Tool for HistoryIndex {
    fn prints_much(&self) -> bool {
        true
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "history_index".into(),
            description: "List the session log, one line per entry: #id, kind, size, a preview, and whether a context reduction hides it. Everything ever said or run is here, including what has left your context.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "from": { "type": "integer", "description": "First entry id (default: the start)." },
                    "to": { "type": "integer", "description": "Last entry id (default: the end)." },
                    "session": { "type": "string", "description": "Another session of this case: its id, or `all` for the list of sessions (default: this session)." }
                }
            }),
        }
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let (from, to) = (
                usize_arg(&args, "from").map(|n| n as u64),
                usize_arg(&args, "to").map(|n| n as u64),
            );
            let Some(session) = str_arg(&args, "session") else {
                let store = ctx.store.lock().unwrap();
                return ToolOutput::ok(history::index_listing(&store, from, to));
            };
            if let Some(refusal) = other_sessions_off(ctx) {
                return refusal;
            }
            let store = ctx.store.lock().unwrap();
            let listing = if session == "all" {
                history::sessions_listing(&store)
            } else {
                history::Log::other(&store, session).map(|log| history::index_of(&log, from, to))
            };
            match listing {
                Ok(t) => ToolOutput::ok(t),
                Err(e) => ToolOutput::err(e.to_string()),
            }
        })
    }
}

pub struct HistoryRead;

impl Tool for HistoryRead {
    fn prints_much(&self) -> bool {
        true
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "history_read".into(),
            description: "Read one session entry in full by its #id. A tool output that was cut or dropped from your context is here whole; page a large one with `offset` and `limit`.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "The entry's #id." },
                    "offset": { "type": "integer", "description": "Line to start from (1-based, default 1)." },
                    "limit": { "type": "integer", "description": "Maximum number of lines (default: to the end)." },
                    "session": { "type": "string", "description": "Read an entry of another session of this case (default: this session)." }
                },
                "required": ["id"]
            }),
        }
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let Some(id) = usize_arg(&args, "id") else {
                return ToolOutput::err("history_read needs an integer `id`");
            };
            let session = str_arg(&args, "session");
            if session.is_some()
                && let Some(refusal) = other_sessions_off(ctx)
            {
                return refusal;
            }
            let store = ctx.store.lock().unwrap();
            let read = match session {
                Some(s) => history::Log::other(&store, s),
                None => Ok(history::Log::own(&store)),
            }
            .and_then(|log| {
                history::read_from(
                    &log,
                    id as u64,
                    usize_arg(&args, "offset"),
                    usize_arg(&args, "limit"),
                )
            });
            match read {
                Ok(t) => ToolOutput::ok(t),
                Err(e) => ToolOutput::err(e.to_string()),
            }
        })
    }
}

pub struct HistorySearch;

impl Tool for HistorySearch {
    fn prints_much(&self) -> bool {
        true
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "history_search".into(),
            description: "Search every line of every session entry, including dropped ones and the whole of cut tool output, with a regular expression. Returns `#id:line` matches.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "A regular expression." },
                    "kinds": { "type": "array", "description": "Only these kinds of entry: user, assistant, tool_result (default: all).", "items": { "type": "string" } },
                    "limit": { "type": "integer", "description": "Maximum number of matches (default 50)." },
                    "all_sessions": { "type": "boolean", "description": "Also search the case's other sessions; their matches are prefixed `session <id>` (default false)." }
                },
                "required": ["pattern"]
            }),
        }
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let Some(pattern) = str_arg(&args, "pattern") else {
                return ToolOutput::err("history_search needs a `pattern`");
            };
            let kinds: Vec<String> = args
                .get("kinds")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|k| k.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let limit = usize_arg(&args, "limit").unwrap_or(50);
            let all = args
                .get("all_sessions")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if all && let Some(refusal) = other_sessions_off(ctx) {
                return refusal;
            }
            let store = ctx.store.lock().unwrap();
            let found = if all {
                history::search_case(&store, pattern, &kinds, limit)
            } else {
                history::search(&store, pattern, &kinds, limit)
            };
            match found {
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
            description: "Overwrite the manifest's step list. A step is a short summary of an investigative question or milestone, not a micro-action, with a 3-word status. Needs a session goal. Unrelated to scenario phases.".into(),
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
                _ => {
                    return ToolOutput::err(
                        "update_steps needs `steps`: a list of {summary, status}",
                    );
                }
            };
            let cfg = Settings::load(&ctx.paths).manifest;
            let mut store = ctx.store.lock().unwrap();
            let mut state = SessionState::load(&store).manifest;
            let out = manifest::update_steps(&mut state, &cfg, &steps);
            if out.persist {
                // Steps are the session's; the goal and guidelines are untouched here, so
                // nothing is written to the case.
                let _ = save_manifest(&mut store, &state, &Author::agent(""));
            }
            ToolOutput::ok(out.text)
        })
    }
}
