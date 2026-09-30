//! The loop, driven by a scripted model.

mod common;

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

use reactor_agent::agent::{Agent, AgentConfig, Event};
use reactor_agent::budget::{Summarizer, SummaryRequest};
use reactor_agent::context::{Msg, SessionState, project};
use reactor_agent::entry::{Block, Kind, Mode};
use reactor_agent::llm::{Reply, ScriptedLlm};
use reactor_agent::prompt::RegistryView;
use reactor_agent::store::Store;
use reactor_agent::tools::Tools;
use reactor_agent::{Error, Result};
use reactor_core::Paths;
use reactor_core::paths::Shipped;
use serde_json::json;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

/// Answers with a canned summary, or fails; remembers what it was asked.
struct Sum {
    answer: std::result::Result<String, String>,
    seen: Mutex<Vec<SummaryRequest>>,
}

impl Sum {
    fn says(t: &str) -> Arc<Self> {
        Arc::new(Sum { answer: Ok(t.into()), seen: Mutex::new(vec![]) })
    }
    fn fails(t: &str) -> Arc<Self> {
        Arc::new(Sum { answer: Err(t.into()), seen: Mutex::new(vec![]) })
    }
}

impl Summarizer for Sum {
    async fn summarize(&self, req: SummaryRequest) -> Result<String> {
        self.seen.lock().unwrap().push(req);
        self.answer.clone().map_err(Error::Model)
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    agent: Agent<Arc<ScriptedLlm>, Arc<Sum>>,
    llm: Arc<ScriptedLlm>,
    sum: Arc<Sum>,
    events: UnboundedReceiver<Event>,
}

fn registry(block: &str, usable: &[&str]) -> Arc<dyn Fn() -> RegistryView + Send + Sync> {
    let view = RegistryView { block: block.to_string(), skill_dirs: vec![], usable: usable.iter().map(|s| s.to_string()).collect::<HashSet<_>>() };
    Arc::new(move || view.clone())
}

fn rig(replies: Vec<Result<Reply>>, sum: Arc<Sum>, window: u64) -> Rig {
    rig_with(replies, sum, window, |_| {})
}

fn rig_with(replies: Vec<Result<Reply>>, sum: Arc<Sum>, window: u64, tweak: impl FnOnce(&mut AgentConfig)) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let store = Store::create(root.join("session"), "s", &cwd).unwrap();
    let paths = Paths::new(root.join("cfg"), Shipped::Embedded);
    let mut cfg = AgentConfig::new(cwd.clone(), paths, window);
    cfg.registry = registry("## Available RE tools (this machine)\n\nsh   shell", &["sh"]);
    cfg.scenarios_dir = root.join("scenarios");
    cfg.budget.reserve = 100;
    tweak(&mut cfg);
    let llm = Arc::new(ScriptedLlm::new(replies));
    let (agent, events) = Agent::new(llm.clone(), sum.clone(), store, Tools::standard(cwd), cfg);
    Rig { _dir: dir, root, agent, llm, sum, events }
}

fn drain(rx: &mut UnboundedReceiver<Event>) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

async fn run(rig: &Rig, text: &str) -> Result<reactor_agent::agent::Outcome> {
    rig.agent.prompt(text, CancellationToken::new()).await
}

fn kinds(rig: &Rig) -> Vec<&'static str> {
    rig.agent.store().lock().unwrap().all().iter().map(|e| e.kind.label()).collect()
}

fn results(rig: &Rig) -> Vec<String> {
    rig.agent
        .store()
        .lock()
        .unwrap()
        .all()
        .iter()
        .filter_map(|e| match &e.kind {
            Kind::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

fn bash_call(id: &str, cmd: &str) -> Result<Reply> {
    ScriptedLlm::call("", &[(id, "bash", json!({ "command": cmd }))])
}

// -- a plain turn ---------------------------------------------------------------------------

#[tokio::test]
async fn a_turn_without_tools_records_the_exchange_and_streams_it() {
    let mut r = rig(vec![ScriptedLlm::say("hello there")], Sum::says("-"), 100_000);
    let out = run(&r, "hi").await.unwrap();
    assert_eq!((out.rounds, out.tool_calls, out.reductions), (1, 0, 0));
    assert_eq!(kinds(&r), ["session", "user", "assistant"]);

    let events = drain(&mut r.events);
    assert!(events.contains(&Event::Text("hello there".into())));
    assert_eq!(events.last(), Some(&Event::Finished));

    // The request carried the tools and a system prompt with the registry block.
    let req = &r.llm.requests()[0];
    assert!(req.system.contains("You are REactor"));
    assert!(req.system.contains("## Available RE tools (this machine)"));
    assert_eq!(req.messages, [Msg::User { text: "hi".into() }]);
    let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["bash", "read", "write", "edit", "history_index", "history_read", "history_search"], "the state-gated tools are not advertised");
}

#[tokio::test]
async fn the_system_prompt_is_byte_identical_across_requests_when_nothing_changed() {
    // The property the prompt cache depends on (ADR-0006).
    let r = rig(vec![bash_call("c1", "true"), ScriptedLlm::say("done")], Sum::says("-"), 100_000);
    run(&r, "go").await.unwrap();
    let reqs = r.llm.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].system, reqs[1].system);
    assert_eq!(reqs[0].tools, reqs[1].tools);
}

