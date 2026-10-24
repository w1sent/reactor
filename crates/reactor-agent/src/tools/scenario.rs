//! Tools for scenarios: finish a phase, list the scenarios there are, write one for the case
//! or for every case, and amend the one that is running
//! ([ADR-0065](../../../../docs/adr/0065-a-scenario-is-general-starts-from-a-question-and-can-be-written-for-the-case.md)).
//!
//! The agent writes scenarios; it never starts one. Starting changes the goal and the system
//! prompt for the rest of the session, which is the person's call: the GUI shows a saved
//! scenario as a card with *Start*.

use std::path::Path;

use reactor_context::scenario::{self, MANUAL, Persist, SKIPPED, Scenario};
use reactor_context::settings::Settings;
use reactor_core::Paths;
use reactor_core::scenarios::{self, Library, Phase, Scope};
use reactor_db::Case;
use reactor_db::graph::{self, Batch, Content, Filter, Kind, Put, query};
use reactor_db::manifest::Author;
use serde_json::{Value, json};

use super::{BoxFut, Tool, ToolCtx, ToolOutput, str_arg};
use crate::context::{KEY_SCENARIO, SessionState, save_state};
use crate::llm::ToolSpec;

/// The group the authoring tools are loaded with (ADR-0055).
pub const GROUP: &str = "scenario";

/// The tag of a phase summary kept in the graph.
pub const TAG: &str = "scenario";

/// Where this session's scenarios are: the case's own, the config directory's, then the
/// checkout's (`bundled`, which may be the config directory itself).
pub fn library(paths: &Paths, case_dir: &Path, bundled: &Path) -> Library {
    Library::new(
        Some(case_dir),
        paths.scenarios_dir(),
        Some(bundled.to_path_buf()),
    )
}

/// The answer when the scenario-editing switch was turned off after the tool list was made.
fn editing_off(ctx: &ToolCtx) -> Option<ToolOutput> {
    let global = Settings::load(&ctx.paths).memory;
    (!ctx.state().memory.over(&global).scenario_editing()).then(|| {
        ToolOutput::err(
            "this session does not edit scenarios (the scenario-editing switch is off).",
        )
    })
}

impl ToolCtx {
    pub fn scenarios(&self) -> Library {
        let case_dir = self.store.lock().unwrap().case().dir().to_path_buf();
        library(&self.paths, &case_dir, &self.scenarios_dir)
    }
}

// -- phase summaries in the graph -------------------------------------------------------------

/// Keep what a phase concluded in the case's graph, as a note tagged [`TAG`], when the
/// session is attached to it. A placeholder (skipped, advanced without a word) is not kept.
#[allow(clippy::too_many_arguments)]
pub fn keep_summary(
    paths: &Paths,
    state: &SessionState,
    case: &Case,
    session: &str,
    entry: u64,
    author: Author,
    scenario_id: &str,
    phase: usize,
    title: &str,
    summary: &str,
) {
    let global = Settings::load(paths).memory;
    let summary = summary.trim();
    if !state.memory.over(&global).case_graph()
        || summary.is_empty()
        || summary == SKIPPED
        || summary == MANUAL
    {
        return;
    }
    let mut content = Content::value(summary);
    content.tags = vec![TAG.to_string()];
    content.props.insert("scenario".into(), json!(scenario_id));
    content.props.insert("phase".into(), json!(phase + 1));
    content.props.insert("phase_title".into(), json!(title));
    let put = Put {
        content,
        ..Put::new(Kind::Annotation, "analysis.note")
    };
    let batch = Batch {
        on_behalf_of: (author.kind != "analyst").then(Author::analyst),
        session: Some(session.to_string()),
        entry: Some(entry),
        ..Batch::new(author)
    }
    .put(put);
    let _ = graph::apply(case.db(), batch);
}

