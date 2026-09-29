//! `reactor-context` against what the pi extensions actually said and did.
//!
//! The goldens in `tests/golden/*.json` are captured from the TypeScript
//! extensions, run through pi's own loader, by
//! `tests/extensions/golden/capture.mjs` (MIGRATE.md phase 3). Each is a scripted
//! session: operations, and beside each the notifications, session entries,
//! prompt text, tool results and side effects the extension produced. This file
//! replays the same operations against the Rust modules and demands the same
//! answers — every key, every byte.
//!
//! The extensions are frozen (ADR-0035), so a failure here is a defect in the
//! port, not drift in the spec.

use std::fs;
use std::path::{Path, PathBuf};

use reactor_context::settings::Settings;
use reactor_context::{Notice, identity, manifest, reporting, scenario};
use serde_json::{Map, Value, json};

fn golden(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(format!("{name}.json"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

fn notify(notices: &[Notice]) -> Value {
    serde_json::to_value(notices).unwrap()
}

fn appended(block: Option<String>) -> Value {
    match block {
        Some(b) => Value::String(format!("\n\n{b}")),
        None => Value::Null,
    }
}

/// `Some(state)` → the one entry it produces; `None` → no entry.
fn entries<T: serde::Serialize>(persist: bool, state: &T) -> Value {
    if persist { json!([serde_json::to_value(state).unwrap()]) } else { json!([]) }
}

fn check(case: &str, i: usize, op: &Value, got: Map<String, Value>) {
    let want = &op["out"];
    assert_eq!(&Value::Object(got), want, "case {case:?}, op #{i}: {}", {
        let mut o = op.clone();
        o.as_object_mut().unwrap().remove("out");
        o
    });
}

// -- manifest -------------------------------------------------------------------

#[test]
fn manifest_matches_the_goal_setting_extension() {
    for case in golden("manifest") {
        let name = s(&case, "name");
        let cfg = Settings::from_json(&json!({ "manifest": case["config"] })).manifest;
        let mut state = manifest::State::default();
        for (i, op) in case["ops"].as_array().unwrap().iter().enumerate() {
            let mut out = Map::new();
            match s(op, "op") {
                "command" => {
                    let e = manifest::command(&mut state, &cfg, s(op, "name"), s(op, "args")).expect("a manifest command");
                    out.insert("notify".into(), notify(&e.notices));
                    out.insert("entries".into(), entries(e.persist, &state));
                }
                "tool" => {
                    let steps: Vec<manifest::Step> = serde_json::from_value(op["steps"].clone()).unwrap();
                    let t = manifest::update_steps(&mut state, &cfg, &steps);
                    out.insert("text".into(), t.text.into());
                    out.insert("notify".into(), json!([]));
                    out.insert("entries".into(), entries(t.persist, &state));
                }
                "prompt" => {
                    out.insert("appended".into(), appended(manifest::block(&state, &cfg)));
                    out.insert("notify".into(), json!([]));
                    out.insert("entries".into(), json!([]));
                }
                "derive" => match manifest::derive_scope(s(op, "args")) {
                    Err(n) => {
                        out.insert("notify".into(), notify(&[n]));
                        out.insert("entries".into(), json!([]));
                    }
                    Ok(scope) => {
                        out.insert("systemPrompt".into(), manifest::DERIVE_SYSTEM.into());
                        // The transcript tail is empty in these sessions.
                        out.insert("userMessage".into(), manifest::derive_task(&scope).into());
                        let e = manifest::apply_derivation(&mut state, &cfg, &scope, s(op, "response"));
                        out.insert("notify".into(), notify(&e.notices));
                        out.insert("entries".into(), entries(e.persist, &state));
                    }
                },
                other => panic!("unknown op {other}"),
            }
            out.insert("advertised".into(), state.tool_usable().into());
            check(name, i, op, out);
        }
    }
}

// -- identity -------------------------------------------------------------------

#[test]
fn identity_matches_the_identity_extension() {
    for case in golden("identity") {
        let name = s(&case, "name");
        let mut cfg = Settings::from_json(&json!({ "identity": case["config"] })).identity;
        let mut state = identity::State::default();
        // The file the extension keeps: what the case started with, then whatever it saved.
        let mut file: Value = case["config"].clone();
        for (i, op) in case["ops"].as_array().unwrap().iter().enumerate() {
            let mut out = Map::new();
            match s(op, "op") {
                "command" => {
                    let args = s(op, "args");
                    let e = if identity::wants_editor(args) {
                        // The editor needs a terminal; in `rpc` mode it says so.
                        assert_eq!(s(op, "mode"), "rpc", "only the no-terminal path is captured");
                        identity::editor_needs_terminal()
                    } else {
                        identity::command(&mut state, &mut cfg, args)
                    };
                    if e.save_settings {
                        file = serde_json::to_value(&cfg).unwrap();
                    }
                    out.insert("notify".into(), notify(&e.notices));
                    out.insert("entries".into(), entries(e.persist, &state));
                }
                "prompt" => {
                    out.insert("appended".into(), appended(identity::block(&state, &cfg)));
                    out.insert("notify".into(), json!([]));
                    out.insert("entries".into(), json!([]));
                }
                other => panic!("unknown op {other}"),
            }
            out.insert("settings".into(), file.clone());
            check(name, i, op, out);
        }
    }
}

// -- reporting ------------------------------------------------------------------

fn set_mtime(path: &Path, secs: u64) {
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
}

#[test]
fn reporting_matches_the_reporting_extension() {
    for case in golden("reporting") {
        let name = s(&case, "name");
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Settings::from_json(&json!({ "reporting": case["config"] })).reporting;
        let mut session = reporting::SessionState::default();
        let mut tracker = reporting::Tracker::new();
        let mut file: Value = case["config"].clone();

        for (i, op) in case["ops"].as_array().unwrap().iter().enumerate() {
            let mut out = Map::new();
            let (mut notices, mut persist, mut user_messages, mut navigated) = (vec![], false, vec![], vec![]);
            match s(op, "op") {
                "report" => {
                    let e = reporting::command(&mut session, &mut tracker, &mut cfg, s(op, "args"));
                    notices = e.notices;
                    persist = e.persist;
                    if e.save_folder.is_some() {
                        file = serde_json::to_value(&cfg).unwrap();
                    }
                }
                "prompt" => {
                    let block = tracker.before_agent_start(&session, &cfg, s(op, "prompt"));
                    out.insert("appended".into(), appended(block));
                }
                "write" => {
                    let target = dir.path().join(s(op, "path"));
                    fs::create_dir_all(target.parent().unwrap()).unwrap();
                    fs::write(&target, s(op, "content")).unwrap();
                    set_mtime(&target, op["mtime"].as_u64().unwrap());
                }
                "tool_end" => tracker.tool_end(&session, reporting::take_snapshot(&dir.path().join(&cfg.folder))),
                "context" => out.insert("nag".into(), tracker.nag(&session, &cfg).map(Value::String).unwrap_or(Value::Null)).map(drop).unwrap_or(()),
                "settled" => match tracker.settled(&session, &cfg) {
                    reporting::Settled::Nothing => {}
                    reporting::Settled::Revert => user_messages.push("/reactor-report-enforce".to_string()),
                    reporting::Settled::GaveUp(n) => notices.push(n),
                },
                "enforce" => {
                    // The last user message in the branch is `u1`.
                    navigated.push("u1".to_string());
                    user_messages.push(tracker.demand(&cfg));
                }
                other => panic!("unknown op {other}"),
            }
            out.insert("notify".into(), notify(&notices));
            out.insert("entries".into(), entries(persist, &session));
            out.insert("userMessages".into(), json!(user_messages));
            out.insert("navigated".into(), json!(navigated));
            out.insert("settings".into(), file.clone());
            check(name, i, op, out);
        }
    }
}

// -- scenario -------------------------------------------------------------------

fn write_scenarios(root: &Path, scenarios: &Value) {
    for (id, steps) in scenarios.as_object().unwrap() {
        let d = root.join(id);
        fs::create_dir_all(&d).unwrap();
        for (i, content) in steps.as_array().unwrap().iter().enumerate() {
            fs::write(d.join(format!("{:02}.md", i + 1)), content.as_str().unwrap()).unwrap();
        }
    }
}

fn scenario_entries(p: &scenario::Persist) -> Value {
    match p {
        scenario::Persist::Nothing => json!([]),
        scenario::Persist::Set(s) => json!([serde_json::to_value(s).unwrap()]),
        scenario::Persist::Clear => json!([null]),
    }
}

#[test]
fn scenario_matches_the_scenario_extension() {
    for case in golden("scenario") {
        let name = s(&case, "name");
        let dir: PathBuf = tempfile::tempdir().unwrap().keep();
        write_scenarios(&dir, &case["scenarios"]);
        let mut sc = scenario::Scenario { dir: &dir, state: None };
        let toolset_call = |t: &String| format!("toolsets enable {t} --format json");

        for (i, op) in case["ops"].as_array().unwrap().iter().enumerate() {
            let mut out = Map::new();
            match s(op, "op") {
                "command" => {
                    let e = sc.command(s(op, "args"));
                    out.insert("notify".into(), notify(&e.notices));
                    out.insert("entries".into(), scenario_entries(&e.persist));
                    out.insert(
                        "messages".into(),
                        e.messages.iter().map(|m| json!({"content": m.content, "display": true})).collect::<Vec<_>>().into(),
                    );
                    out.insert("reactorCalls".into(), e.activate_toolsets.iter().map(toolset_call).collect::<Vec<_>>().into());
                }
                "tool" => {
                    let adv = sc.advance(s(op, "summary"));
                    out.insert("text".into(), adv.text.into());
                    out.insert("notify".into(), json!([]));
                    out.insert("entries".into(), scenario_entries(&adv.persist));
                    out.insert("messages".into(), json!([]));
                    out.insert("reactorCalls".into(), adv.activate_toolset.iter().map(toolset_call).collect::<Vec<_>>().into());
                }
                other => panic!("unknown op {other}"),
            }
            out.insert("advertised".into(), sc.tool_advertised().into());
            check(name, i, op, out);
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