#[tokio::test]
async fn a_model_failure_leaves_the_users_message_and_no_half_reply() {
    let r = rig(vec![Err(Error::Model("503 overloaded".into()))], Sum::says("-"), 100_000);
    let err = run(&r, "hi").await.unwrap_err();
    assert!(err.to_string().contains("503"));
    assert_eq!(kinds(&r), ["session", "user"]);
}

// -- tools ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_tool_round_trip_records_call_and_result_and_sends_the_result_back() {
    let mut r = rig(vec![bash_call("c1", "echo hello"), ScriptedLlm::say("it printed hello")], Sum::says("-"), 100_000);
    let out = run(&r, "run it").await.unwrap();
    assert_eq!((out.rounds, out.tool_calls), (2, 1));
    assert_eq!(kinds(&r), ["session", "user", "assistant", "tool_result", "assistant"]);
    assert_eq!(results(&r), ["hello"]);

    let second = &r.llm.requests()[1];
    assert!(matches!(&second.messages[1], Msg::Assistant { blocks } if matches!(&blocks[0], Block::ToolCall { name, .. } if name == "bash")));
    assert_eq!(second.messages[2], Msg::ToolResult { call_id: "c1".into(), name: "bash".into(), content: "hello".into(), is_error: false });

    let events = drain(&mut r.events);
    assert!(events.iter().any(|e| matches!(e, Event::ToolStart { name, .. } if name == "bash")));
    assert!(events.iter().any(|e| matches!(e, Event::ToolOutput { chunk, .. } if chunk == "hello\n")));
    assert!(events.iter().any(|e| matches!(e, Event::ToolEnd { is_error: false, .. })));
}

#[tokio::test]
async fn several_calls_in_one_reply_all_run_and_all_get_results_in_order() {
    let r = rig(
        vec![
            ScriptedLlm::call("", &[("a", "bash", json!({"command": "echo one"})), ("b", "bash", json!({"command": "echo two"}))]),
            ScriptedLlm::say("ok"),
        ],
        Sum::says("-"),
        100_000,
    );
    run(&r, "go").await.unwrap();
    assert_eq!(results(&r), ["one", "two"]);
}

#[tokio::test]
async fn an_unknown_tool_is_an_error_result_not_a_crash() {
    let r = rig(vec![ScriptedLlm::call("", &[("c", "teleport", json!({}))]), ScriptedLlm::say("sorry")], Sum::says("-"), 100_000);
    run(&r, "go").await.unwrap();
    assert_eq!(results(&r), ["unknown tool `teleport`"]);
    let store = r.agent.store();
    let store = store.lock().unwrap();
    assert!(matches!(&store.all()[3].kind, Kind::ToolResult { is_error: true, .. }));
}

#[tokio::test]
async fn the_bash_shell_persists_between_calls() {
    let r = rig(
        vec![
            bash_call("1", "cd /tmp && export REACTOR_T=42"),
            bash_call("2", "pwd; echo $REACTOR_T"),
            bash_call("3", "pwd; echo $REACTOR_T"),
            ScriptedLlm::say("done"),
        ],
        Sum::says("-"),
        100_000,
    );
    run(&r, "go").await.unwrap();
    assert_eq!(results(&r), ["", "/tmp\n42", "/tmp\n42"]);
}

#[tokio::test]
async fn separate_sessions_do_not_share_state_and_restart_resets() {
    let r = rig(
        vec![
            ScriptedLlm::call("", &[("1", "bash", json!({"command": "export A=one", "session": "s1"}))]),
            ScriptedLlm::call("", &[("2", "bash", json!({"command": "echo [$A]", "session": "s2"}))]),
            ScriptedLlm::call("", &[("3", "bash", json!({"command": "echo [$A]", "session": "s1"}))]),
            ScriptedLlm::call("", &[("4", "bash", json!({"command": "echo [$A]", "session": "s1", "restart": true}))]),
            ScriptedLlm::say("done"),
        ],
        Sum::says("-"),
        100_000,
    );
    run(&r, "go").await.unwrap();
    assert_eq!(results(&r), ["", "[]", "[one]", "[]"]);
}

#[tokio::test]
async fn stderr_is_merged_and_a_nonzero_exit_is_reported_not_an_error() {
    let r = rig(vec![bash_call("1", "echo out; echo err >&2; (exit 3)"), bash_call("2", "echo still alive"), ScriptedLlm::say("ok")], Sum::says("-"), 100_000);
    run(&r, "go").await.unwrap();
    let res = results(&r);
    assert!(res[0].contains("out") && res[0].contains("err"), "{}", res[0]);
    assert!(res[0].ends_with("[exit code 3]"), "{}", res[0]);
    assert_eq!(res[1], "still alive");
}

#[tokio::test]
async fn exit_inside_a_command_ends_the_shell_and_the_next_call_gets_a_fresh_one() {
    let r = rig(vec![bash_call("1", "export GONE=1; exit 7"), bash_call("2", "echo [$GONE]"), ScriptedLlm::say("ok")], Sum::says("-"), 100_000);
    run(&r, "go").await.unwrap();
    let res = results(&r);
    assert!(res[0].contains("the shell exited (code 7)"), "{}", res[0]);
    assert_eq!(res[1], "[]");
}

#[tokio::test]
async fn a_command_that_reads_stdin_cannot_swallow_the_rest_of_the_script() {
    let r = rig(vec![bash_call("1", "cat; echo after"), ScriptedLlm::say("ok")], Sum::says("-"), 100_000);
    run(&r, "go").await.unwrap();
    assert_eq!(results(&r), ["after"]);
}