/// The phase summaries earlier scenarios left in the case, newest first, as lines for the
/// first briefing of a new one. Empty when the session is not attached to the graph.
pub fn earlier_conclusions(paths: &Paths, state: &SessionState, case: &Case) -> String {
    let global = Settings::load(paths).memory;
    if !state.memory.over(&global).case_graph() {
        return String::new();
    }
    let found = query::find(
        case.db(),
        &Filter {
            kind: Some(Kind::Annotation),
            tag: Some(TAG.into()),
            limit: 12,
            ..Filter::default()
        },
    )
    .unwrap_or_default();
    if found.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n\n## What earlier scenarios in this case concluded\n\nNewest first; graph notes tagged `scenario` (`case_get` with `graph:?kind=annotation&tag=scenario` lists them, and a reference reads one).",
    );
    for e in found {
        let p = &e.content.props;
        let text: String = e
            .content
            .value
            .unwrap_or_default()
            .chars()
            .take(400)
            .collect();
        out.push_str(&format!(
            "\n- {} {}. {}: {} ({})",
            p.get("scenario").and_then(Value::as_str).unwrap_or("?"),
            p.get("phase").and_then(Value::as_u64).unwrap_or(0),
            p.get("phase_title").and_then(Value::as_str).unwrap_or(""),
            text.split_whitespace().collect::<Vec<_>>().join(" "),
            reactor_db::reference::Reference::graph(&e.id).short()
        ));
    }
    out
}

// -- reactor_phase_complete -------------------------------------------------------------------

/// Finish the current scenario phase and receive the next briefing.
pub struct PhaseComplete;

impl Tool for PhaseComplete {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: scenario::TOOL_NAME.into(),
            description: "Mark the current scenario phase complete and receive the next phase's briefing. Call it once, when the phase's work is finished, with a summary of what you concluded; not to narrate progress. Unrelated to the manifest's steps (update_steps).".into(),
            parameters: json!({
                "type": "object",
                "properties": { "summary": { "type": "string", "description": "What you concluded in this phase. Required before the scenario advances. Kept in the case's graph." } },
                "required": ["summary"]
            }),
        }
    }

    fn available(&self, state: &SessionState) -> bool {
        state.scenario.is_some()
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let Some(summary) = str_arg(&args, "summary") else {
                return ToolOutput::err("reactor_phase_complete needs a `summary`");
            };
            let lib = ctx.scenarios();
            let adv = {
                let mut store = ctx.store.lock().unwrap();
                let state = SessionState::load(&store);
                let mut sc = Scenario {
                    lib: &lib,
                    state: state.scenario.clone(),
                };
                let adv = sc.advance(summary);
                if let Some(s) = &state.scenario {
                    let title = lib
                        .load(&s.scenario_id)
                        .get(s.step_index)
                        .map(|p| p.title.clone())
                        .unwrap_or_default();
                    keep_summary(
                        &ctx.paths,
                        &state,
                        store.case(),
                        store.session(),
                        store.head(),
                        Author::agent(&ctx.model),
                        &s.scenario_id,
                        s.step_index,
                        &title,
                        summary,
                    );
                }
                match &adv.persist {
                    Persist::Nothing => {}
                    Persist::Set(s) => {
                        let _ =
                            save_state(&mut store, KEY_SCENARIO, serde_json::to_value(s).unwrap());
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

// -- reading the phases an argument gives --------------------------------------------------------

fn phases_arg(args: &Value) -> Result<Vec<Phase>, ToolOutput> {
    let Some(list) = args.get("phases").and_then(Value::as_array) else {
        return Err(ToolOutput::err(
            "needs `phases`: a list of {title, toolset?, body}",
        ));
    };
    let mut out = Vec::new();
    for (i, p) in list.iter().enumerate() {
        let text = |k: &str| {
            p.get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("")
                .to_string()
        };
        let (title, body) = (text("title"), text("body"));
        if title.is_empty() || body.is_empty() {
            return Err(ToolOutput::err(format!(
                "phase {} needs a `title` and a `body`",
                i + 1
            )));
        }
        out.push(Phase {
            title,
            toolset: Some(text("toolset")).filter(|t| !t.is_empty()),
            description: None,
            body,
        });
    }
    Ok(out)
}

fn phase_schema() -> Value {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "toolset": { "type": "string", "description": "A toolset to enable when the phase starts (optional)." },
                "body": { "type": "string", "description": "The briefing, in Markdown: goal, when it applies, what to do, pitfalls, when it is done." }
            },
            "required": ["title", "body"]
        }
    })
}