#[tokio::test]
async fn a_timeout_kills_the_shell_and_says_what_was_lost() {
    let r = rig(
        vec![
            ScriptedLlm::call("", &[("1", "bash", json!({"command": "export KEEP=1; sleep 30", "timeout_secs": 1}))]),
            bash_call("2", "echo [$KEEP]"),
            ScriptedLlm::say("ok"),
        ],
        Sum::says("-"),
        100_000,
    );
    let started = std::time::Instant::now();
    run(&r, "go").await.unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "the timeout was not honoured");
    let res = results(&r);
    assert!(res[0].contains("timed out after 1s") && res[0].contains("was reset"), "{}", res[0]);
    assert_eq!(res[1], "[]", "the killed session's variables are gone; a fresh one answers");
    let store = r.agent.store();
    assert!(matches!(&store.lock().unwrap().all()[3].kind, Kind::ToolResult { is_error: true, .. }));
}

// -- truncation and history -----------------------------------------------------------------------

#[tokio::test]
async fn long_output_is_cut_to_head_and_tail_and_kept_whole_and_addressable() {
    let r = rig(
        vec![
            bash_call("1", "for i in $(seq 1 6000); do echo \"line $i of the dump\"; done"),
            ScriptedLlm::call("", &[("2", "history_read", json!({"id": 3, "offset": 3000, "limit": 2}))]),
            ScriptedLlm::call("", &[("3", "history_search", json!({"pattern": "line 4242 "}))]),
            ScriptedLlm::say("found it"),
        ],
        Sum::says("-"),
        1_000_000,
    );
    run(&r, "dump").await.unwrap();
    let res = results(&r);

    // What the model saw: both ends, a marker naming the entry.
    assert!(res[0].starts_with("line 1 of the dump"), "{}", &res[0][..40]);
    assert!(res[0].trim_end().ends_with("line 6000 of the dump"));
    assert!(res[0].contains("bytes elided -- the whole output is #3"), "{}", res[0].lines().find(|l| l.contains("elided")).unwrap_or(""));
    assert!(res[0].len() < 33 * 1024, "{}", res[0].len());

    // Nothing was lost: the blob has every line, history_read and history_search reach them.
    assert_eq!(res[1], "line 3000 of the dump\nline 3001 of the dump\n… [2999 more line(s); continue with offset 3002]");
    assert!(res[2].contains("#3:4242 (tool_result) line 4242 of the dump"), "{}", res[2]);
    let store = r.agent.store();
    let store = store.lock().unwrap();
    let Kind::ToolResult { blob: Some(b), .. } = &store.get(3).unwrap().kind else { panic!("no blob") };
    assert!(String::from_utf8(store.read_blob(b).unwrap()).unwrap().lines().count() == 6000);
}

#[tokio::test]
async fn a_huge_output_spills_to_disk_and_keeps_every_byte() {
    // Well past what is held in memory.
    let r = rig(vec![bash_call("1", "yes 0123456789abcdef | head -c 6000000"), ScriptedLlm::say("ok")], Sum::says("-"), 10_000_000);
    run(&r, "big").await.unwrap();
    let store = r.agent.store();
    let store = store.lock().unwrap();
    let Kind::ToolResult { content, blob: Some(b), .. } = &store.get(3).unwrap().kind else { panic!("no blob") };
    assert!(b.bytes >= 6_000_000 - 20, "{}", b.bytes);
    assert_eq!(store.read_blob(b).unwrap().len() as u64, b.bytes, "the whole thing is on disk");
    assert!(content.len() < 33 * 1024, "and the model's view is bounded");
    assert!(content.contains("bytes elided"));
}

// -- state-gated tools and the prompt ---------------------------------------------------------------

#[tokio::test]
async fn update_steps_is_advertised_only_while_a_goal_is_set_and_the_block_follows_state() {
    let r = rig(
        vec![
            ScriptedLlm::say("first"),
            ScriptedLlm::call("", &[("u", "update_steps", json!({"steps": [{"summary": "find the check", "status": "in progress"}]}))]),
            ScriptedLlm::say("second"),
        ],
        Sum::says("-"),
        100_000,
    );
    run(&r, "one").await.unwrap();
    assert!(!r.llm.requests()[0].tools.iter().any(|t| t.name == "update_steps"));
    assert!(!r.llm.requests()[0].system.contains("Session Manifest"));

    let c = r.agent.command("goal", "crack the license check").unwrap();
    assert_eq!(c.notices[0].message, "goal set: crack the license check");
    run(&r, "two").await.unwrap();

    let req = &r.llm.requests()[1];
    assert!(req.tools.iter().any(|t| t.name == "update_steps"));
    assert!(req.system.contains("Goal: crack the license check"));
    // The tool acted, stored the state, and the next request's block shows the step.
    assert_eq!(results(&r), ["Steps updated: 1 step(s)."]);
    let last = &r.llm.requests()[2];
    assert!(last.system.contains("1. [in progress] find the check"), "{}", last.system);
    assert_eq!(r.agent.session_state().manifest.steps.len(), 1);
}

#[tokio::test]
async fn identity_and_reporting_blocks_and_settings_writes() {
    let r = rig(vec![ScriptedLlm::say("ok")], Sum::says("-"), 100_000);
    r.agent.command("identity", "reverse-engineer").unwrap();
    r.agent.command("report", "on").unwrap();
    r.agent.command("identity", "write my own persona").unwrap();
    r.agent.command("identity", "save mine").unwrap();
    run(&r, "go").await.unwrap();
    let sys = &r.llm.requests()[0].system;
    assert!(sys.contains("## Identity\n\nmy own persona"));
    assert!(sys.contains("## Reporting mode"));
    // `save` wrote the user's identity into settings.json.
    let text = std::fs::read_to_string(r.root.join("cfg/settings.json")).unwrap();
    assert!(text.contains("\"mine\": \"my own persona\""), "{text}");
    assert!(r.agent.command("nonsense", "").is_err());
}

fn write_scenario(root: &Path) {
    let d = root.join("scenarios/inv");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("01.md"), "---\ntitle: Scoping\n---\nDecide what is in scope.\n").unwrap();
    std::fs::write(d.join("02.md"), "---\ntitle: Triage\n---\nRun file, strings, hashes.\n").unwrap();
}

#[tokio::test]
async fn a_scenarios_phase_is_a_block_rebuilt_every_request_and_advances_by_tool_result() {
    let r = rig(
        vec![
            ScriptedLlm::say("starting"),
            ScriptedLlm::call("", &[("p", "reactor_phase_complete", json!({"summary": "scope: one binary"}))]),
            ScriptedLlm::say("triaging"),
            ScriptedLlm::call("", &[("q", "reactor_phase_complete", json!({"summary": "it is packed"}))]),
            ScriptedLlm::say("all done"),
        ],
        Sum::says("-"),
        100_000,
    );
    write_scenario(&r.root);
    let started = r.agent.command("reactor-scenario", "start inv").unwrap();
    assert_eq!(started.trigger_turn.as_deref(), Some("## Phase 1/2: Scoping\n\nDecide what is in scope."));
    run(&r, "begin").await.unwrap();
    run(&r, "next please").await.unwrap();
    run(&r, "and the last").await.unwrap();

    let reqs = r.llm.requests();
    assert!(reqs[0].system.contains("## Phase 1/2: Scoping"));
    assert!(reqs[0].tools.iter().any(|t| t.name == "reactor_phase_complete"));
    assert!(reqs[2].system.contains("## Phase 2/2: Triage") && reqs[2].system.contains("1. Scoping -- scope: one binary"), "{}", reqs[2].system);
    assert!(!reqs[2].system.contains("Phase 1/2"));
    // Finished: the block and the tool are gone.
    let last = reqs.last().unwrap();
    assert!(!last.system.contains("Scenario:"));
    assert!(!last.tools.iter().any(|t| t.name == "reactor_phase_complete"));
    assert!(results(&r)[1].contains("scenario \"inv\" complete"));
    assert!(r.agent.session_state().scenario.is_none());
}

#[tokio::test]
async fn skills_are_offered_by_their_requirements_and_upstream_skills_by_the_registry() {
    let dir = tempfile::tempdir().unwrap();
    let skills = dir.path().join("skills");
    for (name, requires) in [("uses-sh", "[sh]"), ("needs-gdb", "[gdb]"), ("free", "[]")] {
        let d = skills.join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), format!("---\nname: {name}\ndescription: does {name} things\nrequires: {requires}\n---\nbody")).unwrap();
    }
    let upstream = dir.path().join("upstream/bn");
    std::fs::create_dir_all(&upstream).unwrap();
    std::fs::write(upstream.join("SKILL.md"), "---\nname: binja-cli\ndescription: drive Binary Ninja\n---\n").unwrap();
    let up = upstream.display().to_string();

    let r = rig_with(vec![ScriptedLlm::say("ok")], Sum::says("-"), 100_000, |cfg| {
        cfg.skills_dir = Some(skills);
        let view = RegistryView { block: "## Available RE tools (this machine)".into(), skill_dirs: vec![up], usable: ["sh".to_string()].into() };
        cfg.registry = Arc::new(move || view.clone());
    });
    run(&r, "go").await.unwrap();
    let sys = &r.llm.requests()[0].system;
    assert!(sys.contains("**uses-sh**") && sys.contains("**free**") && sys.contains("**binja-cli**"), "{sys}");
    assert!(!sys.contains("needs-gdb"), "a skill for a tool that is not here is not offered");
}

// -- the budget -----------------------------------------------------------------------------------

/// A window small enough that one dump forces a reduction.
const SMALL: u64 = 5_000;

#[tokio::test]
async fn a_dump_that_overflows_inside_a_turn_is_reduced_at_the_tool_result_boundary() {
    // Older work, then a turn whose tool result pushes past the trigger. The reduction
    // lands *before* the next request, with no continue message and no parked turn.
    let mut r = rig(
        vec![
            bash_call("1", "for i in $(seq 1 150); do echo \"padding line number $i to fill the context\"; done"),
            ScriptedLlm::say("noted"),
            bash_call("2", "for i in $(seq 1 150); do echo \"more padding line number $i to fill the context\"; done"),
            ScriptedLlm::say("finished"),
        ],
        Sum::says("EARLIER: looked at two dumps; nothing found"),
        SMALL,
    );
    run(&r, "first, an older question about the sample").await.unwrap();
    let out = run(&r, "now dump twice and tell me").await.unwrap();

    assert!(out.reductions >= 1, "{out:?}");
    let events = drain(&mut r.events);
    let reduced = events.iter().position(|e| matches!(e, Event::Reduced { .. })).expect("a reduction happened");
    let last_req = r.llm.requests().last().unwrap().clone();
    assert!(last_req.messages.iter().any(|m| matches!(m, Msg::User { text } if text.contains("EARLIER: looked at two dumps"))), "the stand-in went into the next request");
    // The originals are all still there.
    let store = r.agent.store();
    let store = store.lock().unwrap();
    assert!(store.all().iter().any(|e| matches!(&e.kind, Kind::User { text } if text.contains("older question"))));
    assert!(store.all().iter().any(|e| matches!(e.kind, Kind::Reduction(_))));
    // No message anywhere is a synthetic "continue".
    assert!(!store.all().iter().any(|e| matches!(&e.kind, Kind::User { text } if text.to_lowercase() == "continue")));
    assert!(reduced > 0);
    // And the summarizer saw the tool output it summarized.
    assert!(r.sum.seen.lock().unwrap()[0].transcript.contains("padding line number"));
}