/// The toolsets there are, for a warning about a phase that names one that is not.
fn toolset_ids(paths: &Paths) -> Vec<String> {
    reactor_core::commands::toolsets_list(paths)
        .map(|d| d.report.toolsets.into_iter().map(|t| t.id).collect())
        .unwrap_or_default()
}

fn unknown_toolsets(phases: &[Phase], known: &[String]) -> String {
    let unknown: Vec<String> = phases
        .iter()
        .filter_map(|p| p.toolset.clone())
        .filter(|t| !known.is_empty() && !known.contains(t))
        .collect();
    if unknown.is_empty() {
        String::new()
    } else {
        format!(
            "\nNote: not a toolset here: {} (known: {}).",
            unknown.join(", "),
            known.join(", ")
        )
    }
}

// -- scenario_list ------------------------------------------------------------------------------

pub struct ScenarioList;

impl Tool for ScenarioList {
    fn group(&self) -> Option<&'static str> {
        Some(GROUP)
    }
    fn available(&self, state: &SessionState) -> bool {
        state.memory.scenario_editing()
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "scenario_list".into(),
            description: "List the scenarios (multi-phase analysis plans) with where each is kept and what it is for, or read one in full by `id`.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "id": { "type": "string", "description": "A scenario to read in full (default: list them all)." } }
            }),
        }
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            if let Some(off) = editing_off(ctx) {
                return off;
            }
            let lib = ctx.scenarios();
            if let Some(id) = str_arg(&args, "id") {
                let Some((scope, _)) = lib.find(id) else {
                    return ToolOutput::err(format!("no scenario \"{id}\""));
                };
                let phases = lib.load(id);
                let mut out = format!(
                    "# {id} ({})\n{}",
                    scope.as_str(),
                    phases[0]
                        .description
                        .as_deref()
                        .unwrap_or("(no description)")
                );
                for (i, p) in phases.iter().enumerate() {
                    out.push_str(&format!(
                        "\n\n## {}. {}{}\n\n{}",
                        i + 1,
                        p.title,
                        p.toolset
                            .as_deref()
                            .map(|t| format!(" [toolset: {t}]"))
                            .unwrap_or_default(),
                        p.body
                    ));
                }
                return ToolOutput::ok(out);
            }
            let listed = lib.list();
            if listed.is_empty() {
                return ToolOutput::ok("no scenarios");
            }
            ToolOutput::ok(
                listed
                    .iter()
                    .map(|l| {
                        format!(
                            "- {} ({}): {} -- phases: {}",
                            l.id,
                            l.scope.as_str(),
                            l.description.as_deref().unwrap_or("(no description)"),
                            l.titles.join(" / ")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        })
    }
}

// -- scenario_save ------------------------------------------------------------------------------

pub struct ScenarioSave;