#[tokio::test]
async fn a_failed_reduction_stops_the_turn_and_still_records_every_call_a_result() {
    // ADR-0037 invariant: never continue after a failed reduction.
    let r = rig(
        vec![
            ScriptedLlm::say("noted"),
            ScriptedLlm::call(
                "",
                &[
                    ("1", "bash", json!({"command": "for i in $(seq 1 300); do echo \"padding line number $i to fill the context up\"; done"})),
                    ("2", "bash", json!({"command": "echo never runs"})),
                ],
            ),
            ScriptedLlm::say("must not be reached"),
        ],
        Sum::fails("429 rate limited"),
        SMALL,
    );
    run(&r, "an older question, so there is something to reduce").await.unwrap();
    let err = run(&r, "now dump").await.unwrap_err();

    assert!(matches!(err, Error::Reduction(_)), "{err}");
    assert!(err.to_string().contains("429 rate limited"));
    assert_eq!(r.llm.requests().len(), 2, "no request was made on a context that did not shrink");
    // Both calls of the recorded reply have results, the second saying why it did not run.
    let res = results(&r);
    assert_eq!(res.len(), 2);
    assert!(res[1].contains("not run: context reduction failed"));
}

#[tokio::test]
async fn a_reduction_that_does_not_make_the_context_fit_stops_the_turn_instead_of_looping() {
    // A summarizer whose "summary" is bigger than the window can never help: the loop
    // must stop rather than reduce forever or send a request that will overflow.
    let bloat = "x".repeat(24_000);
    let replies = (0..12).map(|i| ScriptedLlm::say(&format!("reply {i}"))).collect();
    let r = rig(replies, Sum::says(&bloat), SMALL);
    let mut err = None;
    for i in 0..8 {
        if let Err(e) = run(&r, &format!("question {i}: {}", "y".repeat(4_000))).await {
            err = Some(e);
            break;
        }
    }
    let err = err.expect("the turn must stop once reduction cannot fit the context");
    assert!(matches!(err, Error::Reduction(_) | Error::ReductionBudget { .. }), "{err}");
    let sent = r.llm.requests().len();
    assert!(sent <= 3, "no request went out on a context that did not fit ({sent})");
}

#[tokio::test]
async fn zero_allowed_reductions_in_a_row_means_the_first_one_needed_stops_the_turn() {
    // The guard itself: `max_reductions_in_a_row` is a hard cap on attempts.
    let replies = (0..12).map(|i| ScriptedLlm::say(&format!("reply {i}"))).collect();
    let r = rig_with(replies, Sum::says("s"), SMALL, |cfg| cfg.max_reductions_in_a_row = 0);
    let mut last = Ok(Default::default());
    for i in 0..8 {
        last = run(&r, &format!("question {i}: {}", "y".repeat(4_000))).await;
        if last.is_err() {
            break;
        }
    }
    assert!(matches!(last, Err(Error::ReductionBudget { attempts: 0 })), "{last:?}");
}

#[tokio::test]
async fn a_manual_reduction_previews_then_summarizes_and_can_be_undone() {
    let r = rig(vec![ScriptedLlm::say("a"), ScriptedLlm::say("b"), ScriptedLlm::say("c")], Sum::says("Manual summary."), 30_000);
    for i in 0..3 {
        run(&r, &format!("question {i} {}", "z".repeat(3_000))).await.unwrap();
    }
    let plan = r.agent.preview(Mode::Compact).await.unwrap();
    assert!(plan.reclaimed_tokens() > 0 && plan.needs_summarizer());
    assert_eq!(r.sum.seen.lock().unwrap().len(), 0, "previewing asks no model");

    let id = r.agent.reduce_now(Mode::Compact).await.unwrap();
    assert_eq!(r.sum.seen.lock().unwrap().len(), 1);
    let store = r.agent.store();
    assert!(matches!(&store.lock().unwrap().get(id).unwrap().kind, Kind::Reduction(red) if red.trigger == reactor_agent::entry::Trigger::Manual));
    let reduced = project(&store.lock().unwrap()).len();

    // Undo: an append, and the originals are back in the projection.
    store.lock().unwrap().append(Kind::Restore { reduction: id }).unwrap();
    assert!(project(&store.lock().unwrap()).len() > reduced);
}

// -- cancellation and reporting -------------------------------------------------------------------------

#[tokio::test]
async fn cancelling_mid_batch_kills_the_running_command_and_answers_the_rest() {
    let r = rig(
        vec![ScriptedLlm::call(
            "",
            &[
                ("1", "bash", json!({"command": "sleep 30", "timeout_secs": 60})),
                ("2", "bash", json!({"command": "echo never"})),
            ],
        )],
        Sum::says("-"),
        100_000,
    );
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        c2.cancel();
    });
    let started = std::time::Instant::now();
    let err = r.agent.prompt("sleep", cancel).await.unwrap_err();

    assert!(matches!(err, Error::Cancelled));
    assert!(started.elapsed() < std::time::Duration::from_secs(15));
    let res = results(&r);
    assert_eq!(res.len(), 2, "the log stays replayable: every call has a result");
    assert!(res[0].contains("cancelled"), "{}", res[0]);
    assert!(res[1].contains("not run: the turn was cancelled"));
}

#[tokio::test]
async fn level_two_reporting_reverts_an_undocumented_turn_by_forking() {
    let r = rig(
        vec![
            bash_call("1", "true"),
            bash_call("2", "true"),
            bash_call("3", "true"),
            ScriptedLlm::say("done, but I wrote nothing down"),
            ScriptedLlm::say("now documented"),
        ],
        Sum::says("-"),
        100_000,
    );
    r.agent.command("report", "level 2").unwrap();
    // A tiny threshold so three tool calls trip it.
    std::fs::create_dir_all(r.root.join("cfg")).unwrap();
    std::fs::write(r.root.join("cfg/settings.json"), r#"{"reporting": {"stepThreshold": 2, "maxReverts": 1}}"#).unwrap();

    let out = run(&r, "analyse the sample").await.unwrap();
    assert_eq!(out.reverts, 1);

    let store = r.agent.store();
    let store = store.lock().unwrap();
    // The demand went in as a new user message on a fork from the original.
    let demand = store.all().iter().find(|e| matches!(&e.kind, Kind::User { text } if text.contains("You did not document your findings"))).unwrap();
    let original = store.all().iter().find(|e| matches!(&e.kind, Kind::User { text } if text == "analyse the sample")).unwrap();
    assert_eq!(demand.parent, Some(original.id), "forked from the user's message");
    assert!(store.leaves().len() >= 2, "the reverted turn is a dead branch, still in the log");
    let SessionState { reporting, .. } = SessionState::load(&store);
    assert!(reporting.is_enabled());
}

// -- resuming -------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_session_resumes_from_its_log_identically() {
    let r = rig(vec![bash_call("1", "echo hi"), ScriptedLlm::say("done")], Sum::says("-"), 100_000);
    r.agent.command("goal", "find the bug").unwrap();
    run(&r, "go").await.unwrap();
    let dir = r.agent.store().lock().unwrap().dir().to_path_buf();
    let before = project(&r.agent.store().lock().unwrap());

    let reopened = Store::open(&dir).unwrap();
    assert_eq!(project(&reopened), before);
    assert_eq!(SessionState::load(&reopened).manifest.goal.as_deref(), Some("find the bug"));
}

#[tokio::test]
async fn the_file_tools_read_write_and_edit_relative_to_the_working_directory() {
    let r = rig(
        vec![
            ScriptedLlm::call("", &[("1", "write", json!({"path": "notes/a.txt", "content": "alpha\nbeta\ngamma\n"}))]),
            ScriptedLlm::call("", &[("2", "edit", json!({"path": "notes/a.txt", "old_string": "beta", "new_string": "BETA"}))]),
            ScriptedLlm::call("", &[("3", "edit", json!({"path": "notes/a.txt", "old_string": "a", "new_string": "x"}))]),
            ScriptedLlm::call("", &[("4", "read", json!({"path": "notes/a.txt", "offset": 2, "limit": 1}))]),
            ScriptedLlm::call("", &[("5", "read", json!({"path": "nope.txt"}))]),
            ScriptedLlm::say("ok"),
        ],
        Sum::says("-"),
        100_000,
    );
    run(&r, "files").await.unwrap();
    let res = results(&r);
    assert!(res[0].starts_with("wrote 17 bytes to "));
    assert!(res[1].starts_with("edited "));
    assert!(res[2].contains("matches 4 times"), "{}", res[2]);
    assert_eq!(res[3], "BETA\n… [1 more line(s); continue with offset 3]");
    assert!(res[4].contains("nope.txt"));
    assert_eq!(std::fs::read_to_string(r.root.join("work/notes/a.txt")).unwrap(), "alpha\nBETA\ngamma\n");
}

// -- settings layers and session scope (ADR-0038) ----------------------------------------------------