impl Tool for ScenarioSave {
    fn group(&self) -> Option<&'static str> {
        Some(GROUP)
    }
    fn available(&self, state: &SessionState) -> bool {
        state.memory.scenario_editing()
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "scenario_save".into(),
            description: "Save a scenario: phases the agent is briefed on one at a time. `case` keeps it in this case and wins over a global one of the same id; `global` is for every case. Replaces one of the same id in that place. It is not started: the person starts it.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Lower-case letters, digits, - and _." },
                    "scope": { "type": "string", "enum": ["case", "global"], "description": "Default: case." },
                    "description": { "type": "string", "description": "One line: what it is for and when to use it." },
                    "phases": phase_schema()
                },
                "required": ["id", "description", "phases"]
            }),
        }
    }
    fn guidelines(&self) -> &'static [&'static str] {
        &[
            "Write a scenario phase as a method for any case like this one: what to do and how to tell it is done, with no paths the case may lack; put this case's specifics in the goal the person starts it with.",
        ]
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            if let Some(off) = editing_off(ctx) {
                return off;
            }
            let Some(id) = str_arg(&args, "id").map(str::trim) else {
                return ToolOutput::err("scenario_save needs an `id`");
            };
            let scope = match str_arg(&args, "scope").unwrap_or("case") {
                "case" => Scope::Case,
                "global" => Scope::Global,
                other => {
                    return ToolOutput::err(format!("`scope` is case or global, not \"{other}\""));
                }
            };
            let phases = match phases_arg(&args) {
                Ok(p) => p,
                Err(e) => return e,
            };
            if ctx.state().scenario.is_some_and(|s| s.scenario_id == id) {
                return ToolOutput::err(format!(
                    "\"{id}\" is running: change what is left of it with scenario_amend"
                ));
            }
            let lib = ctx.scenarios();
            let Some(dir) = lib.dir(scope).map(Path::to_path_buf) else {
                return ToolOutput::err("that place is not available here");
            };
            let description = str_arg(&args, "description");
            match scenarios::save(&dir, id, description, &phases) {
                Ok(_) => ToolOutput::ok(format!(
                    "saved scenario \"{id}\" ({}, {} phases). The person starts it.{}",
                    scope.as_str(),
                    phases.len(),
                    unknown_toolsets(&phases, &toolset_ids(&ctx.paths))
                )),
                Err(e) => ToolOutput::err(e.to_string()),
            }
        })
    }
}

// -- scenario_amend -----------------------------------------------------------------------------

pub struct ScenarioAmend;

impl Tool for ScenarioAmend {
    fn group(&self) -> Option<&'static str> {
        Some(GROUP)
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "scenario_amend".into(),
            description: "Replace the phases after the current one in the running scenario. The phases done and the current one stay. Saved as this case's copy of the scenario.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "phases": phase_schema() },
                "required": ["phases"]
            }),
        }
    }
    fn guidelines(&self) -> &'static [&'static str] {
        &["Amend a running scenario only after the person agreed to the change."]
    }
    fn available(&self, state: &SessionState) -> bool {
        state.scenario.is_some() && state.memory.scenario_editing()
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            if let Some(off) = editing_off(ctx) {
                return off;
            }
            let Some(state) = ctx.state().scenario else {
                return ToolOutput::err("no scenario is running");
            };
            let rest = match args.get("phases").and_then(Value::as_array) {
                Some(a) if a.is_empty() => vec![],
                _ => match phases_arg(&args) {
                    Ok(p) => p,
                    Err(e) => return e,
                },
            };
            let lib = ctx.scenarios();
            let mut phases = lib.load(&state.scenario_id);
            if phases.is_empty() {
                return ToolOutput::err(format!("the files of \"{}\" are gone", state.scenario_id));
            }
            let description = phases[0].description.clone();
            phases.truncate(state.step_index + 1);
            phases.extend(rest);
            let Some(dir) = lib.dir(Scope::Case).map(Path::to_path_buf) else {
                return ToolOutput::err("this session has no case to keep it in");
            };
            match scenarios::save(&dir, &state.scenario_id, description.as_deref(), &phases) {
                Ok(_) => ToolOutput::ok(format!(
                    "\"{}\" now has {} phases; you are on {}. Saved for this case.{}",
                    state.scenario_id,
                    phases.len(),
                    state.step_index + 1,
                    unknown_toolsets(&phases, &toolset_ids(&ctx.paths))
                )),
                Err(e) => ToolOutput::err(e.to_string()),
            }
        })
    }
}