#[tokio::test]
async fn the_budget_is_the_baseline_then_the_global_settings_then_the_sessions_own_layer() {
    use reactor_context::settings::ContextSettings;
    let r = rig(vec![], Sum::says("-"), 50_000);
    let base = r.agent.effective_budget();
    assert_eq!((base.window, base.reserve, base.mode), (50_000, 100, Mode::Auto), "the baseline");

    // Global: settings.json.
    std::fs::create_dir_all(r.root.join("cfg")).unwrap();
    std::fs::write(r.root.join("cfg/settings.json"), r#"{"context": {"mode": "compact", "reserve": 2000, "pct": 0.8}}"#).unwrap();
    let eff = r.agent.effective_budget();
    assert_eq!((eff.mode, eff.reserve, eff.pct, eff.window), (Mode::Compact, 2000, 0.8, 50_000));

    // Session: wins field by field, inherits the rest.
    r.agent
        .set_session_context(&ContextSettings { mode: Some("fade".into()), window: Some(8_000), ..Default::default() })
        .unwrap();
    let eff = r.agent.effective_budget();
    assert_eq!((eff.mode, eff.window, eff.reserve, eff.pct), (Mode::Fade, 8_000, 2000, 0.8));

    let (global, session) = r.agent.context_layers();
    assert_eq!(global.mode.as_deref(), Some("compact"));
    assert_eq!(session.mode.as_deref(), Some("fade"));
    assert_eq!(session.reserve, None);

    // Making it the default writes settings.json; the session layer is unchanged.
    r.agent.set_global_context(session.over(&global)).unwrap();
    assert_eq!(r.agent.context_layers().0.mode.as_deref(), Some("fade"));
}

#[tokio::test]
async fn a_session_override_changes_when_the_loop_reduces() {
    // Same conversation, same window: with the global reserve the context fits; with the
    // session's tighter window it must reduce. The setting takes effect on the next request.
    let replies = (0..12).map(|i| ScriptedLlm::say(&format!("reply {i}"))).collect();
    let r = rig(replies, Sum::says("SUMMARY"), 200_000);
    for i in 0..4 {
        run(&r, &format!("question {i}: {}", "w".repeat(3_000))).await.unwrap();
    }
    assert!(!kinds(&r).contains(&"reduction"), "a big window never reduces");

    r.agent.set_session_context(&reactor_context::settings::ContextSettings { window: Some(4_000), ..Default::default() }).unwrap();
    run(&r, "and one more").await.unwrap();
    assert!(kinds(&r).contains(&"reduction"), "the session's smaller window took effect at once");
}

#[tokio::test]
async fn switching_the_model_takes_effect_on_the_next_call() {
    use reactor_agent::llm::{Llm, Switchable};
    let a = Arc::new(ScriptedLlm::new(vec![ScriptedLlm::say("from a")]));
    let b = Arc::new(ScriptedLlm::new(vec![ScriptedLlm::say("from b")]));
    let sw = Switchable::new(a.clone());
    let req = || reactor_agent::llm::LlmRequest { system: String::new(), messages: vec![Msg::User { text: "hi".into() }], tools: vec![], max_tokens: None };

    assert_eq!(sw.complete(req(), &mut |_| {}).await.unwrap().blocks, vec![Block::Text { text: "from a".into() }]);
    sw.set(b.clone());
    assert_eq!(sw.complete(req(), &mut |_| {}).await.unwrap().blocks, vec![Block::Text { text: "from b".into() }]);
    assert_eq!((a.requests().len(), b.requests().len()), (1, 1));
}

const SCOPE_TOOLS: &str = r#"
version = 1
[tool.alpha]
name = "Alpha"
desc = "the alpha tool"
source = "https://example.invalid/a"
invoke = "sh"
detect = { binary = "sh" }
tags = ["one"]
[tool.beta]
name = "Beta"
desc = "the beta tool"
source = "https://example.invalid/b"
invoke = "ls"
detect = { binary = "ls" }
tags = ["two"]
"#;

const SCOPE_TOOLSETS: &str = "version = 1\n[toolset.only-alpha]\ndesc = \"alpha only\"\ntags = [\"one\"]\n[toolset.only-beta]\ndesc = \"beta only\"\ntags = [\"two\"]\n";

#[tokio::test]
async fn activation_in_one_session_changes_its_registry_block_and_no_other_sessions() {
    use reactor_core::commands::set_activation;
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("tools.toml"), SCOPE_TOOLS).unwrap();
    std::fs::write(cfg_dir.join("toolsets.toml"), SCOPE_TOOLSETS).unwrap();
    let machine = Paths::new(&cfg_dir, Shipped::Embedded);
    let session_a = machine.clone().with_session_state(dir.path().join("a-activation.json"));
    let session_b = machine.clone().with_session_state(dir.path().join("b-activation.json"));

    let make = |paths: Paths| {
        let dir2 = dir.path().join(format!("s-{}", paths.session_state.as_ref().unwrap().file_stem().unwrap().to_string_lossy()));
        let store = Store::create(&dir2, "s", dir.path()).unwrap();
        let mut cfg = AgentConfig::new(dir.path().to_path_buf(), paths.clone(), 100_000);
        cfg.registry = reactor_agent::agent::registry_from_core(paths);
        let llm = Arc::new(ScriptedLlm::new(vec![ScriptedLlm::say("1"), ScriptedLlm::say("2")]));
        let (agent, _rx) = Agent::new(llm.clone(), Sum::says("-"), store, Tools::new(), cfg);
        (agent, llm)
    };
    let (a, llm_a) = make(session_a.clone());
    let (b, llm_b) = make(session_b);

    a.prompt("go", CancellationToken::new()).await.unwrap();
    b.prompt("go", CancellationToken::new()).await.unwrap();
    for llm in [&llm_a, &llm_b] {
        let sys = &llm.requests()[0].system;
        assert!(sys.contains("the alpha tool") && sys.contains("the beta tool"), "everything is active by default:\n{sys}");
    }

    // Session A narrows to alpha. Only A's next request changes.
    set_activation(&session_a, &["only-alpha".to_string()], false, Some(true)).unwrap();
    a.prompt("again", CancellationToken::new()).await.unwrap();
    b.prompt("again", CancellationToken::new()).await.unwrap();
    let (sys_a, sys_b) = (&llm_a.requests()[1].system, &llm_b.requests()[1].system);
    assert!(sys_a.contains("the alpha tool") && !sys_a.contains("the beta tool"), "{sys_a}");
    assert!(sys_b.contains("the alpha tool") && sys_b.contains("the beta tool"), "{sys_b}");
    assert!(!cfg_dir.join("state.json").exists(), "the machine state was never written");
}

#[tokio::test]
async fn a_frontend_can_measure_list_and_undo_reductions() {
    let replies = (0..6).map(|i| ScriptedLlm::say(&format!("reply {i}"))).collect();
    let r = rig(replies, Sum::says("The summary."), 30_000);
    for i in 0..3 {
        run(&r, &format!("question {i} {}", "z".repeat(3_000))).await.unwrap();
    }
    let before = r.agent.measure().await.unwrap();
    assert!(before.total > before.fixed && before.messages > 2_000, "{before:?}");
    assert_eq!(before.calibration, 1.0);
    assert!(r.agent.reductions().is_empty());

    let id = r.agent.reduce_now(Mode::Compact).await.unwrap();
    let after = r.agent.measure().await.unwrap();
    assert!(after.messages < before.messages, "{after:?} vs {before:?}");
    let info = &r.agent.reductions()[0];
    assert_eq!((info.entry, info.mode, info.summary.as_deref()), (id, Mode::Compact, Some("The summary.")));
    assert!(info.covers >= 2 && info.after_tokens < info.before_tokens);

    r.agent.restore(id).unwrap();
    assert!(r.agent.reductions().is_empty());
    assert_eq!(r.agent.measure().await.unwrap().messages, before.messages, "undo brings the originals back exactly");
    assert!(r.agent.restore(id).is_err(), "and it is only undoable while in force");
}

#[tokio::test]
async fn an_unconfigured_model_fails_each_call_clearly_instead_of_blocking_the_session() {
    use reactor_agent::llm::Llm;
    let llm = reactor_agent::provider::AnyLlm::Missing("no model selected -- pick one".into());
    let e = llm.complete(reactor_agent::llm::LlmRequest { system: String::new(), messages: vec![], tools: vec![], max_tokens: None }, &mut |_| {}).await.unwrap_err();
    assert!(e.to_string().contains("pick one"));
    assert_eq!(llm.name(), "none");
}

#[tokio::test]
async fn the_context_preview_takes_the_request_apart_and_labels_every_piece() {
    use reactor_agent::inspect::{Origin, Section};
    let r = rig(vec![ScriptedLlm::say("noted")], Sum::says("."), 30_000);
    r.agent.command("goal", "find the loader").unwrap();
    run(&r, "hello there").await.unwrap();

    let p = r.agent.context_preview().await.unwrap();
    let origins: Vec<Origin> = p.segments.iter().map(|s| s.origin).collect();
    assert!(origins.contains(&Origin::Base) && origins.contains(&Origin::Manifest), "{origins:?}");
    assert!(origins.contains(&Origin::ToolDefinition));
    let goal = p.segments.iter().find(|s| s.origin == Origin::Manifest).unwrap();
    assert!(goal.text.contains("find the loader") && goal.section == Section::System);
    let you = p.segments.iter().find(|s| s.origin == Origin::User).unwrap();
    assert_eq!((you.text.as_str(), you.section, you.entry.is_some()), ("hello there", Section::Messages, true));
    assert!(p.segments.iter().any(|s| s.origin == Origin::Assistant && s.text == "noted"));

    // What is listed is what is measured: the same total the budget works from.
    let m = r.agent.measure().await.unwrap();
    assert_eq!(p.total_tokens, m.total, "the preview and the budget count the same request");
}

fn thinking_only() -> Result<Reply> {
    Ok(Reply { blocks: vec![Block::Thinking { text: "hmm, where to begin".into(), signature: None }], usage: None, stop: Some("length".into()) })
}

fn summary_request() -> SummaryRequest {
    SummaryRequest { system: "You compress.".into(), transcript: "#1 user: hi".into(), max_tokens: 1_500 }
}

#[tokio::test]
async fn a_reasoning_model_that_runs_out_of_room_thinking_is_given_more_and_the_notes_arrive() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = Arc::new(ScriptedLlm::new([thinking_only(), ScriptedLlm::say("The notes.")]));
    let notes = LlmSummarizer { llm: llm.clone() }.summarize(summary_request()).await.unwrap();
    assert_eq!(notes, "The notes.");
    let seen = llm.requests();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].max_tokens.unwrap() >= 4096, "room for thinking as well as the notes");
    assert!(seen[1].max_tokens > seen[0].max_tokens, "and more on the second try");
    assert!(seen[0].system.contains("under about 1125 words"), "{}", seen[0].system);
}

#[tokio::test]
async fn a_summarizer_that_never_writes_notes_fails_saying_why() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = Arc::new(ScriptedLlm::new([thinking_only(), thinking_only()]));
    let e = LlmSummarizer { llm }.summarize(summary_request()).await.unwrap_err().to_string();
    assert!(e.contains("only thinking") && e.contains("stop: length") && e.contains("Settings"), "{e}");

    let empty = Arc::new(ScriptedLlm::new([Ok(Reply { blocks: vec![], usage: None, stop: Some("stop".into()) })]));
    let e = LlmSummarizer { llm: empty }.summarize(summary_request()).await.unwrap_err().to_string();
    assert!(e.contains("nothing at all"), "{e}");
}
