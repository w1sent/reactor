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
    /// Never answers.
    hang: bool,
}

impl Sum {
    fn says(t: &str) -> Arc<Self> {
        Arc::new(Sum {
            answer: Ok(t.into()),
            seen: Mutex::new(vec![]),
            hang: false,
        })
    }
    fn fails(t: &str) -> Arc<Self> {
        Arc::new(Sum {
            answer: Err(t.into()),
            seen: Mutex::new(vec![]),
            hang: false,
        })
    }
    fn hangs() -> Arc<Self> {
        Arc::new(Sum {
            answer: Ok(String::new()),
            seen: Mutex::new(vec![]),
            hang: true,
        })
    }
}

impl Summarizer for Sum {
    async fn summarize(&self, req: SummaryRequest) -> Result<String> {
        self.seen.lock().unwrap().push(req);
        if self.hang {
            std::future::pending::<()>().await;
        }
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
    let view = RegistryView {
        block: block.to_string(),
        skill_dirs: vec![],
        usable: usable.iter().map(|s| s.to_string()).collect::<HashSet<_>>(),
        ..Default::default()
    };
    Arc::new(move || view.clone())
}

fn rig(replies: Vec<Result<Reply>>, sum: Arc<Sum>, window: u64) -> Rig {
    rig_with(replies, sum, window, |_| {})
}

fn rig_with(
    replies: Vec<Result<Reply>>,
    sum: Arc<Sum>,
    window: u64,
    tweak: impl FnOnce(&mut AgentConfig),
) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let case = reactor_db::Case::create(&cwd, None).unwrap();
    let store = Store::create(&case, "s").unwrap();
    let paths = Paths::new(root.join("cfg"), Shipped::Embedded);
    let mut cfg = AgentConfig::new(cwd.clone(), paths, window);
    cfg.registry = registry("## Available tools (this machine)\n\nsh   shell", &["sh"]);
    cfg.scenarios_dir = root.join("scenarios");
    cfg.budget.reserve = 100;
    tweak(&mut cfg);
    let llm = Arc::new(ScriptedLlm::new(replies));
    let (agent, events) = Agent::new(llm.clone(), sum.clone(), store, Tools::standard(cwd), cfg);
    Rig {
        _dir: dir,
        root,
        agent,
        llm,
        sum,
        events,
    }
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
    rig.agent
        .store()
        .lock()
        .unwrap()
        .all()
        .iter()
        .map(|e| e.kind.label())
        .collect()
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
    let mut r = rig(
        vec![ScriptedLlm::say("hello there")],
        Sum::says("-"),
        100_000,
    );
    let out = run(&r, "hi").await.unwrap();
    assert_eq!((out.rounds, out.tool_calls, out.reductions), (1, 0, 0));
    assert_eq!(kinds(&r), ["session", "user", "assistant"]);

    let events = drain(&mut r.events);
    assert!(events.contains(&Event::Text("hello there".into())));
    assert_eq!(events.last(), Some(&Event::Finished));

    // The request carried the tools and a system prompt with the registry block.
    let req = &r.llm.requests()[0];
    assert!(req.system.contains("You are REactor"));
    assert!(req.system.contains("## Available tools (this machine)"));
    assert_eq!(req.messages, [Msg::User { text: "hi".into() }]);
    let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "bash",
            "read",
            "write",
            "edit",
            "history_index",
            "history_read",
            "history_search",
            "case_get",
            "scenario_list",
            "scenario_save",
            "tool_search"
        ],
        "the state-gated tools are not advertised, and the graph's six wait for tool_search"
    );
    let search = req.tools.last().unwrap();
    for name in [
        "graph_write",
        "graph_query",
        "graph_pull",
        "graph_revert",
        "define_type",
        "artifact_register",
    ] {
        assert!(search.description.contains(name), "{name} is not listed");
    }
    assert!(req.system.contains("select:graph_write"));
}

#[tokio::test]
async fn the_system_prompt_is_byte_identical_across_requests_when_nothing_changed() {
    // The property the prompt cache depends on (ADR-0006).
    let r = rig(
        vec![bash_call("c1", "true"), ScriptedLlm::say("done")],
        Sum::says("-"),
        100_000,
    );
    run(&r, "go").await.unwrap();
    let reqs = r.llm.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].system, reqs[1].system);
    assert_eq!(reqs[0].tools, reqs[1].tools);
}

#[tokio::test]
async fn a_model_failure_leaves_the_users_message_and_no_half_reply() {
    let r = rig(
        vec![Err(Error::Model("503 overloaded".into()))],
        Sum::says("-"),
        100_000,
    );
    let err = run(&r, "hi").await.unwrap_err();
    assert!(err.to_string().contains("503"));
    assert_eq!(kinds(&r), ["session", "user"]);
}

// -- tools ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_tool_round_trip_records_call_and_result_and_sends_the_result_back() {
    let mut r = rig(
        vec![
            bash_call("c1", "echo hello"),
            ScriptedLlm::say("it printed hello"),
        ],
        Sum::says("-"),
        100_000,
    );
    let out = run(&r, "run it").await.unwrap();
    assert_eq!((out.rounds, out.tool_calls), (2, 1));
    assert_eq!(
        kinds(&r),
        ["session", "user", "assistant", "tool_result", "assistant"]
    );
    assert_eq!(results(&r), ["hello"]);

    let second = &r.llm.requests()[1];
    assert!(
        matches!(&second.messages[1], Msg::Assistant { blocks } if matches!(&blocks[0], Block::ToolCall { name, .. } if name == "bash"))
    );
    assert_eq!(
        second.messages[2],
        Msg::ToolResult {
            call_id: "c1".into(),
            name: "bash".into(),
            content: "hello".into(),
            is_error: false
        }
    );

    let events = drain(&mut r.events);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::ToolStart { name, .. } if name == "bash"))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::ToolOutput { chunk, .. } if chunk == "hello\n"))
    );
    assert!(events.iter().any(|e| matches!(
        e,
        Event::ToolEnd {
            is_error: false,
            ..
        }
    )));
}

#[tokio::test]
async fn several_calls_in_one_reply_all_run_and_all_get_results_in_order() {
    let r = rig(
        vec![
            ScriptedLlm::call(
                "",
                &[
                    ("a", "bash", json!({"command": "echo one"})),
                    ("b", "bash", json!({"command": "echo two"})),
                ],
            ),
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
    let r = rig(
        vec![
            ScriptedLlm::call("", &[("c", "teleport", json!({}))]),
            ScriptedLlm::say("sorry"),
        ],
        Sum::says("-"),
        100_000,
    );
    run(&r, "go").await.unwrap();
    assert_eq!(results(&r), ["unknown tool `teleport`"]);
    let store = r.agent.store();
    let store = store.lock().unwrap();
    assert!(matches!(
        &store.all()[3].kind,
        Kind::ToolResult { is_error: true, .. }
    ));
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
            ScriptedLlm::call(
                "",
                &[(
                    "1",
                    "bash",
                    json!({"command": "export A=one", "session": "s1"}),
                )],
            ),
            ScriptedLlm::call(
                "",
                &[(
                    "2",
                    "bash",
                    json!({"command": "echo [$A]", "session": "s2"}),
                )],
            ),
            ScriptedLlm::call(
                "",
                &[(
                    "3",
                    "bash",
                    json!({"command": "echo [$A]", "session": "s1"}),
                )],
            ),
            ScriptedLlm::call(
                "",
                &[(
                    "4",
                    "bash",
                    json!({"command": "echo [$A]", "session": "s1", "restart": true}),
                )],
            ),
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
    let r = rig(
        vec![
            bash_call("1", "echo out; echo err >&2; (exit 3)"),
            bash_call("2", "echo still alive"),
            ScriptedLlm::say("ok"),
        ],
        Sum::says("-"),
        100_000,
    );
    run(&r, "go").await.unwrap();
    let res = results(&r);
    assert!(
        res[0].contains("out") && res[0].contains("err"),
        "{}",
        res[0]
    );
    assert!(res[0].ends_with("[exit code 3]"), "{}", res[0]);
    assert_eq!(res[1], "still alive");
}

#[tokio::test]
async fn exit_inside_a_command_ends_the_shell_and_the_next_call_gets_a_fresh_one() {
    let r = rig(
        vec![
            bash_call("1", "export GONE=1; exit 7"),
            bash_call("2", "echo [$GONE]"),
            ScriptedLlm::say("ok"),
        ],
        Sum::says("-"),
        100_000,
    );
    run(&r, "go").await.unwrap();
    let res = results(&r);
    assert!(res[0].contains("the shell exited (code 7)"), "{}", res[0]);
    assert_eq!(res[1], "[]");
}

#[tokio::test]
async fn a_command_that_reads_stdin_cannot_swallow_the_rest_of_the_script() {
    let r = rig(
        vec![bash_call("1", "cat; echo after"), ScriptedLlm::say("ok")],
        Sum::says("-"),
        100_000,
    );
    run(&r, "go").await.unwrap();
    assert_eq!(results(&r), ["after"]);
}

#[tokio::test]
async fn a_timeout_kills_the_shell_and_says_what_was_lost() {
    let r = rig(
        vec![
            ScriptedLlm::call(
                "",
                &[(
                    "1",
                    "bash",
                    json!({"command": "export KEEP=1; sleep 30", "timeout_secs": 1}),
                )],
            ),
            bash_call("2", "echo [$KEEP]"),
            ScriptedLlm::say("ok"),
        ],
        Sum::says("-"),
        100_000,
    );
    let started = std::time::Instant::now();
    run(&r, "go").await.unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "the timeout was not honoured"
    );
    let res = results(&r);
    assert!(
        res[0].contains("timed out after 1s") && res[0].contains("was reset"),
        "{}",
        res[0]
    );
    assert_eq!(
        res[1], "[]",
        "the killed session's variables are gone; a fresh one answers"
    );
    let store = r.agent.store();
    assert!(matches!(
        &store.lock().unwrap().all()[3].kind,
        Kind::ToolResult { is_error: true, .. }
    ));
}

// -- truncation and history -----------------------------------------------------------------------

#[tokio::test]
async fn long_output_is_cut_to_head_and_tail_and_kept_whole_and_addressable() {
    let r = rig(
        vec![
            bash_call(
                "1",
                "for i in $(seq 1 6000); do echo \"line $i of the dump\"; done",
            ),
            ScriptedLlm::call(
                "",
                &[(
                    "2",
                    "history_read",
                    json!({"id": 3, "offset": 3000, "limit": 2}),
                )],
            ),
            ScriptedLlm::call(
                "",
                &[("3", "history_search", json!({"pattern": "line 4242 "}))],
            ),
            ScriptedLlm::say("found it"),
        ],
        Sum::says("-"),
        1_000_000,
    );
    run(&r, "dump").await.unwrap();
    let res = results(&r);

    // What the model saw: both ends, a marker naming the entry.
    assert!(
        res[0].starts_with("line 1 of the dump"),
        "{}",
        &res[0][..40]
    );
    assert!(res[0].trim_end().ends_with("line 6000 of the dump"));
    assert!(
        res[0].contains("output truncated; the rest: history_read {\"id\": 3, "),
        "{}",
        res[0]
            .lines()
            .find(|l| l.contains("truncated"))
            .unwrap_or("")
    );
    assert!(res[0].len() < 33 * 1024, "{}", res[0].len());

    // Nothing was lost: the blob has every line, history_read and history_search reach them.
    assert_eq!(
        res[1],
        "line 3000 of the dump\nline 3001 of the dump\n… [2999 more line(s); continue with offset 3002]"
    );
    assert!(
        res[2].contains("#3:4242 (tool_result) line 4242 of the dump"),
        "{}",
        res[2]
    );
    let store = r.agent.store();
    let store = store.lock().unwrap();
    let Kind::ToolResult { blob: Some(b), .. } = &store.get(3).unwrap().kind else {
        panic!("no blob")
    };
    assert!(
        String::from_utf8(store.read_blob(b).unwrap())
            .unwrap()
            .lines()
            .count()
            == 6000
    );
}

#[tokio::test]
async fn a_call_sets_its_own_limit_and_the_marker_says_how_to_read_the_rest() {
    let dump = "for i in $(seq 1 6000); do echo \"line $i of the dump\"; done";
    let r = rig(
        vec![
            // Asks for everything: 6000 lines of ~20 bytes is under 200 KB.
            ScriptedLlm::call(
                "",
                &[(
                    "1",
                    "bash",
                    json!({ "command": dump, "max_output_bytes": 200_000 }),
                )],
            ),
            // Asks for less than the default.
            ScriptedLlm::call(
                "",
                &[(
                    "2",
                    "bash",
                    json!({ "command": dump, "max_output_bytes": 2048 }),
                )],
            ),
            ScriptedLlm::say("ok"),
        ],
        Sum::says("-"),
        1_000_000,
    );
    run(&r, "dump").await.unwrap();
    let res = results(&r);
    assert_eq!(res[0].lines().count(), 6000, "not cut");
    assert!(!res[0].contains("truncated"));

    assert!(res[1].len() < 2048 + 512, "{}", res[1].len());
    let marker = res[1].lines().find(|l| l.contains("truncated")).unwrap();
    assert!(
        marker.contains("history_read {\"id\": 5, \"offset\": "),
        "{marker}"
    );
}

#[tokio::test]
async fn the_tools_that_can_print_much_advertise_the_limit_and_only_they() {
    let state = reactor_agent::context::SessionState::default();
    let mut with: Vec<String> = reactor_agent::tools::Tools::standard(".".into())
        .specs(&state)
        .into_iter()
        .filter(|s| s.parameters["properties"]["max_output_bytes"].is_object())
        .map(|s| s.name)
        .collect();
    with.sort();
    assert_eq!(
        with,
        // `docs_search` too, but it is offered only while the case has documents.
        [
            "bash",
            "case_get",
            "graph_query",
            "history_index",
            "history_read",
            "history_search",
            "read"
        ]
    );
}

#[tokio::test]
async fn a_huge_output_spills_to_disk_and_keeps_every_byte() {
    // Well past what is held in memory.
    let r = rig(
        vec![
            bash_call("1", "yes 0123456789abcdef | head -c 6000000"),
            ScriptedLlm::say("ok"),
        ],
        Sum::says("-"),
        10_000_000,
    );
    run(&r, "big").await.unwrap();
    let store = r.agent.store();
    let store = store.lock().unwrap();
    let Kind::ToolResult {
        content,
        blob: Some(b),
        ..
    } = &store.get(3).unwrap().kind
    else {
        panic!("no blob")
    };
    assert!(b.bytes >= 6_000_000 - 20, "{}", b.bytes);
    assert_eq!(
        store.read_blob(b).unwrap().len() as u64,
        b.bytes,
        "the whole thing is on disk"
    );
    assert!(content.len() < 33 * 1024, "and the model's view is bounded");
    assert!(content.contains("output truncated"));
}

// -- state-gated tools and the prompt ---------------------------------------------------------------

#[tokio::test]
async fn update_steps_is_advertised_only_while_a_goal_is_set_and_the_block_follows_state() {
    let r = rig(
        vec![
            ScriptedLlm::say("first"),
            ScriptedLlm::call(
                "",
                &[(
                    "u",
                    "update_steps",
                    json!({"steps": [{"summary": "find the check", "status": "in progress"}]}),
                )],
            ),
            ScriptedLlm::say("second"),
        ],
        Sum::says("-"),
        100_000,
    );
    run(&r, "one").await.unwrap();
    assert!(
        !r.llm.requests()[0]
            .tools
            .iter()
            .any(|t| t.name == "update_steps")
    );
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
    assert!(
        last.system.contains("1. [in progress] find the check"),
        "{}",
        last.system
    );
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
    std::fs::write(
        d.join("01.md"),
        "---\ntitle: Scoping\n---\nDecide what is in scope.\n",
    )
    .unwrap();
    std::fs::write(
        d.join("02.md"),
        "---\ntitle: Triage\n---\nRun file, strings, hashes.\n",
    )
    .unwrap();
}

#[tokio::test]
async fn a_scenarios_phase_is_a_block_rebuilt_every_request_and_advances_by_tool_result() {
    let r = rig(
        vec![
            ScriptedLlm::say("starting"),
            ScriptedLlm::call(
                "",
                &[(
                    "p",
                    "reactor_phase_complete",
                    json!({"summary": "scope: one binary"}),
                )],
            ),
            ScriptedLlm::say("triaging"),
            ScriptedLlm::call(
                "",
                &[(
                    "q",
                    "reactor_phase_complete",
                    json!({"summary": "it is packed"}),
                )],
            ),
            ScriptedLlm::say("all done"),
        ],
        Sum::says("-"),
        100_000,
    );
    write_scenario(&r.root);
    let started = r.agent.command("reactor-scenario", "start inv").unwrap();
    assert_eq!(
        started.trigger_turn.as_deref(),
        Some("## Phase 1/2: Scoping\n\nDecide what is in scope.")
    );
    run(&r, "begin").await.unwrap();
    run(&r, "next please").await.unwrap();
    run(&r, "and the last").await.unwrap();

    let reqs = r.llm.requests();
    assert!(reqs[0].system.contains("## Phase 1/2: Scoping"));
    assert!(
        reqs[0]
            .tools
            .iter()
            .any(|t| t.name == "reactor_phase_complete")
    );
    assert!(
        reqs[2].system.contains("## Phase 2/2: Triage")
            && reqs[2].system.contains("1. Scoping -- scope: one binary"),
        "{}",
        reqs[2].system
    );
    assert!(!reqs[2].system.contains("Phase 1/2"));
    // Finished: the block and the tool are gone.
    let last = reqs.last().unwrap();
    assert!(!last.system.contains("Scenario:"));
    assert!(
        !last
            .tools
            .iter()
            .any(|t| t.name == "reactor_phase_complete")
    );
    assert!(results(&r)[1].contains("scenario \"inv\" complete"));
    assert!(r.agent.session_state().scenario.is_none());
}

#[tokio::test]
async fn a_phase_chosen_by_hand_waits_for_the_next_prompt_and_that_prompt_is_the_summary() {
    let r = rig(
        vec![ScriptedLlm::say("one"), ScriptedLlm::say("two")],
        Sum::says("-"),
        100_000,
    );
    write_scenario(&r.root);
    r.agent.command("reactor-scenario", "start inv").unwrap();
    run(&r, "begin").await.unwrap();

    // Jumping on starts no turn, and says what to do.
    let moved = r.agent.command("reactor-scenario", "goto 2").unwrap();
    assert!(moved.trigger_turn.is_none());
    assert!(
        moved.notices[0].message.contains("next prompt"),
        "{:?}",
        moved.notices
    );
    let state = r.agent.session_state().scenario.unwrap();
    assert_eq!((state.step_index, state.pending_summary), (1, Some(0)));

    // The next prompt is the summary of the phase that was left, and the new phase's
    // briefing is already in the system prompt it is sent with.
    run(&r, "scope is one binary,\n nothing else")
        .await
        .unwrap();
    let sys = &r.llm.requests()[1].system;
    assert!(sys.contains("## Phase 2/2: Triage"), "{sys}");
    assert!(
        sys.contains("1. Scoping -- scope is one binary, nothing else"),
        "{sys}"
    );
    let state = r.agent.session_state().scenario.unwrap();
    assert_eq!(state.pending_summary, None);

    // Going back drops that summary; and the prompt after it is just a prompt.
    r.agent.command("reactor-scenario", "goto 1").unwrap();
    let state = r.agent.session_state().scenario.unwrap();
    assert_eq!((state.step_index, state.summaries.len()), (0, 0));
}

// -- scenarios: a question, phases left out, written for the case (ADR-0065) ----------------

#[tokio::test]
async fn a_scenario_started_with_a_question_sets_the_goal_and_its_summaries_outlive_it() {
    let r = rig_with(
        vec![
            ScriptedLlm::call(
                "",
                &[("p", "reactor_phase_complete", json!({"summary": "the VPN box was the way in"}))],
            ),
            ScriptedLlm::say("done"),
            ScriptedLlm::say("again"),
        ],
        Sum::says("-"),
        100_000,
        no_deferral,
    );
    write_scenario(&r.root);
    // Scoping is left out: it starts on Triage, and the question is the goal.
    let started = r
        .agent
        .command("reactor-scenario", "start inv skip=1 Was the VPN appliance\nthe entry point?")
        .unwrap();
    assert!(started.trigger_turn.unwrap().starts_with("## Phase 2/2: Triage"));
    assert_eq!(
        r.agent.session_state().manifest.goal.as_deref(),
        Some("Was the VPN appliance\nthe entry point?")
    );
    run(&r, "begin").await.unwrap();
    assert!(r.agent.session_state().scenario.is_none());

    // The next scenario reads what this one concluded, from the graph.
    let again = r.agent.command("reactor-scenario", "start inv").unwrap();
    let briefing = again.trigger_turn.unwrap();
    assert!(
        briefing.contains("What earlier scenarios in this case concluded")
            && briefing.contains("inv 2. Triage: the VPN box was the way in"),
        "{briefing}"
    );
}

#[tokio::test]
async fn the_agent_writes_a_scenario_for_the_case_and_amends_only_what_is_ahead() {
    let phase = |t: &str| json!({"title": t, "body": format!("Do {t}.")});
    let r = rig_with(
        vec![
            ScriptedLlm::call(
                "",
                &[(
                    "s",
                    "scenario_save",
                    json!({"id": "inv", "description": "This case's plan", "phases": [phase("Hosts"), phase("Logs")]}),
                )],
            ),
            ScriptedLlm::say("saved"),
            ScriptedLlm::call(
                "",
                &[("a", "scenario_amend", json!({"phases": [phase("Mail"), phase("Report")]}))],
            ),
            ScriptedLlm::say("amended"),
        ],
        Sum::says("-"),
        100_000,
        no_deferral,
    );
    // A global `inv` exists; the case's own, saved by the agent, wins over it.
    write_scenario(&r.root);
    run(&r, "write the plan down").await.unwrap();
    assert!(results(&r)[0].contains("saved scenario \"inv\" (case, 2 phases)"), "{:?}", results(&r));
    let lib = r.agent.scenarios();
    assert_eq!(lib.find("inv").unwrap().0, reactor_core::scenarios::Scope::Case);
    assert_eq!(lib.load("inv")[0].description.as_deref(), Some("This case's plan"));

    r.agent.command("reactor-scenario", "start inv").unwrap();
    run(&r, "change what is left").await.unwrap();
    let titles: Vec<String> = r.agent.scenarios().load("inv").into_iter().map(|p| p.title).collect();
    assert_eq!(titles, ["Hosts", "Mail", "Report"]);
    // The global one is untouched.
    assert_eq!(
        reactor_core::scenarios::load_dir(&r.root.join("scenarios/inv"))[0].title,
        "Scoping"
    );
}

#[tokio::test]
async fn the_scenario_editing_switch_withdraws_the_authoring_tools_and_refuses_a_stale_call() {
    let off = |cfg: &mut AgentConfig| {
        std::fs::create_dir_all(&cfg.paths.config_dir).unwrap();
        std::fs::write(
            cfg.paths.config_dir.join("settings.json"),
            r#"{"toolSearch":{"deferOverTokens":100000},"memory":{"scenarioEditing":false}}"#,
        )
        .unwrap();
    };
    let r = rig_with(
        vec![
            ScriptedLlm::call("", &[("l", "scenario_list", json!({}))]),
            ScriptedLlm::say("ok"),
        ],
        Sum::says("-"),
        100_000,
        off,
    );
    write_scenario(&r.root);
    run(&r, "list them").await.unwrap();
    let reqs = r.llm.requests();
    let names = tool_names(&reqs[0]);
    assert!(!names.contains(&"scenario_list") && !names.contains(&"scenario_save"));
    assert!(results(&r)[0].contains("scenario-editing switch is off"), "{:?}", results(&r));
    // Running one is not editing: the phase tool is still there.
    r.agent.command("reactor-scenario", "start inv").unwrap();
    assert!(r.agent.session_state().scenario.is_some());
}

#[tokio::test]
async fn promoting_asks_the_agent_to_rewrite_a_case_scenario_for_every_case() {
    let r = rig(vec![], Sum::says("-"), 100_000);
    write_scenario(&r.root);
    let refused = r.agent.command("reactor-scenario", "promote inv").unwrap();
    assert!(refused.trigger_turn.is_none(), "a global one is not promoted");
    let case_dir = r.agent.store().lock().unwrap().case().dir().to_path_buf();
    let p = reactor_core::scenarios::Phase { title: "A".into(), body: "b".into(), ..Default::default() };
    reactor_core::scenarios::save(&case_dir.join("scenarios"), "mine", None, &[p]).unwrap();
    let asked = r.agent.command("reactor-scenario", "promote mine").unwrap();
    assert!(asked.trigger_turn.unwrap().contains("scope global, id `mine`"));
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
    std::fs::write(
        upstream.join("SKILL.md"),
        "---\nname: binja-cli\ndescription: drive Binary Ninja\n---\n",
    )
    .unwrap();
    let up = upstream.display().to_string();

    let r = rig_with(
        vec![ScriptedLlm::say("ok")],
        Sum::says("-"),
        100_000,
        |cfg| {
            cfg.skills_dir = Some(skills);
            let view = RegistryView {
                block: "## Available tools (this machine)".into(),
                skill_dirs: vec![up],
                usable: ["sh".to_string()].into(),
                ..Default::default()
            };
            cfg.registry = Arc::new(move || view.clone());
        },
    );
    run(&r, "go").await.unwrap();
    let sys = &r.llm.requests()[0].system;
    assert!(
        sys.contains("**uses-sh**") && sys.contains("**free**") && sys.contains("**binja-cli**"),
        "{sys}"
    );
    assert!(
        !sys.contains("needs-gdb"),
        "a skill for a tool that is not here is not offered"
    );
}

// -- the budget -----------------------------------------------------------------------------------

/// A window small enough that one dump forces a reduction.
const SMALL: u64 = 5_000;

/// Advertise every tool, whatever a group costs (ADR-0055).
fn no_deferral(cfg: &mut AgentConfig) {
    std::fs::create_dir_all(&cfg.paths.config_dir).unwrap();
    std::fs::write(
        cfg.paths.config_dir.join("settings.json"),
        r#"{"toolSearch":{"deferOverTokens":100000}}"#,
    )
    .unwrap();
}

/// Detach the session from the case's graph: its tools and its block alone are most of
/// a window this small, and these tests are about the budget, not the graph.
fn detached(cfg: &mut AgentConfig) {
    std::fs::create_dir_all(&cfg.paths.config_dir).unwrap();
    std::fs::write(
        cfg.paths.config_dir.join("settings.json"),
        // The output limit is pinned too: these tests fill a small window with big dumps, and
        // the limit that follows a small window would cut them down before they could.
        r#"{"memory":{"caseGraph":false},"toolOutput":{"maxInlineBytes":32768}}"#,
    )
    .unwrap();
}

#[tokio::test]
async fn a_dump_that_overflows_inside_a_turn_is_reduced_at_the_tool_result_boundary() {
    // Older work, then a turn whose tool result pushes past the trigger. The reduction
    // lands *before* the next request, with no continue message and no parked turn.
    let mut r = rig_with(
        vec![
            bash_call(
                "1",
                "for i in $(seq 1 150); do echo \"padding line number $i to fill the context\"; done",
            ),
            ScriptedLlm::say("noted"),
            bash_call(
                "2",
                "for i in $(seq 1 150); do echo \"more padding line number $i to fill the context\"; done",
            ),
            ScriptedLlm::say("finished"),
        ],
        Sum::says("EARLIER: looked at two dumps; nothing found"),
        SMALL,
        // `compact` asks for summaries; in `auto` the dumps would just be masked.
        |c| {
            detached(c);
            c.budget.mode = Mode::Compact;
        },
    );
    run(&r, "first, an older question about the sample")
        .await
        .unwrap();
    let out = run(&r, "now dump twice and tell me").await.unwrap();

    assert!(out.reductions >= 1, "{out:?}");
    let events = drain(&mut r.events);
    let reduced = events
        .iter()
        .position(|e| matches!(e, Event::Reduced { .. }))
        .expect("a reduction happened");
    let last_req = r.llm.requests().last().unwrap().clone();
    assert!(
        last_req.messages.iter().any(
            |m| matches!(m, Msg::User { text } if text.contains("EARLIER: looked at two dumps"))
        ),
        "the stand-in went into the next request"
    );
    // The originals are all still there.
    let store = r.agent.store();
    let store = store.lock().unwrap();
    assert!(
        store
            .all()
            .iter()
            .any(|e| matches!(&e.kind, Kind::User { text } if text.contains("older question")))
    );
    assert!(
        store
            .all()
            .iter()
            .any(|e| matches!(e.kind, Kind::Reduction(_)))
    );
    // No message anywhere is a synthetic "continue".
    assert!(
        !store
            .all()
            .iter()
            .any(|e| matches!(&e.kind, Kind::User { text } if text.to_lowercase() == "continue"))
    );
    assert!(reduced > 0);
    // And the summarizer saw the tool output it summarized.
    assert!(
        r.sum.seen.lock().unwrap()[0]
            .transcript
            .contains("padding line number")
    );
}

#[tokio::test]
async fn a_summarizer_that_fails_is_replaced_by_a_fade_and_the_turn_goes_on() {
    // ADR-0058: a context that did not shrink must not be continued on -- and a fade, which
    // needs no model, shrinks it. So the summarizer's failure no longer ends the turn.
    let mut r = rig_with(
        vec![
            ScriptedLlm::say("noted"),
            ScriptedLlm::call(
                "",
                &[
                    (
                        "1",
                        "bash",
                        json!({"command": "for i in $(seq 1 300); do echo \"padding line number $i to fill the context up\"; done"}),
                    ),
                    ("2", "bash", json!({"command": "echo second call runs"})),
                ],
            ),
            ScriptedLlm::say("finished"),
        ],
        Sum::fails("429 rate limited"),
        SMALL,
        |c| {
            detached(c);
            c.budget.mode = Mode::Compact;
        },
    );
    run(&r, "an older question, so there is something to reduce")
        .await
        .unwrap();
    run(&r, "now dump").await.unwrap();

    let events = drain(&mut r.events);
    assert!(
        notices(&events)
            .iter()
            .any(|n| n.contains("summarizing failed") && n.contains("429 rate limited")),
        "{:?}",
        notices(&events)
    );
    // Both calls of the reply have results, and the second really ran.
    let res = results(&r);
    assert_eq!(res.len(), 2);
    assert!(res[1].contains("second call runs"), "{res:?}");
    // What was recorded is a fade: it needs no model, and it kept what the person said.
    let store = r.agent.store();
    let store = store.lock().unwrap();
    let fade = store
        .all()
        .iter()
        .find_map(|e| match &e.kind {
            Kind::Reduction(red) if red.mode == Mode::Fade && !red.stubs.is_empty() => Some(red),
            _ => None,
        })
        .expect("a fade was recorded");
    assert!(
        fade.stubs.iter().any(|s| s
            .text
            .as_deref()
            .is_some_and(|t| t.contains("an older question, so there is something to reduce"))),
        "the person's words are kept as they were said"
    );
}

#[tokio::test]
async fn a_summary_bigger_than_what_it_covers_is_refused_and_a_fade_takes_its_place() {
    // A summarizer whose "summary" is bigger than the window could never help. It is
    // refused (it frees nothing), the range is faded instead, and the turn goes on: no
    // reduction loop, and no request that will overflow.
    let bloat = "x".repeat(24_000);
    let replies = (0..12)
        .map(|i| ScriptedLlm::say(&format!("reply {i}")))
        .collect();
    let r = rig_with(replies, Sum::says(&bloat), SMALL, |c| {
        c.budget.mode = Mode::Compact;
    });
    for i in 0..8 {
        run(&r, &format!("question {i}: {}", "y".repeat(4_000)))
            .await
            .unwrap_or_else(|e| panic!("turn {i}: {e}"));
    }
    let window = r.agent.effective_budget().window;
    let store = r.agent.store();
    let store = store.lock().unwrap();
    let reductions: Vec<_> = store
        .all()
        .iter()
        .filter_map(|e| match &e.kind {
            Kind::Reduction(red) => Some(red.clone()),
            _ => None,
        })
        .collect();
    assert!(!reductions.is_empty());
    assert!(
        reductions
            .iter()
            .all(|red| red.mode == Mode::Fade && red.summary.is_none()),
        "no bloated summary was ever kept"
    );
    // Every request that was sent fits the window it was sent to.
    for q in r.llm.requests() {
        let size: usize = q
            .messages
            .iter()
            .map(|m| match m {
                Msg::User { text } => text.len(),
                _ => 0,
            })
            .sum::<usize>()
            + q.system.len();
        assert!(size as u64 / 4 < window, "{} tokens of {window}", size / 4);
    }
}

#[tokio::test]
async fn a_context_with_nothing_big_enough_to_cut_down_stops_the_turn_saying_so() {
    // The one thing left is the person's own message, bigger than the window: no reduction
    // helps, and the turn stops instead of sending a request that will overflow.
    let r = rig(vec![ScriptedLlm::say("never")], Sum::says("-"), SMALL);
    let e = run(&r, &"z".repeat(40_000)).await.unwrap_err();
    assert!(matches!(e, Error::Reduction(_)), "{e}");
    assert!(
        e.to_string().contains("the context is full")
            && e.to_string()
                .contains("nothing in it is big enough to cut down"),
        "{e}"
    );
    assert!(r.llm.requests().is_empty(), "no request was made");
}
#[tokio::test]
async fn zero_allowed_reductions_in_a_row_means_the_first_one_needed_stops_the_turn() {
    // The guard itself: `max_reductions_in_a_row` is a hard cap on attempts.
    let replies = (0..12)
        .map(|i| ScriptedLlm::say(&format!("reply {i}")))
        .collect();
    // A little over SMALL: the tool definitions alone take most of SMALL's headroom.
    let r = rig_with(replies, Sum::says("s"), SMALL + 500, |cfg| {
        cfg.max_reductions_in_a_row = 0
    });
    let mut last = Ok(Default::default());
    for i in 0..8 {
        last = run(&r, &format!("question {i}: {}", "y".repeat(4_000))).await;
        if last.is_err() {
            break;
        }
    }
    assert!(
        matches!(last, Err(Error::ReductionBudget { attempts: 0 })),
        "{last:?}"
    );
}

#[tokio::test]
async fn a_manual_reduction_previews_then_summarizes_and_can_be_undone() {
    let r = rig(
        vec![
            ScriptedLlm::say("a"),
            ScriptedLlm::say("b"),
            ScriptedLlm::say("c"),
        ],
        Sum::says("Manual summary."),
        30_000,
    );
    for i in 0..3 {
        run(&r, &format!("question {i} {}", "z".repeat(3_000)))
            .await
            .unwrap();
    }
    let plan = r.agent.preview(Mode::Compact).await.unwrap();
    assert!(plan.reclaimed_tokens() > 0 && plan.needs_summarizer());
    assert_eq!(
        r.sum.seen.lock().unwrap().len(),
        0,
        "previewing asks no model"
    );

    let id = r.agent.reduce_now(Mode::Compact, None).await.unwrap();
    assert_eq!(r.sum.seen.lock().unwrap().len(), 1);
    let store = r.agent.store();
    assert!(
        matches!(&store.lock().unwrap().get(id).unwrap().kind, Kind::Reduction(red) if red.trigger == reactor_agent::entry::Trigger::Manual)
    );
    let reduced = project(&store.lock().unwrap()).len();

    // Undo: an append, and the originals are back in the projection.
    store
        .lock()
        .unwrap()
        .append(Kind::Restore { reduction: id })
        .unwrap();
    assert!(project(&store.lock().unwrap()).len() > reduced);
}

// -- cancellation and reporting -------------------------------------------------------------------------

#[tokio::test]
async fn cancelling_mid_batch_kills_the_running_command_and_answers_the_rest() {
    let r = rig(
        vec![ScriptedLlm::call(
            "",
            &[
                (
                    "1",
                    "bash",
                    json!({"command": "sleep 30", "timeout_secs": 60}),
                ),
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
    assert_eq!(
        res.len(),
        2,
        "the log stays replayable: every call has a result"
    );
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
    std::fs::write(
        r.root.join("cfg/settings.json"),
        r#"{"reporting": {"stepThreshold": 2, "maxReverts": 1}}"#,
    )
    .unwrap();

    let out = run(&r, "analyse the sample").await.unwrap();
    assert_eq!(out.reverts, 1);

    let store = r.agent.store();
    let store = store.lock().unwrap();
    // The demand went in as a new user message on a fork from the original.
    let demand = store.all().iter().find(|e| matches!(&e.kind, Kind::User { text } if text.contains("You did not document your findings"))).unwrap();
    let original = store
        .all()
        .iter()
        .find(|e| matches!(&e.kind, Kind::User { text } if text == "analyse the sample"))
        .unwrap();
    assert_eq!(
        demand.parent,
        Some(original.id),
        "forked from the user's message"
    );
    assert!(
        store.leaves().len() >= 2,
        "the reverted turn is a dead branch, still in the log"
    );
    let SessionState { reporting, .. } = SessionState::load(&store);
    assert!(reporting.is_enabled());
}

// -- resuming -------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_session_resumes_from_its_log_identically() {
    let r = rig(
        vec![bash_call("1", "echo hi"), ScriptedLlm::say("done")],
        Sum::says("-"),
        100_000,
    );
    r.agent.command("goal", "find the bug").unwrap();
    run(&r, "go").await.unwrap();
    let case = r.agent.store().lock().unwrap().case().clone();
    let before = project(&r.agent.store().lock().unwrap());
    // One frontend per session: the session is reopened once the agent has let go.
    let Rig { _dir, agent, .. } = r;
    drop(agent);

    let reopened = Store::open(&case, "s").unwrap();
    assert_eq!(project(&reopened), before);
    assert_eq!(
        SessionState::load(&reopened).manifest.goal.as_deref(),
        Some("find the bug")
    );
}

#[tokio::test]
async fn the_file_tools_read_write_and_edit_relative_to_the_working_directory() {
    let r = rig(
        vec![
            ScriptedLlm::call(
                "",
                &[(
                    "1",
                    "write",
                    json!({"path": "notes/a.txt", "content": "alpha\nbeta\ngamma\n"}),
                )],
            ),
            ScriptedLlm::call(
                "",
                &[(
                    "2",
                    "edit",
                    json!({"path": "notes/a.txt", "old_string": "beta", "new_string": "BETA"}),
                )],
            ),
            ScriptedLlm::call(
                "",
                &[(
                    "3",
                    "edit",
                    json!({"path": "notes/a.txt", "old_string": "a", "new_string": "x"}),
                )],
            ),
            ScriptedLlm::call(
                "",
                &[(
                    "4",
                    "read",
                    json!({"path": "notes/a.txt", "offset": 2, "limit": 1}),
                )],
            ),
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
    assert_eq!(
        std::fs::read_to_string(r.root.join("work/notes/a.txt")).unwrap(),
        "alpha\nBETA\ngamma\n"
    );
}

#[tokio::test]
async fn edit_applies_every_replacement_against_the_original_or_none_of_them() {
    let call = |id: &str, args: serde_json::Value| ScriptedLlm::call("", &[(id, "edit", args)]);
    let r = rig(
        vec![
            ScriptedLlm::call(
                "",
                &[(
                    "w",
                    "write",
                    json!({"path": "f.txt", "content": "one\ntwo\nthree\nfour\n"}),
                )],
            ),
            // Two disjoint replacements in one call.
            call(
                "1",
                json!({"path": "f.txt", "edits": [
                {"old_string": "one", "new_string": "1"},
                {"old_string": "three", "new_string": "3"}]}),
            ),
            // One that cannot apply stops them all.
            call(
                "2",
                json!({"path": "f.txt", "edits": [
                {"old_string": "two", "new_string": "2"},
                {"old_string": "nope", "new_string": "x"}]}),
            ),
            // Matched against the original, so overlapping ones are refused.
            call(
                "3",
                json!({"path": "f.txt", "edits": [
                {"old_string": "1\ntwo", "new_string": "A"},
                {"old_string": "two\n3", "new_string": "B"}]}),
            ),
            // Not unique, and not asked to be.
            call(
                "4",
                json!({"path": "f.txt", "edits": [{"old_string": "o", "new_string": "O"}]}),
            ),
            // replace_all.
            call(
                "5",
                json!({"path": "f.txt", "edits": [
                {"old_string": "o", "new_string": "O", "replace_all": true}]}),
            ),
            // The older shape, one replacement at the top level.
            call(
                "6",
                json!({"path": "f.txt", "old_string": "3", "new_string": "three"}),
            ),
            call("7", json!({"path": "f.txt"})),
            ScriptedLlm::say("ok"),
        ],
        Sum::says("-"),
        100_000,
    );
    run(&r, "edits").await.unwrap();
    let res = results(&r);
    assert!(res[1].contains("(2 replacements)"), "{}", res[1]);
    assert!(
        res[2].contains("edit 2: old_string not found"),
        "{}",
        res[2]
    );
    assert!(res[3].contains("edit 1 and edit 2 overlap"), "{}", res[3]);
    assert!(
        res[4].contains("matches 2 times") && res[4].contains("replace_all"),
        "{}",
        res[4]
    );
    assert!(res[5].starts_with("edited "), "{}", res[5]);
    assert!(res[6].starts_with("edited "), "{}", res[6]);
    assert!(res[7].contains("needs `edits`"), "{}", res[7]);
    assert_eq!(
        std::fs::read_to_string(r.root.join("work/f.txt")).unwrap(),
        "1\ntwO\nthree\nfOur\n"
    );
}

// -- large tool groups are loaded on demand (ADR-0055) ----------------------------------------

fn tool_names(req: &reactor_agent::llm::LlmRequest) -> Vec<&str> {
    req.tools.iter().map(|t| t.name.as_str()).collect()
}

#[tokio::test]
async fn a_load_puts_the_tool_in_the_next_request_and_stays_loaded() {
    let r = rig(
        vec![
            ScriptedLlm::call(
                "",
                &[(
                    "t1",
                    "tool_search",
                    json!({"query": "select:graph_query,nope"}),
                )],
            ),
            ScriptedLlm::say("looked"),
            ScriptedLlm::say("again"),
        ],
        Sum::says("-"),
        100_000,
    );
    run(&r, "go").await.unwrap();
    run(&r, "and again").await.unwrap();

    let reqs = r.llm.requests();
    assert!(!tool_names(&reqs[0]).contains(&"graph_query"));
    assert!(
        tool_names(&reqs[1]).contains(&"graph_query"),
        "{:?}",
        tool_names(&reqs[1])
    );
    assert!(
        !tool_names(&reqs[1]).contains(&"graph_write"),
        "only what was asked for"
    );
    assert!(
        tool_names(&reqs[2]).contains(&"graph_query"),
        "a later turn keeps it"
    );

    let res = results(&r);
    assert!(res[0].starts_with("Loaded: graph_query."), "{}", res[0]);
    // The list tool_search shows does not change with a load: the request's prefix holds.
    let listing = |i: usize| {
        reqs[i]
            .tools
            .iter()
            .find(|t| t.name == "tool_search")
            .unwrap()
            .description
            .clone()
    };
    assert_eq!(listing(0), listing(1));
    assert!(
        SessionState::load(&r.agent.store().lock().unwrap())
            .loaded
            .contains("graph_query")
    );
}

#[tokio::test]
async fn words_load_the_best_matches_and_a_miss_says_what_is_left() {
    let r = rig(
        vec![
            ScriptedLlm::call(
                "",
                &[(
                    "t1",
                    "tool_search",
                    json!({"query": "register file hashes"}),
                )],
            ),
            ScriptedLlm::call("", &[("t2", "tool_search", json!({"query": "zebra"}))]),
            ScriptedLlm::call(
                "",
                &[(
                    "t3",
                    "tool_search",
                    json!({"query": "select:artifact_register"}),
                )],
            ),
            ScriptedLlm::say("done"),
        ],
        Sum::says("-"),
        100_000,
    );
    run(&r, "go").await.unwrap();
    let res = results(&r);
    assert!(
        res[0].starts_with("Loaded: artifact_register"),
        "{}",
        res[0]
    );
    assert!(
        res[1].contains("No tool matches `zebra`") && res[1].contains("graph_write"),
        "{}",
        res[1]
    );
    assert!(
        res[2].starts_with("Already loaded: artifact_register"),
        "{}",
        res[2]
    );
}

#[tokio::test]
async fn a_group_under_the_threshold_is_advertised_and_there_is_nothing_to_search() {
    let r = rig_with(
        vec![ScriptedLlm::say("ok")],
        Sum::says("-"),
        100_000,
        no_deferral,
    );
    run(&r, "hi").await.unwrap();
    let names = tool_names(&r.llm.requests()[0])
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert!(names.contains(&"graph_write".to_string()));
    assert!(!names.contains(&"tool_search".to_string()));
    assert!(!r.llm.requests()[0].system.contains("tool_search"));
}

#[tokio::test]
async fn a_threshold_of_zero_defers_every_group_and_a_detached_graph_has_none() {
    let zero = |cfg: &mut AgentConfig| {
        std::fs::create_dir_all(&cfg.paths.config_dir).unwrap();
        std::fs::write(
            cfg.paths.config_dir.join("settings.json"),
            r#"{"toolSearch":{"deferOverTokens":0},"memory":{"caseGraph":false}}"#,
        )
        .unwrap();
    };
    let r = rig_with(vec![ScriptedLlm::say("ok")], Sum::says("-"), 100_000, zero);
    run(&r, "hi").await.unwrap();
    let reqs = r.llm.requests();
    // No graph and no MCP: only the scenario tools are left to load.
    let search = reqs[0]
        .tools
        .iter()
        .find(|t| t.name == "tool_search")
        .expect("the scenario group is deferred too");
    assert!(search.description.contains("scenario_save"));
    assert!(!search.description.contains("graph_write"));
}

// -- settings layers and session scope (ADR-0038) ----------------------------------------------------

#[tokio::test]
async fn the_budget_is_the_baseline_then_the_global_settings_then_the_sessions_own_layer() {
    use reactor_context::settings::ContextSettings;
    let r = rig(vec![], Sum::says("-"), 50_000);
    let base = r.agent.effective_budget();
    assert_eq!(
        (base.window, base.reserve, base.mode),
        (50_000, 100, Mode::Auto),
        "the baseline"
    );

    // Global: settings.json.
    std::fs::create_dir_all(r.root.join("cfg")).unwrap();
    std::fs::write(
        r.root.join("cfg/settings.json"),
        r#"{"context": {"mode": "compact", "reserve": 2000, "pct": 0.8}}"#,
    )
    .unwrap();
    let eff = r.agent.effective_budget();
    assert_eq!(
        (eff.mode, eff.reserve, eff.pct, eff.window),
        (Mode::Compact, 2000, 0.8, 50_000)
    );

    // Session: wins field by field, inherits the rest.
    r.agent
        .set_session_context(&ContextSettings {
            mode: Some("fade".into()),
            window: Some(8_000),
            ..Default::default()
        })
        .unwrap();
    let eff = r.agent.effective_budget();
    assert_eq!(
        (eff.mode, eff.window, eff.reserve, eff.pct),
        (Mode::Fade, 8_000, 2000, 0.8)
    );

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
    let replies = (0..12)
        .map(|i| ScriptedLlm::say(&format!("reply {i}")))
        .collect();
    let r = rig(replies, Sum::says("SUMMARY"), 200_000);
    for i in 0..4 {
        run(&r, &format!("question {i}: {}", "w".repeat(3_000)))
            .await
            .unwrap();
    }
    assert!(
        !kinds(&r).contains(&"reduction"),
        "a big window never reduces"
    );

    r.agent
        .set_session_context(&reactor_context::settings::ContextSettings {
            window: Some(4_500),
            ..Default::default()
        })
        .unwrap();
    run(&r, "and one more").await.unwrap();
    assert!(
        kinds(&r).contains(&"reduction"),
        "the session's smaller window took effect at once"
    );
}

#[tokio::test]
async fn switching_the_model_takes_effect_on_the_next_call() {
    use reactor_agent::llm::{Llm, Switchable};
    let a = Arc::new(ScriptedLlm::new(vec![ScriptedLlm::say("from a")]));
    let b = Arc::new(ScriptedLlm::new(vec![ScriptedLlm::say("from b")]));
    let sw = Switchable::new(a.clone());
    let req = || reactor_agent::llm::LlmRequest {
        system: String::new(),
        messages: vec![Msg::User { text: "hi".into() }],
        tools: vec![],
        max_tokens: None,
        num_ctx: None,
        ..Default::default()
    };

    assert_eq!(
        sw.complete(req(), &mut |_| {}).await.unwrap().blocks,
        vec![Block::Text {
            text: "from a".into()
        }]
    );
    sw.set(b.clone());
    assert_eq!(
        sw.complete(req(), &mut |_| {}).await.unwrap().blocks,
        vec![Block::Text {
            text: "from b".into()
        }]
    );
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

const SCOPE_TOOLSETS: &str = "version = 1\n[toolset.only-alpha]\ndesc = \"alpha only\"\ntags = [\"one\"]\n[toolset.only-beta]\ndesc = \"beta only\"\ntags = [\"two\"]\n[toolset.everything]\ndesc = \"all\"\nall = true\n";

#[tokio::test]
async fn activation_in_one_session_changes_its_registry_block_and_no_other_sessions() {
    use reactor_core::commands::set_activation;
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("tools.toml"), SCOPE_TOOLS).unwrap();
    std::fs::write(cfg_dir.join("toolsets.toml"), SCOPE_TOOLSETS).unwrap();
    let machine = Paths::new(&cfg_dir, Shipped::Embedded);
    // The machine default: everything asked for. (Nothing is active until something is.)
    set_activation(&machine, &["everything".to_string()], false, Some(true)).unwrap();
    let machine_state = std::fs::read_to_string(cfg_dir.join("state.json")).unwrap();
    let session_a = machine
        .clone()
        .with_session_state(dir.path().join("a-activation.json"));
    let session_b = machine
        .clone()
        .with_session_state(dir.path().join("b-activation.json"));

    let make = |paths: Paths| {
        let session = paths
            .session_state
            .as_ref()
            .unwrap()
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let case = reactor_db::Case::open_any(dir.path(), &cfg_dir).unwrap();
        let store = Store::create(&case, &session).unwrap();
        let mut cfg = AgentConfig::new(dir.path().to_path_buf(), paths.clone(), 100_000);
        cfg.registry = reactor_agent::agent::registry_from_core(paths);
        let llm = Arc::new(ScriptedLlm::new(vec![
            ScriptedLlm::say("1"),
            ScriptedLlm::say("2"),
        ]));
        let (agent, _rx) = Agent::new(llm.clone(), Sum::says("-"), store, Tools::new(), cfg);
        (agent, llm)
    };
    let (a, llm_a) = make(session_a.clone());
    let (b, llm_b) = make(session_b);

    a.prompt("go", CancellationToken::new()).await.unwrap();
    b.prompt("go", CancellationToken::new()).await.unwrap();
    for llm in [&llm_a, &llm_b] {
        let sys = &llm.requests()[0].system;
        assert!(
            sys.contains("the alpha tool") && sys.contains("the beta tool"),
            "the machine default is everything:\n{sys}"
        );
    }

    // Session A narrows to alpha. Only A's next request changes.
    set_activation(&session_a, &["everything".to_string()], false, Some(false)).unwrap();
    set_activation(&session_a, &["only-alpha".to_string()], false, Some(true)).unwrap();
    a.prompt("again", CancellationToken::new()).await.unwrap();
    b.prompt("again", CancellationToken::new()).await.unwrap();
    let (sys_a, sys_b) = (&llm_a.requests()[1].system, &llm_b.requests()[1].system);
    assert!(
        sys_a.contains("the alpha tool") && !sys_a.contains("the beta tool"),
        "{sys_a}"
    );
    assert!(
        sys_b.contains("the alpha tool") && sys_b.contains("the beta tool"),
        "{sys_b}"
    );
    assert_eq!(
        std::fs::read_to_string(cfg_dir.join("state.json")).unwrap(),
        machine_state,
        "the machine state was never written by a session"
    );
}

#[tokio::test]
async fn a_frontend_can_measure_list_and_undo_reductions() {
    let replies = (0..6)
        .map(|i| ScriptedLlm::say(&format!("reply {i}")))
        .collect();
    let r = rig(replies, Sum::says("The summary."), 30_000);
    for i in 0..3 {
        run(&r, &format!("question {i} {}", "z".repeat(3_000)))
            .await
            .unwrap();
    }
    let before = r.agent.measure().await.unwrap();
    assert!(
        before.total > before.fixed && before.messages > 2_000,
        "{before:?}"
    );
    assert_eq!(before.calibration, 1.0);
    assert!(r.agent.reductions().is_empty());

    let id = r.agent.reduce_now(Mode::Compact, None).await.unwrap();
    let after = r.agent.measure().await.unwrap();
    assert!(after.messages < before.messages, "{after:?} vs {before:?}");
    let info = &r.agent.reductions()[0];
    assert_eq!(
        (info.entry, info.mode, info.summary.as_deref()),
        (id, Mode::Compact, Some("The summary."))
    );
    assert!(info.covers >= 2 && info.after_tokens < info.before_tokens);

    r.agent.restore(id).unwrap();
    assert!(r.agent.reductions().is_empty());
    assert_eq!(
        r.agent.measure().await.unwrap().messages,
        before.messages,
        "undo brings the originals back exactly"
    );
    assert!(
        r.agent.restore(id).is_err(),
        "and it is only undoable while in force"
    );
}

#[tokio::test]
async fn an_unconfigured_model_fails_each_call_clearly_instead_of_blocking_the_session() {
    use reactor_agent::llm::Llm;
    let llm = reactor_agent::provider::AnyLlm::Missing("no model selected -- pick one".into());
    let e = llm
        .complete(
            reactor_agent::llm::LlmRequest {
                system: String::new(),
                messages: vec![],
                tools: vec![],
                max_tokens: None,
                num_ctx: None,
                ..Default::default()
            },
            &mut |_| {},
        )
        .await
        .unwrap_err();
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
    assert!(
        origins.contains(&Origin::Base) && origins.contains(&Origin::Manifest),
        "{origins:?}"
    );
    assert!(origins.contains(&Origin::ToolDefinition));
    let goal = p
        .segments
        .iter()
        .find(|s| s.origin == Origin::Manifest)
        .unwrap();
    assert!(goal.text.contains("find the loader") && goal.section == Section::System);
    let you = p
        .segments
        .iter()
        .find(|s| s.origin == Origin::User)
        .unwrap();
    assert_eq!(
        (you.text.as_str(), you.section, you.entry.is_some()),
        ("hello there", Section::Messages, true)
    );
    assert!(
        p.segments
            .iter()
            .any(|s| s.origin == Origin::Assistant && s.text == "noted")
    );

    // What is listed is what is measured: the same total the budget works from.
    let m = r.agent.measure().await.unwrap();
    assert_eq!(
        p.total_tokens, m.total,
        "the preview and the budget count the same request"
    );
}

fn thinking_only() -> Result<Reply> {
    Ok(Reply {
        blocks: vec![Block::Thinking {
            text: "hmm, where to begin".into(),
            signature: None,
        }],
        usage: None,
        stop: Some("length".into()),
    })
}

fn summary_request() -> SummaryRequest {
    SummaryRequest {
        system: "You compress.".into(),
        transcript: "#1 user: hi".into(),
        max_tokens: 1_500,
        ..Default::default()
    }
}

#[tokio::test]
async fn a_reasoning_model_that_runs_out_of_room_thinking_is_given_more_and_the_notes_arrive() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = Arc::new(ScriptedLlm::new([
        thinking_only(),
        ScriptedLlm::say("The notes."),
    ]));
    let notes = LlmSummarizer::new(llm.clone())
        .summarize(summary_request())
        .await
        .unwrap();
    assert_eq!(notes, "The notes.");
    let seen = llm.requests();
    assert_eq!(seen.len(), 2);
    assert!(
        seen[0].max_tokens.unwrap() >= 4096,
        "room for thinking as well as the notes"
    );
    assert!(
        seen[1].max_tokens > seen[0].max_tokens,
        "and more on the second try"
    );
    assert!(
        seen[0].system.contains("under about 1125 words"),
        "{}",
        seen[0].system
    );
}

#[tokio::test]
async fn a_summarizer_that_never_writes_notes_fails_saying_why() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = Arc::new(ScriptedLlm::new([thinking_only(), thinking_only()]));
    let e = LlmSummarizer::new(llm)
        .summarize(summary_request())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("only thinking") && e.contains("stop: length") && e.contains("Settings"),
        "{e}"
    );

    let empty = Arc::new(ScriptedLlm::new([Ok(Reply {
        blocks: vec![],
        usage: None,
        stop: Some("stop".into()),
    })]));
    let e = LlmSummarizer::new(empty)
        .summarize(summary_request())
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("nothing at all"), "{e}");
}

// -- the memory switches (ADR-0047) ---------------------------------------------------------

#[tokio::test]
async fn a_session_reaches_the_cases_other_sessions_unless_the_switch_is_off() {
    use reactor_agent::context::{KEY_MEMORY, save_state};
    use reactor_agent::tools::ToolCtx;

    let r = rig(vec![], Sum::says("-"), 100_000);
    let store = r.agent.store();
    let case = store.lock().unwrap().case().clone();
    let mut other = Store::create(&case, "earlier").unwrap();
    other
        .append(Kind::User {
            text: "the beacon interval is 47 seconds".into(),
        })
        .unwrap();

    let ctx = ToolCtx {
        cwd: r.root.join("work"),
        cancel: CancellationToken::new(),
        emit: Arc::new(|_: &str| {}),
        store: store.clone(),
        paths: r.agent.config().paths.clone(),
        scenarios_dir: r.root.join("scenarios"),
        model: "test/model".into(),
        embedder: None,
        limit: Default::default(),
        deferred: Default::default(),
        children: None,
    };
    let tools = Tools::standard(r.root.join("work"));
    let search = tools.find("history_search").unwrap();
    let read = tools.find("history_read").unwrap();
    let index = tools.find("history_index").unwrap();
    let across = json!({ "pattern": "beacon", "all_sessions": true });

    // On by default.
    let out = search.call(across.clone(), &ctx).await;
    assert!(
        out.text.contains("session earlier #1:1 (user)"),
        "{}",
        out.text
    );
    let out = read
        .call(json!({ "id": 1, "session": "earlier" }), &ctx)
        .await;
    assert!(out.text.contains("47 seconds"), "{}", out.text);
    let out = index.call(json!({ "session": "all" }), &ctx).await;
    assert!(
        out.text.contains("earlier") && out.text.contains("s (this session)"),
        "{}",
        out.text
    );
    // Without asking, a search stays in its own session.
    let out = search.call(json!({ "pattern": "beacon" }), &ctx).await;
    assert_eq!(out.text, "no matches");

    // Off globally: every way out of the session explains itself and returns nothing.
    let settings = ctx.paths.config_dir.join("settings.json");
    std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
    std::fs::write(&settings, r#"{"memory":{"sessionHistory":false}}"#).unwrap();
    for (tool, args) in [
        (search, across.clone()),
        (read, json!({ "id": 1, "session": "earlier" })),
        (index, json!({ "session": "all" })),
    ] {
        let out = tool.call(args, &ctx).await;
        assert!(
            out.is_error && out.text.contains("switch is off"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("47 seconds"));
    }
    // Its own history is untouched by the switch.
    assert!(!index.call(json!({}), &ctx).await.is_error);

    // The session's own layer wins over the global one (ADR-0038).
    save_state(
        &mut store.lock().unwrap(),
        KEY_MEMORY,
        json!({ "sessionHistory": true }),
    )
    .unwrap();
    let out = search.call(across, &ctx).await;
    assert!(out.text.contains("session earlier #1:1"), "{}", out.text);
}

// -- the case graph (ADR-0047) --------------------------------------------------------------

#[tokio::test]
async fn the_graph_costs_a_known_amount_of_every_request_and_nothing_when_detached() {
    use reactor_agent::context::estimate_tokens;
    let cost = |r: &Rig| -> (u64, bool, usize) {
        let req = &r.llm.requests()[0];
        let tools: u64 = req
            .tools
            .iter()
            .map(|t| {
                estimate_tokens(&t.name)
                    + estimate_tokens(&t.description)
                    + estimate_tokens(&t.parameters.to_string())
            })
            .sum();
        (
            tools + estimate_tokens(&req.system),
            req.system.contains("## Case graph"),
            req.tools.len(),
        )
    };
    let on = rig_with(
        vec![ScriptedLlm::say("ok")],
        Sum::says("-"),
        100_000,
        no_deferral,
    );
    run(&on, "hi").await.unwrap();
    let off = rig_with(
        vec![ScriptedLlm::say("ok")],
        Sum::says("-"),
        100_000,
        detached,
    );
    run(&off, "hi").await.unwrap();

    let ((with, block, n), (without, no_block, m)) = (cost(&on), cost(&off));
    assert!(block && !no_block);
    assert_eq!((n, m), (16, 10), "six tools come and go with the switch");
    let graph = with - without;
    eprintln!("the case graph costs {graph} tokens of every request (estimated)");
    // A ceiling, so that growing it is a decision and not an accident.
    assert!(graph < 3_000, "{graph}");

    // By default the six wait for tool_search: the block stays, the definitions go.
    let waiting = rig(vec![ScriptedLlm::say("ok")], Sum::says("-"), 100_000);
    run(&waiting, "hi").await.unwrap();
    let (deferred, block, count) = cost(&waiting);
    let left = deferred - without;
    eprintln!("deferred, the case graph costs {left} tokens of every request (estimated)");
    assert!(
        block && count == 11,
        "{count} tools: the ten and tool_search"
    );
    assert!(left + 1_000 < graph, "{left} against {graph}");
}

// -- given documents and # references ---------------------------------------------------------

fn given_text(label: &str, text: &str) -> reactor_agent::entry::Attachment {
    reactor_agent::entry::Attachment {
        label: label.into(),
        source: label.into(),
        part: "whole document".into(),
        text: text.into(),
        tokens: 5,
        truncated: false,
        via: "panel".into(),
    }
}

#[tokio::test]
async fn a_file_named_with_an_at_goes_along_with_the_message_and_stays_text_in_it() {
    let r = rig(vec![ScriptedLlm::say("ok")], Sum::says("-"), 100_000);
    std::fs::write(
        r.root.join("work").join("notes.txt"),
        "the alarm was silenced",
    )
    .unwrap();
    run(&r, "why? see @notes.txt").await.unwrap();

    assert_eq!(kinds(&r), ["session", "attachment", "user", "assistant"]);
    let req = &r.llm.requests()[0];
    let texts: Vec<&str> = req
        .messages
        .iter()
        .filter_map(|m| match m {
            Msg::User { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts.len(), 2);
    assert!(texts[0].contains("the alarm was silenced"));
    assert!(texts[0].contains("notes.txt"));
    assert_eq!(texts[1], "why? see @notes.txt");
}

#[tokio::test]
async fn a_reference_that_names_nothing_is_left_as_text_and_attaches_nothing() {
    let mut r = rig(vec![ScriptedLlm::say("ok")], Sum::says("-"), 100_000);
    run(&r, "look at @missing.txt and @doc:nothing.pdf")
        .await
        .unwrap();
    assert_eq!(kinds(&r), ["session", "user", "assistant"]);
    let notices = drain(&mut r.events)
        .into_iter()
        .filter(|e| matches!(e, Event::Notice(_)))
        .count();
    assert_eq!(notices, 2);
}

#[tokio::test]
async fn something_given_is_in_the_next_request_until_it_is_detached() {
    let r = rig(
        vec![ScriptedLlm::say("one"), ScriptedLlm::say("two")],
        Sum::says("-"),
        100_000,
    );
    let id = r
        .agent
        .attach(given_text("manual.pdf", "bearing limit 85"))
        .unwrap();
    assert_eq!(r.agent.attachments().len(), 1);
    run(&r, "first").await.unwrap();
    assert!(
        r.llm.requests()[0]
            .messages
            .iter()
            .any(|m| matches!(m, Msg::User { text } if text.contains("bearing limit 85")))
    );

    r.agent.detach(id).unwrap();
    assert!(r.agent.attachments().is_empty());
    run(&r, "second").await.unwrap();
    assert!(
        !r.llm.requests()[1]
            .messages
            .iter()
            .any(|m| matches!(m, Msg::User { text } if text.contains("bearing limit 85")))
    );
    // The log keeps it.
    assert!(kinds(&r).contains(&"attachment"));
    assert!(kinds(&r).contains(&"detach"));
}

// -- a turn recovers from a rejected or cut-off request (ADR-0057) -------------------------------

fn cut_call(id: &str) -> Result<Reply> {
    Ok(Reply {
        blocks: vec![Block::ToolCall {
            id: id.into(),
            name: "bash".into(),
            arguments: json!({"command": "echo half a comma"}),
        }],
        usage: None,
        stop: Some("length".into()),
    })
}

fn no_waiting(cfg: &mut AgentConfig) {
    cfg.retry_delay = std::time::Duration::ZERO;
}

fn assistants(r: &Rig) -> usize {
    r.agent
        .store()
        .lock()
        .unwrap()
        .all()
        .iter()
        .filter(|e| matches!(e.kind, Kind::Assistant { .. }))
        .count()
}

fn notices(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Notice(n) => Some(n.clone()),
            _ => None,
        })
        .collect()
}

/// A session with enough older turns that a reduction has something to take.
async fn with_history(r: &Rig) {
    for i in 0..8 {
        run(
            r,
            &format!("older question {i}: {}", "padding ".repeat(500)),
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn a_reply_cut_off_in_a_tool_call_runs_none_of_it_and_the_model_is_told() {
    let mut r = rig(
        vec![cut_call("c1"), ScriptedLlm::say("a smaller step, then")],
        Sum::says("-"),
        100_000,
    );
    let out = run(&r, "do the thing").await.unwrap();

    assert_eq!(out.tool_calls, 0, "the half-made call was not run");
    let res = results(&r);
    assert_eq!(res.len(), 1, "but it was answered, so the log replays");
    assert!(
        res[0].contains("not run: your reply was cut off"),
        "{res:?}"
    );
    assert_eq!(r.llm.requests().len(), 2, "and the model asked again");
    assert!(
        notices(&drain(&mut r.events))
            .iter()
            .any(|n| n.contains("cut off by the output limit")),
    );
}

#[tokio::test]
async fn replies_cut_off_in_a_tool_call_again_and_again_stop_the_turn() {
    let r = rig(
        vec![
            cut_call("c1"),
            cut_call("c2"),
            cut_call("c3"),
            ScriptedLlm::say("never reached"),
        ],
        Sum::says("-"),
        100_000,
    );
    let e = run(&r, "do the thing").await.unwrap_err();
    assert!(
        e.to_string().contains("3 replies in a row were cut off"),
        "{e}"
    );
    assert_eq!(results(&r).len(), 3, "every call still has its result");
    assert_eq!(r.llm.requests().len(), 3);
}

#[tokio::test]
async fn text_cut_off_by_the_output_limit_is_kept_and_said_to_be_cut() {
    let mut r = rig(
        vec![Ok(Reply {
            blocks: vec![Block::Text {
                text: "The binary is packed with UPX and the unpack stub".into(),
            }],
            usage: None,
            stop: Some("length".into()),
        })],
        Sum::says("-"),
        100_000,
    );
    run(&r, "what is it").await.unwrap();
    assert_eq!(assistants(&r), 1, "what it said is kept");
    assert!(
        notices(&drain(&mut r.events))
            .iter()
            .any(|n| n.contains("cut off by the output limit"))
    );
}

#[tokio::test]
async fn a_reply_that_ran_out_of_room_thinking_is_tried_again_once_with_more_room() {
    let mut r = rig(
        vec![thinking_only(), ScriptedLlm::say("the answer")],
        Sum::says("-"),
        100_000,
    );
    run(&r, "work it out").await.unwrap();

    let seen = r.llm.requests();
    assert_eq!(seen.len(), 2);
    assert!(
        seen[1].max_tokens > seen[0].max_tokens,
        "{:?} then {:?}",
        seen[0].max_tokens,
        seen[1].max_tokens
    );
    assert_eq!(
        assistants(&r),
        1,
        "the reply that said nothing is not in the log"
    );
    assert!(
        notices(&drain(&mut r.events))
            .iter()
            .any(|n| n.contains("ran out of output room while thinking"))
    );
}

#[tokio::test]
async fn thinking_that_runs_out_of_room_twice_ends_the_turn_and_says_so() {
    let r = rig(
        vec![thinking_only(), thinking_only(), ScriptedLlm::say("never")],
        Sum::says("-"),
        100_000,
    );
    let e = run(&r, "work it out").await.unwrap_err();
    assert!(
        e.to_string()
            .contains("ran out of output room while thinking"),
        "{e}"
    );
    assert_eq!(r.llm.requests().len(), 2, "one more try, not a loop");
}

#[tokio::test]
async fn a_request_the_provider_finds_too_long_is_reduced_and_sent_again() {
    let mut r = rig(
        (0..8)
            .map(|_| ScriptedLlm::say("noted"))
            .chain([
                Err(Error::Overflow("prompt is too long: 30001 > 20000".into())),
                ScriptedLlm::say("answered"),
            ])
            .collect(),
        Sum::says("notes on the older turns"),
        20_000,
    );
    with_history(&r).await;
    drain(&mut r.events);
    let before = r.agent.measure().await.unwrap().calibration;

    run(&r, "and now?").await.unwrap();

    let events = drain(&mut r.events);
    assert!(
        events.iter().any(|e| matches!(e, Event::Reduced { .. })),
        "the history was reduced"
    );
    assert!(
        notices(&events)
            .iter()
            .any(|n| n.contains("rejected the request as too long")),
    );
    let seen = r.llm.requests();
    let (rejected, retried) = (&seen[seen.len() - 2], &seen[seen.len() - 1]);
    let size = |q: &reactor_agent::llm::LlmRequest| {
        q.messages
            .iter()
            .map(|m| match m {
                Msg::User { text } => text.len(),
                _ => 0,
            })
            .sum::<usize>()
    };
    assert!(
        size(retried) < size(rejected),
        "the second request is smaller: {} then {}",
        size(rejected),
        size(retried)
    );
    assert!(
        r.agent.measure().await.unwrap().calibration > before,
        "and the estimate is trusted less from now on"
    );
}

#[tokio::test]
async fn a_second_rejection_gives_up_and_names_the_window() {
    let r = rig(
        (0..8)
            .map(|_| ScriptedLlm::say("noted"))
            .chain([
                Err(Error::Overflow("prompt is too long".into())),
                Err(Error::Overflow("prompt is too long".into())),
                ScriptedLlm::say("never"),
            ])
            .collect(),
        Sum::says("notes"),
        20_000,
    );
    with_history(&r).await;
    let e = run(&r, "and now?").await.unwrap_err();
    assert!(matches!(e, Error::Overflow(_)), "{e}");
    assert!(
        e.to_string().contains("real window") && e.to_string().contains("20000"),
        "{e}"
    );
}

#[tokio::test]
async fn a_rejection_with_nothing_older_to_reduce_says_what_to_do_about_it() {
    let r = rig(
        vec![Err(Error::Overflow("prompt is too long".into()))],
        Sum::says("-"),
        20_000,
    );
    let e = run(&r, "the very first question").await.unwrap_err();
    assert!(matches!(e, Error::Overflow(_)), "{e}");
    assert!(e.to_string().contains("nothing older to reduce"), "{e}");
    assert_eq!(
        r.llm.requests().len(),
        1,
        "no retry on a request that cannot shrink"
    );
}

#[tokio::test]
async fn a_busy_server_is_tried_again_and_the_turn_goes_on() {
    let mut r = rig_with(
        vec![
            Err(Error::Transient("503 service unavailable".into())),
            Err(Error::Transient("connection reset".into())),
            ScriptedLlm::say("back"),
        ],
        Sum::says("-"),
        100_000,
        no_waiting,
    );
    run(&r, "hello").await.unwrap();
    assert_eq!(r.llm.requests().len(), 3);
    assert_eq!(
        notices(&drain(&mut r.events))
            .iter()
            .filter(|n| n.contains("trying again"))
            .count(),
        2
    );
}

#[tokio::test]
async fn a_server_that_stays_down_is_given_up_on_after_the_retries() {
    let r = rig_with(
        (0..6)
            .map(|_| Err(Error::Transient("503".into())))
            .collect(),
        Sum::says("-"),
        100_000,
        |c| {
            no_waiting(c);
            c.transient_retries = 2;
        },
    );
    let e = run(&r, "hello").await.unwrap_err();
    assert!(matches!(e, Error::Transient(_)), "{e}");
    assert!(e.to_string().contains("gave up after 2 retries"), "{e}");
    assert_eq!(r.llm.requests().len(), 3, "the first try and two more");
}

#[tokio::test]
async fn a_request_the_provider_blocked_is_not_sent_again() {
    let r = rig_with(
        vec![
            Err(Error::Blocked(
                "This request was flagged for cyber policy.".into(),
            )),
            ScriptedLlm::say("never"),
        ],
        Sum::says("-"),
        100_000,
        no_waiting,
    );
    let e = run(&r, "take apart this sample").await.unwrap_err();
    assert!(matches!(e, Error::Blocked(_)), "{e}");
    assert!(e.to_string().contains("was not retried"), "{e}");
    assert_eq!(r.llm.requests().len(), 1);
}

#[tokio::test]
async fn an_error_with_no_recovery_is_passed_on_as_it_is() {
    let r = rig_with(
        vec![Err(Error::Model("invalid api key".into()))],
        Sum::says("-"),
        100_000,
        no_waiting,
    );
    let e = run(&r, "hello").await.unwrap_err();
    assert_eq!(e.to_string(), "invalid api key");
    assert_eq!(r.llm.requests().len(), 1, "and not tried again");
}

// -- every request says how long the reply may be and how big the window is --------------------------

#[tokio::test]
async fn every_request_carries_a_reply_limit_and_the_window_it_is_sized_for() {
    let r = rig(vec![ScriptedLlm::say("hi")], Sum::says("-"), 100_000);
    run(&r, "hello").await.unwrap();
    let q = &r.llm.requests()[0];
    assert_eq!(q.max_tokens, Some(25_000), "a quarter of the window");
    assert_eq!(q.num_ctx, Some(100_000));
}

#[tokio::test]
async fn a_reply_limit_is_cut_to_the_room_the_window_has_left() {
    let r = rig_with(vec![ScriptedLlm::say("hi")], Sum::says("-"), 20_000, |c| {
        c.max_tokens = Some(50_000)
    });
    run(&r, "hello").await.unwrap();
    let limit = r.llm.requests()[0].max_tokens.unwrap();
    assert!(
        (256..20_000).contains(&limit),
        "asked for 50k in a window of 20k, got {limit}"
    );
}

// -- a reduction can be given up on, and a stalled summarizer does not hold the turn ------------------

#[tokio::test]
async fn a_turn_can_be_cancelled_while_a_reduction_waits_for_its_summary() {
    let r = rig(
        (0..9).map(|_| ScriptedLlm::say("noted")).collect(),
        Sum::hangs(),
        20_000,
    );
    // Reductions happen as the history grows; the summarizer never answers.
    let token = CancellationToken::new();
    let t2 = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        t2.cancel();
    });
    let started = std::time::Instant::now();
    let mut ended = None;
    for i in 0..9 {
        if let Err(e) = r
            .agent
            .prompt(
                &format!("question {i}: {}", "padding ".repeat(900)),
                token.clone(),
            )
            .await
        {
            ended = Some(e);
            break;
        }
    }
    assert!(matches!(ended, Some(Error::Cancelled)), "{ended:?}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the wait for the summary ended with the cancel"
    );
    assert!(
        !kinds(&r).contains(&"reduction"),
        "and nothing was written for a summary that never came"
    );
}

// -- windows from 16k to 100k all work -----------------------------------------------------------------

#[tokio::test]
async fn windows_from_16k_to_100k_all_run_a_long_session_without_the_context_filling_up() {
    for window in [16_000u64, 20_000, 32_000, 50_000, 100_000] {
        let r = rig_with(
            (0..40).map(|_| ScriptedLlm::say("noted")).collect(),
            Sum::says("notes on what came before"),
            window,
            |c| c.budget = reactor_agent::budget::BudgetConfig::new(window),
        );
        for i in 0..14 {
            let e = run(
                &r,
                &format!("question {i}: {}", "padding text ".repeat(450)),
            )
            .await;
            assert!(e.is_ok(), "window {window}, turn {i}: {:?}", e.err());
        }
        let m = r.agent.measure().await.unwrap();
        let hard = r.agent.effective_budget().hard();
        assert!(
            m.total <= hard,
            "window {window}: {} tokens in a budget of {hard}",
            m.total
        );
    }
}

// -- the summarizer asks again, smaller, when its own window is overrun ---------------------------------

fn big_parts(n: usize) -> Vec<reactor_agent::budget::TranscriptPart> {
    use reactor_agent::budget::{PartKind, TranscriptPart};
    (0..n)
        .map(|i| TranscriptPart {
            label: format!("#{i} tool result: strings"),
            body: format!("line {i} ").repeat(1_500),
            kind: PartKind::Result,
        })
        .collect()
}

fn big_request(n: usize) -> SummaryRequest {
    let parts = big_parts(n);
    SummaryRequest {
        system: "You compress.".into(),
        transcript: reactor_agent::budget::render_parts(&parts, 6_000),
        max_tokens: 1_500,
        parts,
        ..Default::default()
    }
}

fn user_len(q: &reactor_agent::llm::LlmRequest) -> usize {
    match &q.messages[0] {
        Msg::User { text } => text.len(),
        _ => 0,
    }
}

fn quick(
    s: reactor_agent::llm::LlmSummarizer<Arc<ScriptedLlm>>,
) -> reactor_agent::llm::LlmSummarizer<Arc<ScriptedLlm>> {
    reactor_agent::llm::LlmSummarizer {
        retry_delay: std::time::Duration::ZERO,
        ..s
    }
}

#[tokio::test]
async fn a_summarizer_whose_window_is_overrun_is_asked_again_with_less() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = Arc::new(ScriptedLlm::new([
        Err(Error::Overflow("prompt is too long".into())),
        Err(Error::Overflow("prompt is too long".into())),
        ScriptedLlm::say("The notes."),
    ]));
    let notes = quick(LlmSummarizer::new(llm.clone()))
        .summarize(big_request(24))
        .await
        .unwrap();
    assert_eq!(notes, "The notes.");
    let seen = llm.requests();
    assert_eq!(seen.len(), 3);
    assert!(
        user_len(&seen[1]) < user_len(&seen[0]) && user_len(&seen[2]) < user_len(&seen[1]),
        "smaller each time: {} {} {}",
        user_len(&seen[0]),
        user_len(&seen[1]),
        user_len(&seen[2])
    );
}

#[tokio::test]
async fn a_summarizer_that_never_fits_gives_up_after_three_smaller_tries_saying_so() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = Arc::new(ScriptedLlm::new(
        (0..6).map(|_| Err(Error::Overflow("prompt is too long".into()))),
    ));
    let e = quick(LlmSummarizer::new(llm.clone()))
        .summarize(big_request(24))
        .await
        .unwrap_err();
    assert!(matches!(e, Error::Overflow(_)), "{e}");
    assert!(e.to_string().contains("a third of its size"), "{e}");
    assert_eq!(llm.requests().len(), 4, "the first try and three smaller");
}

#[tokio::test]
async fn a_transcript_over_the_summarizers_cap_is_cut_before_it_is_sent() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = Arc::new(ScriptedLlm::new([ScriptedLlm::say("The notes.")]));
    let mut req = big_request(24);
    req.input_cap = 3_000;
    let whole = req.transcript.len();
    quick(LlmSummarizer::new(llm.clone()))
        .summarize(req)
        .await
        .unwrap();
    let sent = user_len(&llm.requests()[0]);
    assert!(sent < whole / 4, "{whole} bytes cut to {sent}");
    assert!(
        sent / 4 <= 3_000,
        "and within the cap: about {} tokens",
        sent / 4
    );
}

#[tokio::test]
async fn a_summarizer_in_a_small_window_is_told_the_window_and_leaves_room_to_answer() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = Arc::new(ScriptedLlm::new([ScriptedLlm::say("The notes.")]));
    let mut req = summary_request();
    req.window = 8_000;
    quick(LlmSummarizer::new(llm.clone()))
        .summarize(req)
        .await
        .unwrap();
    let q = &llm.requests()[0];
    assert_eq!(q.num_ctx, Some(8_000));
    assert!(q.max_tokens.unwrap() <= 4_000, "{:?}", q.max_tokens);
}

#[tokio::test]
async fn a_busy_summarizer_is_asked_again_after_a_pause() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = Arc::new(ScriptedLlm::new([
        Err(Error::Transient("503".into())),
        ScriptedLlm::say("The notes."),
    ]));
    let notes = quick(LlmSummarizer::new(llm.clone()))
        .summarize(summary_request())
        .await
        .unwrap();
    assert_eq!(notes, "The notes.");
    assert_eq!(llm.requests().len(), 2);
}

#[tokio::test]
async fn a_summarizer_that_fails_is_replaced_by_the_main_model_once() {
    use reactor_agent::llm::LlmSummarizer;
    let cheap =
        Arc::new(ScriptedLlm::new([Err(Error::Model("quota exceeded".into()))]).named("cheap"));
    let main =
        Arc::new(ScriptedLlm::new([ScriptedLlm::say("Notes from the main model.")]).named("main"));
    let notes = quick(LlmSummarizer::new(cheap.clone()).with_fallback(main.clone()))
        .summarize(summary_request())
        .await
        .unwrap();
    assert_eq!(notes, "Notes from the main model.");
    assert_eq!((cheap.requests().len(), main.requests().len()), (1, 1));
}

#[tokio::test]
async fn the_main_model_is_not_asked_again_when_it_is_the_one_that_failed() {
    use reactor_agent::llm::LlmSummarizer;
    let same = Arc::new(ScriptedLlm::new([
        Err(Error::Model("down".into())),
        ScriptedLlm::say("never"),
    ]));
    let e = quick(LlmSummarizer::new(same.clone()).with_fallback(same.clone()))
        .summarize(summary_request())
        .await
        .unwrap_err();
    assert_eq!(e.to_string(), "down");
    assert_eq!(same.requests().len(), 1);
}

#[tokio::test]
async fn when_both_fail_the_error_names_both() {
    use reactor_agent::llm::LlmSummarizer;
    let cheap =
        Arc::new(ScriptedLlm::new([Err(Error::Model("quota exceeded".into()))]).named("cheap"));
    let main =
        Arc::new(ScriptedLlm::new([Err(Error::Model("invalid api key".into()))]).named("main"));
    let e = quick(LlmSummarizer::new(cheap).with_fallback(main))
        .summarize(summary_request())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("quota exceeded") && e.contains("invalid api key"),
        "{e}"
    );
}

// -- the ladder through the loop (ADR-0058) -----------------------------------------------------------

fn dump_cmd(tag: &str) -> String {
    format!("for i in $(seq 1 150); do echo \"{tag} line $i padding to fill the context\"; done")
}

#[tokio::test]
async fn old_tool_output_is_masked_with_no_summary_when_that_is_enough() {
    let mut replies = Vec::new();
    for n in 0..7 {
        replies.push(bash_call(&format!("d{n}"), &dump_cmd(&format!("DUMP{n}"))));
    }
    replies.push(ScriptedLlm::say("done reading"));
    let mut r = rig_with(replies, Sum::says("never asked"), 12_000, detached);
    let out = run(&r, "read the dumps one by one").await.unwrap();

    assert!(out.reductions >= 1, "{out:?}");
    assert!(
        r.sum.seen.lock().unwrap().is_empty(),
        "no summarizer call: the old output was simply cut down"
    );
    let events = drain(&mut r.events);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::Reduced {
            mode: Mode::Fade,
            ..
        }
    )));

    // The last request shows the oldest dumps as stubs and the newest in full.
    let last = r.llm.requests().last().unwrap().clone();
    let shown: Vec<&str> = last
        .messages
        .iter()
        .filter_map(|m| match m {
            Msg::ToolResult { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(shown.len(), 7, "every call still has its result in view");
    assert!(
        shown[0].starts_with("[masked: #"),
        "{}",
        &shown[0][..80.min(shown[0].len())]
    );
    assert!(shown[0].contains("DUMP0 line 1 ") && shown[0].contains("history_read"));
    assert!(
        shown[6].contains("DUMP6 line 75 padding"),
        "the newest is whole: the model has read it"
    );

    // It is listed as a mask, and undoing it brings exactly its results back.
    let masks = r.agent.reductions();
    let masked = masks
        .iter()
        .find(|m| m.masks > 0 && m.covers == 0)
        .expect("a mask in force");
    let entries: Vec<u64> = {
        let store = r.agent.store();
        let store = store.lock().unwrap();
        match &store.get(masked.entry).unwrap().kind {
            Kind::Reduction(red) => red.masks.iter().map(|m| m.entry).collect(),
            _ => panic!(),
        }
    };
    assert!(!entries.is_empty());
    let shown = |agent: &reactor_agent::agent::Agent<Arc<ScriptedLlm>, Arc<Sum>>| -> Vec<String> {
        agent
            .context()
            .into_iter()
            .filter_map(|i| match (i.source, i.msg) {
                (reactor_agent::context::Source::Entry(id), Msg::ToolResult { content, .. })
                    if entries.contains(&id) =>
                {
                    Some(content)
                }
                _ => None,
            })
            .collect()
    };
    assert!(shown(&r.agent).iter().all(|c| c.starts_with("[masked: ")));
    r.agent.restore(masked.entry).unwrap();
    let back = shown(&r.agent);
    assert_eq!(back.len(), entries.len());
    assert!(
        back.iter()
            .all(|c| !c.starts_with("[masked: ") && c.len() > 5_000),
        "whole again"
    );
}

#[tokio::test]
async fn one_prompt_that_drives_a_long_turn_is_reduced_inside_it_and_the_task_stays_in_front() {
    // Before the cut could fall inside a turn, nothing after the person's last message could be
    // reduced, and a long run of tool calls filled the window with nothing to take out.
    let mut replies = Vec::new();
    for n in 0..9 {
        replies.push(bash_call(&format!("d{n}"), &dump_cmd(&format!("STEP{n}"))));
    }
    replies.push(ScriptedLlm::say("found it"));
    let r = rig_with(
        replies,
        Sum::says("EARLIER: read the first steps; nothing yet"),
        8_000,
        |c| {
            detached(c);
            // Summaries, not masks, so the cut itself is what is under test.
            c.budget.mode = Mode::Compact;
        },
    );
    let out = run(&r, "INVESTIGATE EVERYTHING in this sample")
        .await
        .unwrap();
    assert!(out.reductions >= 1, "{out:?}");

    // The reduction kept the task out of what it covers, and anchored itself behind it.
    let task = {
        let store = r.agent.store();
        let store = store.lock().unwrap();
        store
            .all()
            .iter()
            .find(|e| matches!(&e.kind, Kind::User { text } if text.contains("INVESTIGATE")))
            .unwrap()
            .id
    };
    let store = r.agent.store();
    let anchored: Vec<_> = store
        .lock()
        .unwrap()
        .all()
        .iter()
        .filter_map(|e| match &e.kind {
            Kind::Reduction(red) => Some((red.after, red.covers.contains(&task))),
            _ => None,
        })
        .collect();
    assert!(!anchored.is_empty());
    assert!(
        anchored
            .iter()
            .all(|(after, covers_task)| *after == Some(task) && !covers_task),
        "{anchored:?}"
    );

    // In the request, the task comes first and the stand-in is right behind it.
    let q = r.llm.requests().last().unwrap().clone();
    assert!(
        matches!(&q.messages[0], Msg::User { text } if text.contains("INVESTIGATE EVERYTHING"))
    );
    assert!(
        matches!(&q.messages[1], Msg::User { text } if text.contains("EARLIER: read the first steps")),
        "{:?}",
        q.messages[1]
    );
}

#[tokio::test]
async fn a_dump_that_alone_overflows_the_window_is_cut_to_what_fits_and_still_read() {
    // Nothing older to reduce, and the result just arrived: it is the one thing left to cut,
    // and it is cut to the room there is -- the start and the end, not a token's worth.
    let r = rig_with(
        vec![
            bash_call(
                "1",
                "for i in $(seq 1 600); do echo \"huge dump line number $i of a very long listing\"; done",
            ),
            ScriptedLlm::say("read what I could"),
        ],
        Sum::says("never"),
        SMALL,
        detached,
    );
    run(&r, "dump it").await.unwrap();

    let second = &r.llm.requests()[1];
    let Some(Msg::ToolResult { content, .. }) = second
        .messages
        .iter()
        .find(|m| matches!(m, Msg::ToolResult { .. }))
    else {
        panic!("{:?}", second.messages)
    };
    assert!(
        content.starts_with("[masked: #"),
        "{}",
        &content[..60.min(content.len())]
    );
    assert!(
        content.contains("huge dump line number 1 ")
            && content.contains("huge dump line number 600 "),
        "both ends survive"
    );
    assert!(
        content.len() > 1_000,
        "and more than a stub's worth: {} bytes",
        content.len()
    );
    // Whole in the log.
    assert!(results(&r)[0].len() > 10_000);
}

#[tokio::test]
async fn masking_is_for_auto_only() {
    for mode in [Mode::Compact, Mode::Fade] {
        let mut replies: Vec<_> = (0..7)
            .map(|n| bash_call(&format!("d{n}"), &dump_cmd(&format!("DUMP{n}"))))
            .collect();
        replies.push(ScriptedLlm::say("done"));
        let r = rig_with(replies, Sum::says("EARLIER notes"), 12_000, |c| {
            detached(c);
            c.budget.mode = mode;
        });
        run(&r, "read the dumps").await.unwrap();
        let store = r.agent.store();
        let store = store.lock().unwrap();
        assert!(
            store
                .all()
                .iter()
                .all(|e| !matches!(&e.kind, Kind::Reduction(red) if red.is_mask())),
            "{mode:?}: the person asked for {}",
            if mode == Mode::Fade {
                "the range to go"
            } else {
                "summaries"
            }
        );
    }
}

#[tokio::test]
async fn notes_that_ignore_the_headings_are_asked_for_once_more() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = Arc::new(ScriptedLlm::new([
        ScriptedLlm::say("just some loose notes"),
        ScriptedLlm::say("## Goal\nfind the C2\n## Established\nit is 10.1.2.3\n## Next\nconfirm"),
    ]));
    let mut req = summary_request();
    req.template = true;
    let notes = LlmSummarizer::new(llm.clone())
        .summarize(req)
        .await
        .unwrap();
    assert!(notes.contains("## Established"));
    let seen = llm.requests();
    assert_eq!(seen.len(), 2);
    let Msg::User { text } = &seen[1].messages[0] else {
        panic!()
    };
    assert!(
        text.ends_with(reactor_agent::budget::SUMMARY_REMINDER),
        "{text}"
    );
}

#[tokio::test]
async fn notes_that_ignore_the_headings_twice_are_taken_as_they_are() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = Arc::new(ScriptedLlm::new([
        ScriptedLlm::say("loose notes"),
        ScriptedLlm::say("still loose, but useful: the key is 0x5a"),
    ]));
    let mut req = summary_request();
    req.template = true;
    let notes = LlmSummarizer::new(llm.clone())
        .summarize(req)
        .await
        .unwrap();
    assert!(
        notes.contains("0x5a"),
        "a small model that cannot follow a template is not failed for it"
    );
    assert_eq!(llm.requests().len(), 2);

    // And a request that does not ask for headings is not held to them.
    let llm = Arc::new(ScriptedLlm::new([ScriptedLlm::say("loose notes")]));
    LlmSummarizer::new(llm.clone())
        .summarize(summary_request())
        .await
        .unwrap();
    assert_eq!(llm.requests().len(), 1);
}

#[tokio::test]
async fn the_mask_settings_layer_like_the_others() {
    use reactor_context::settings::ContextSettings;
    let r = rig(vec![], Sum::says("-"), 50_000);
    let base = r.agent.effective_budget();
    assert_eq!((base.mask_target, base.protect), (0.6, 0.15));

    std::fs::create_dir_all(r.root.join("cfg")).unwrap();
    std::fs::write(
        r.root.join("cfg/settings.json"),
        r#"{"context": {"maskTarget": 0.5, "protect": 0.1}}"#,
    )
    .unwrap();
    let eff = r.agent.effective_budget();
    assert_eq!(
        (eff.mask_target, eff.protect),
        (0.5, 0.1),
        "the global layer"
    );

    r.agent
        .set_session_context(&ContextSettings {
            protect: Some(0.3),
            ..Default::default()
        })
        .unwrap();
    let eff = r.agent.effective_budget();
    assert_eq!(
        (eff.mask_target, eff.protect),
        (0.5, 0.3),
        "the session wins field by field"
    );
}

// -- the summary request begins as the main request did (ADR-0058) ---------------------------------------

fn main_replay(model: &str) -> reactor_agent::budget::Replay {
    use reactor_agent::llm::ToolSpec;
    reactor_agent::budget::Replay {
        system: "THE MAIN SYSTEM PROMPT".into(),
        tools: vec![ToolSpec {
            name: "bash".into(),
            description: "run a command".into(),
            parameters: json!({"type": "object"}),
        }],
        messages: vec![
            Msg::User {
                text: "the task".into(),
            },
            Msg::Assistant {
                blocks: vec![Block::Text {
                    text: "looking".into(),
                }],
            },
        ],
        model: model.into(),
    }
}

fn main_llm(replies: impl IntoIterator<Item = Result<Reply>>) -> Arc<ScriptedLlm> {
    Arc::new(ScriptedLlm::new(replies).named("main"))
}

const NOTES: &str = "## Goal\nfind the C2\n## Established\nit is 10.1.2.3\n## Next\nconfirm";

#[tokio::test]
async fn a_summarizer_that_is_the_main_model_asks_the_way_the_main_request_was_made() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = main_llm([ScriptedLlm::say(NOTES)]);
    let mut req = summary_request();
    req.replay = Some(main_replay("main"));
    let notes = LlmSummarizer::new(llm.clone())
        .summarize(req)
        .await
        .unwrap();
    assert_eq!(notes, NOTES);

    let q = &llm.requests()[0];
    let replay = main_replay("main");
    assert_eq!(
        q.system, replay.system,
        "the main request's own system prompt"
    );
    assert_eq!(q.tools, replay.tools, "and its tools");
    assert_eq!(
        q.messages[..2],
        replay.messages[..],
        "and its messages, as they were sent"
    );
    assert_eq!(q.messages.len(), 3);
    let Msg::User { text } = &q.messages[2] else {
        panic!()
    };
    assert!(
        text.contains("Everything above is the part of the session to be summarized")
            && text.contains("Do not call any tool"),
        "{text}"
    );
    assert!(
        text.contains("You compress."),
        "the instruction is the last message"
    );
    assert!(q.no_tools, "and the model is told not to call one");
}

#[tokio::test]
async fn another_model_than_the_main_one_gets_the_transcript_on_its_own() {
    use reactor_agent::llm::LlmSummarizer;
    let cheap = Arc::new(ScriptedLlm::new([ScriptedLlm::say(NOTES)]).named("cheap"));
    let mut req = summary_request();
    req.replay = Some(main_replay("main"));
    LlmSummarizer::new(cheap.clone())
        .summarize(req)
        .await
        .unwrap();
    let q = &cheap.requests()[0];
    assert!(
        q.system.starts_with("You compress."),
        "its own prompt: {}",
        q.system
    );
    assert!(q.tools.is_empty() && !q.no_tools);
    assert_eq!(q.messages.len(), 1, "the transcript, as one message");
}

#[tokio::test]
async fn a_request_too_big_to_replay_is_asked_the_other_way_without_trying() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = main_llm([ScriptedLlm::say(NOTES)]);
    let mut req = summary_request();
    req.replay = Some(main_replay("main"));
    req.input_cap = 5; // far less than the replay comes to
    LlmSummarizer::new(llm.clone())
        .summarize(req)
        .await
        .unwrap();
    let q = &llm.requests()[0];
    assert!(
        q.tools.is_empty() && !q.no_tools,
        "no request was wasted on the replay"
    );
    assert_eq!(llm.requests().len(), 1);
}

#[tokio::test]
async fn a_replay_the_provider_rejects_as_too_long_falls_back_to_the_shrinking_way() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = main_llm([
        Err(Error::Overflow("prompt is too long".into())),
        ScriptedLlm::say(NOTES),
    ]);
    let mut req = big_request(6);
    req.replay = Some(main_replay("main"));
    let notes = LlmSummarizer::new(llm.clone())
        .summarize(req)
        .await
        .unwrap();
    assert_eq!(notes, NOTES);
    let seen = llm.requests();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].no_tools, "first the main model's way");
    assert!(
        !seen[1].no_tools && seen[1].tools.is_empty(),
        "then the transcript alone"
    );
    assert!(
        user_len(&seen[1]) > 1_000,
        "and at its full size: the rejection of the replay does not count as one of the three cuts"
    );
}

#[tokio::test]
async fn a_reply_that_calls_a_tool_or_says_nothing_is_not_notes_and_is_asked_again_the_other_way() {
    use reactor_agent::llm::LlmSummarizer;
    for first in [
        ScriptedLlm::call("", &[("c1", "bash", json!({"command": "ls"}))]),
        Ok(Reply {
            blocks: vec![],
            usage: None,
            stop: Some("stop".into()),
        }),
    ] {
        let llm = main_llm([first, ScriptedLlm::say(NOTES)]);
        let mut req = summary_request();
        req.replay = Some(main_replay("main"));
        let notes = LlmSummarizer::new(llm.clone())
            .summarize(req)
            .await
            .unwrap();
        assert_eq!(notes, NOTES);
        let seen = llm.requests();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].no_tools && !seen[1].no_tools);
    }
}

#[tokio::test]
async fn the_reminder_and_the_extra_room_work_inside_the_replay_too() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = main_llm([
        thinking_only(),
        ScriptedLlm::say("loose notes"),
        ScriptedLlm::say(NOTES),
    ]);
    let mut req = summary_request();
    req.template = true;
    req.replay = Some(main_replay("main"));
    let notes = LlmSummarizer::new(llm.clone())
        .summarize(req)
        .await
        .unwrap();
    assert_eq!(notes, NOTES);
    let seen = llm.requests();
    assert_eq!(seen.len(), 3);
    assert!(
        seen.iter().all(|q| q.no_tools),
        "all three the main model's way"
    );
    assert!(
        seen[1].max_tokens > seen[0].max_tokens,
        "more room after the thinking"
    );
    let Msg::User { text } = seen[2].messages.last().unwrap() else {
        panic!()
    };
    assert!(
        text.ends_with(reactor_agent::budget::SUMMARY_REMINDER),
        "{text}"
    );
}

#[tokio::test]
async fn the_loop_gives_the_summarizer_what_the_main_model_was_sent() {
    let r = rig(
        (0..9).map(|_| ScriptedLlm::say("noted")).collect(),
        Sum::says("## Goal\nread\n## Established\nnothing\n## Next\nmore"),
        9_000,
    );
    with_history(&r).await;
    let seen = r.sum.seen.lock().unwrap().clone();
    assert!(!seen.is_empty(), "the history was reduced");
    let first = &r.llm.requests()[0];
    let replay = seen[0].replay.clone().expect("a replay was offered");
    assert_eq!(replay.model, "scripted");
    assert_eq!(replay.system, first.system, "the same system prompt");
    assert_eq!(
        replay.tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
        first.tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
        "the same tools"
    );
    assert!(replay.messages.len() >= 2);
    assert_eq!(
        replay.messages[0], first.messages[0],
        "and the same messages, from the first"
    );
}

// -- the output limit follows the window --------------------------------------------------------------------

fn shown_size(window: u64, settings: Option<&'static str>) -> usize {
    let tweak = move |c: &mut AgentConfig| {
        c.budget = reactor_agent::budget::BudgetConfig::new(window);
        if let Some(json) = settings {
            std::fs::create_dir_all(&c.paths.config_dir).unwrap();
            std::fs::write(c.paths.config_dir.join("settings.json"), json).unwrap();
        }
    };
    let r = rig_with(
        vec![
            bash_call("1", "head -c 60000 /dev/zero | tr '\\0' 'x'"),
            ScriptedLlm::say("seen"),
        ],
        Sum::says("-"),
        window,
        tweak,
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(run(&r, "print a lot")).unwrap();
    results(&r)[0].len()
}

#[test]
fn what_the_model_sees_of_one_result_is_a_twentieth_of_the_window_unless_it_was_set() {
    // Four bytes a token, so a fifth of the window in bytes: 3.2 KB, 20 KB and 32 KiB.
    for (window, limit) in [
        (16_000u64, 3_200usize),
        (100_000, 20_000),
        (200_000, 32 * 1024),
    ] {
        let shown = shown_size(window, None);
        assert!(
            (limit * 9 / 10..=limit + 1_500).contains(&shown),
            "window {window}: {shown} bytes shown of a limit of {limit}"
        );
    }
    // A limit that was set is the person's, whatever the window.
    let set = shown_size(16_000, Some(r#"{"toolOutput":{"maxInlineBytes":8192}}"#));
    assert!((7_400..=9_700).contains(&set), "{set}");
}

// -- the same call, again and again ------------------------------------------------------------------------------

fn same_calls(n: usize, cmd: &str) -> Vec<Result<Reply>> {
    (0..n).map(|i| bash_call(&format!("c{i}"), cmd)).collect()
}

#[tokio::test]
async fn the_same_call_with_the_same_result_eight_times_in_a_row_ends_the_turn_and_says_so() {
    let mut replies = same_calls(12, "echo still nothing here");
    replies.push(ScriptedLlm::say("never reached"));
    let r = rig(replies, Sum::says("-"), 100_000);
    let e = run(&r, "look again").await.unwrap_err();
    assert!(
        e.to_string().contains("was called 8 times in a row")
            && e.to_string().contains("still nothing here"),
        "{e}"
    );
    assert_eq!(r.llm.requests().len(), 8, "no ninth request");
    assert_eq!(results(&r).len(), 8, "and every call has its result");
}

#[tokio::test]
async fn the_same_call_that_gives_a_different_result_each_time_is_not_a_loop() {
    // Waiting on something by asking again is not going round in circles.
    let mut replies = same_calls(8, "cat /proc/sys/kernel/random/uuid");
    replies.push(ScriptedLlm::say("done waiting"));
    let r = rig(replies, Sum::says("-"), 100_000);
    run(&r, "poll it").await.unwrap();
    assert_eq!(results(&r).len(), 8);
}

#[tokio::test]
async fn another_call_in_between_starts_the_count_again() {
    let mut replies = Vec::new();
    for round in 0..3 {
        for i in 0..4 {
            replies.push(bash_call(&format!("a{round}-{i}"), "echo same"));
        }
        replies.push(bash_call(&format!("b{round}"), "echo different"));
    }
    replies.push(ScriptedLlm::say("done"));
    let r = rig(replies, Sum::says("-"), 100_000);
    run(&r, "alternate").await.unwrap();
}

#[tokio::test]
async fn nine_identical_calls_in_one_reply_stop_at_the_eighth_and_the_ninth_is_answered_unrun() {
    let calls: Vec<(String, &str, serde_json::Value)> = (0..9)
        .map(|i| (format!("c{i}"), "bash", json!({"command": "echo same"})))
        .collect();
    let refs: Vec<(&str, &str, serde_json::Value)> = calls
        .iter()
        .map(|(a, b, c)| (a.as_str(), *b, c.clone()))
        .collect();
    let r = rig(
        vec![ScriptedLlm::call("", &refs), ScriptedLlm::say("never")],
        Sum::says("-"),
        100_000,
    );
    let e = run(&r, "go").await.unwrap_err();
    assert!(e.to_string().contains("8 times in a row"), "{e}");
    let res = results(&r);
    assert_eq!(res.len(), 9);
    assert!(res[8].contains("not run: the turn was stopped"), "{res:?}");
}

#[tokio::test]
async fn the_detector_can_be_turned_off() {
    let mut replies = same_calls(8, "echo again");
    replies.push(ScriptedLlm::say("done"));
    let r = rig_with(replies, Sum::says("-"), 100_000, |c| c.max_repeats = 0);
    run(&r, "go").await.unwrap();
    assert_eq!(results(&r).len(), 8);
}

// -- a repeated call is first a notice (ADR-0060) ------------------------------------------------------------

fn advisories(rig: &Rig) -> Vec<usize> {
    results(rig)
        .iter()
        .enumerate()
        .filter(|(_, c)| c.contains("[reactor]"))
        .map(|(i, _)| i)
        .collect()
}

#[tokio::test]
async fn the_third_identical_call_gets_one_line_asking_what_it_is_waiting_for_and_no_other_does() {
    let mut replies = same_calls(6, "echo nothing new");
    replies.push(ScriptedLlm::say("done"));
    let r = rig(replies, Sum::says("-"), 100_000);
    run(&r, "look").await.unwrap();
    assert_eq!(advisories(&r), vec![2], "the third result, once");
    let third = &results(&r)[2];
    assert!(
        third.starts_with("nothing new") && third.contains("3rd identical call to `bash`"),
        "after the tool's own words, not instead of them: {third}"
    );
    assert!(!third.to_lowercase().contains("must not") && !third.contains("stop"));
}

#[tokio::test]
async fn arguments_in_another_order_are_the_same_call() {
    let a = json!({"command": "echo same", "session": "main"});
    let b = json!({"session": "main", "command": "echo same"});
    let mut replies: Vec<Result<Reply>> = Vec::new();
    for (i, args) in [a.clone(), b.clone(), a, b].iter().enumerate() {
        replies.push(ScriptedLlm::call("", &[(&format!("c{i}"), "bash", args.clone())]));
    }
    replies.push(ScriptedLlm::say("done"));
    let r = rig(replies, Sum::says("-"), 100_000);
    run(&r, "go").await.unwrap();
    assert_eq!(advisories(&r), vec![2]);
}

#[tokio::test]
async fn the_same_tool_failing_three_times_in_a_row_is_noticed_even_with_other_arguments() {
    let mut replies: Vec<Result<Reply>> = (0..5)
        .map(|i| ScriptedLlm::call("", &[(&format!("c{i}"), "no_such_tool", json!({"n": i}))]))
        .collect();
    replies.push(ScriptedLlm::say("done"));
    let r = rig(replies, Sum::says("-"), 100_000);
    run(&r, "go").await.unwrap();
    assert_eq!(advisories(&r), vec![2]);
    assert!(results(&r)[2].contains("`no_such_tool` has failed 3 times in a row"));
}

#[tokio::test]
async fn a_success_in_between_starts_the_failures_again() {
    let fail = |i: usize| ScriptedLlm::call("", &[(&format!("f{i}"), "no_such_tool", json!({"n": i}))]);
    let replies = vec![
        fail(0),
        fail(1),
        bash_call("ok", "echo fine"),
        fail(2),
        fail(3),
        ScriptedLlm::say("done"),
    ];
    let r = rig(replies, Sum::says("-"), 100_000);
    run(&r, "go").await.unwrap();
    assert!(advisories(&r).is_empty());
}

#[tokio::test]
async fn the_notices_can_be_turned_off() {
    let mut replies = same_calls(4, "echo again");
    replies.push(ScriptedLlm::say("done"));
    let r = rig_with(replies, Sum::says("-"), 100_000, |c| {
        c.repeat_notice = 0;
        c.failure_notice = 0;
    });
    run(&r, "go").await.unwrap();
    assert!(advisories(&r).is_empty());
}

// -- Part D: the count, the focus, the second look, the threshold, the warning ---------------------------------

fn counted(text: &str, input: u64) -> Result<Reply> {
    Ok(Reply {
        blocks: vec![Block::Text { text: text.into() }],
        usage: Some(reactor_agent::entry::Usage {
            input_tokens: input,
            output_tokens: 100,
            ..Default::default()
        }),
        stop: Some("stop".into()),
    })
}

#[tokio::test]
async fn what_the_provider_counted_decides_when_to_reduce_not_the_estimate() {
    // The estimate of this session is a few thousand tokens. The provider says the last request
    // was 16,800 of a window of 20,000 (trigger: 16,915): the next message crosses it.
    let r = rig(
        vec![
            ScriptedLlm::say("a"),
            counted("b", 16_800),
            ScriptedLlm::say("c"),
        ],
        Sum::says("## Goal\nx\n## Established\ny\n## Next\nz"),
        20_000,
    );
    run(&r, &format!("first: {}", "padding ".repeat(300)))
        .await
        .unwrap();
    run(&r, &format!("second: {}", "padding ".repeat(300)))
        .await
        .unwrap();
    assert!(r.sum.seen.lock().unwrap().is_empty(), "nothing yet");
    let m = r.agent.measure().await.unwrap();
    assert!(
        m.real >= 16_900,
        "the count plus what came after: {}",
        m.real
    );
    assert!(
        m.real > m.total * 3 / 2,
        "{} against an estimate of {}",
        m.real,
        m.total
    );

    let out = run(&r, &format!("third: {}", "padding ".repeat(300)))
        .await
        .unwrap();
    assert!(
        out.reductions >= 1,
        "the count said the window was nearly full: {out:?}"
    );
    assert!(!r.sum.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_count_is_there_when_a_session_is_reopened_and_the_estimate_alone_would_not_be() {
    let r = rig(vec![counted("a", 12_000)], Sum::says("-"), 20_000);
    run(&r, "hello").await.unwrap();
    let before = r.agent.measure().await.unwrap();
    assert!(before.real >= 12_100);

    // A new Agent over the same log, as when a window is reopened: its learned ratio starts at 1.
    let case = r.agent.store().lock().unwrap().case().clone();
    let Rig {
        _dir: dir, agent, ..
    } = r;
    drop(agent);
    let reopened = Store::open(&case, "s").unwrap();
    let paths = Paths::new(dir.path().join("cfg"), Shipped::Embedded);
    let mut cfg = AgentConfig::new(dir.path().join("work"), paths, 20_000);
    cfg.registry = registry("## Available tools (this machine)\n\nsh   shell", &["sh"]);
    cfg.budget.reserve = 100;
    let llm = Arc::new(ScriptedLlm::new(vec![]));
    let (agent, _ev) = Agent::new(
        llm,
        Sum::says("-"),
        reopened,
        Tools::standard(dir.path().join("work")),
        cfg,
    );
    let after = agent.measure().await.unwrap();
    assert!(after.calibration > 1.5, "{}", after.calibration);
    assert!(after.real >= 12_100, "{}", after.real);
}

#[tokio::test]
async fn a_reply_records_the_fixed_part_of_the_request_it_answered() {
    let r = rig(vec![counted("a", 3_000)], Sum::says("-"), 20_000);
    run(&r, "hello").await.unwrap();
    let store = r.agent.store();
    let store = store.lock().unwrap();
    let usage = store
        .all()
        .iter()
        .find_map(|e| match &e.kind {
            Kind::Assistant { usage, .. } => *usage,
            _ => None,
        })
        .unwrap();
    assert!(usage.fixed_estimate > 100, "{usage:?}");
    assert_eq!(usage.input_tokens, 3_000);
}

#[tokio::test]
async fn a_reduction_asked_for_with_a_focus_tells_the_summarizer_to_keep_it() {
    let r = rig(
        (0..9).map(|_| ScriptedLlm::say("noted")).collect(),
        Sum::says("## Goal\nx\n## Established\ny\n## Next\nz"),
        60_000,
    );
    for i in 0..4 {
        run(&r, &format!("question {i}: {}", "padding ".repeat(300)))
            .await
            .unwrap();
    }
    r.agent
        .reduce_now(Mode::Compact, Some("  the C2 address and the XOR key  "))
        .await
        .unwrap();
    let seen = r.sum.seen.lock().unwrap().clone();
    assert!(
        seen[0]
            .system
            .contains("keep this in particular, in full: the C2 address and the XOR key"),
        "{}",
        seen[0].system
    );

    // Without one, nothing is added; and a blank one is none.
    r.agent.reduce_now(Mode::Compact, None).await.ok();
    r.agent.reduce_now(Mode::Compact, Some("   ")).await.ok();
    for later in &r.sum.seen.lock().unwrap()[1..] {
        assert!(!later.system.contains("in particular"), "{}", later.system);
    }
}

#[tokio::test]
async fn a_fade_has_no_summary_so_a_focus_is_not_asked_of_anyone() {
    let r = rig(
        (0..5).map(|_| ScriptedLlm::say("noted")).collect(),
        Sum::says("-"),
        60_000,
    );
    for i in 0..3 {
        run(&r, &format!("question {i}: {}", "padding ".repeat(300)))
            .await
            .unwrap();
    }
    r.agent
        .reduce_now(Mode::Fade, Some("the C2 findings"))
        .await
        .unwrap();
    assert!(r.sum.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_reduction_by_hand_checks_its_summary_and_one_by_the_budget_does_not_unless_asked() {
    let mk = |settings: Option<&'static str>| {
        rig_with(
            (0..12).map(|_| ScriptedLlm::say("noted")).collect(),
            Sum::says("## Goal\nx\n## Established\ny\n## Next\nz"),
            9_000,
            move |c: &mut AgentConfig| {
                if let Some(json) = settings {
                    std::fs::create_dir_all(&c.paths.config_dir).unwrap();
                    std::fs::write(c.paths.config_dir.join("settings.json"), json).unwrap();
                }
            },
        )
    };
    // By the budget, nothing asked: no second look.
    let r = mk(None);
    with_history(&r).await;
    let seen = r.sum.seen.lock().unwrap().clone();
    assert!(!seen.is_empty() && seen.iter().all(|q| !q.verify));
    // By hand: always.
    drop(seen);
    r.agent.reduce_now(Mode::Compact, None).await.ok();
    assert!(r.sum.seen.lock().unwrap().last().unwrap().verify);
    // By the budget, with the setting on: the second look.
    let r = mk(Some(r#"{"context":{"verify":true}}"#));
    with_history(&r).await;
    assert!(r.sum.seen.lock().unwrap().iter().all(|q| q.verify));
}

#[tokio::test]
async fn the_deferral_threshold_follows_the_window_unless_it_was_set() {
    let at = |window: u64, settings: Option<&'static str>| {
        let r = rig_with(
            vec![],
            Sum::says("-"),
            window,
            move |c: &mut AgentConfig| {
                c.budget = reactor_agent::budget::BudgetConfig::new(window);
                if let Some(json) = settings {
                    std::fs::create_dir_all(&c.paths.config_dir).unwrap();
                    std::fs::write(c.paths.config_dir.join("settings.json"), json).unwrap();
                }
            },
        );
        r.agent.deferral_threshold()
    };
    assert_eq!(at(200_000, None), 1_000, "what it always was");
    assert_eq!(at(100_000, None), 1_000);
    assert_eq!(at(32_000, None), 320);
    assert_eq!(at(8_000, None), 250);
    assert_eq!(
        at(8_000, Some(r#"{"toolSearch":{"deferOverTokens":4000}}"#)),
        4_000
    );
}

#[tokio::test]
async fn a_window_the_fixed_part_fills_is_told_so_once_and_the_work_goes_on() {
    let mut r = rig(
        vec![ScriptedLlm::say("a"), ScriptedLlm::say("b")],
        Sum::says("-"),
        4_000,
    );
    run(&r, "hello").await.unwrap();
    run(&r, "and again").await.unwrap();
    let said: Vec<String> = notices(&drain(&mut r.events))
        .into_iter()
        .filter(|n| n.contains("fixed part of every request"))
        .collect();
    assert_eq!(said.len(), 1, "once, not on every request: {said:?}");

    // The preview carries it too.
    let p = r.agent.context_preview().await.unwrap();
    assert_eq!(p.warnings.len(), 1);

    // A roomy window has nothing to say.
    let roomy = rig(vec![], Sum::says("-"), 100_000);
    assert!(
        roomy
            .agent
            .context_preview()
            .await
            .unwrap()
            .warnings
            .is_empty()
    );
}

// -- the second look at a summary ---------------------------------------------------------------------------------

fn verifying(req: SummaryRequest) -> SummaryRequest {
    SummaryRequest {
        verify: true,
        template: true,
        ..req
    }
}

#[tokio::test]
async fn a_summary_that_is_checked_is_replaced_by_the_better_one_the_model_writes() {
    use reactor_agent::llm::LlmSummarizer;
    let first = "## Goal\nfind the C2\n## Established\nit is 10.1.2.3\n## Next\nconfirm";
    let better = "## Goal\nfind the C2\n## Established\nit is 10.1.2.3, port 4443, XOR key 0x5a\n## Next\nconfirm";
    let llm = Arc::new(ScriptedLlm::new([
        ScriptedLlm::say(first),
        ScriptedLlm::say(better),
    ]));
    let notes = LlmSummarizer::new(llm.clone())
        .summarize(verifying(summary_request()))
        .await
        .unwrap();
    assert_eq!(notes, better);
    let seen = llm.requests();
    assert_eq!(seen.len(), 2);
    // The second request is the first, then the notes as the model's own words, then the question.
    assert_eq!(seen[1].system, seen[0].system);
    assert_eq!(
        seen[1].messages[..seen[0].messages.len()],
        seen[0].messages[..]
    );
    let n = seen[1].messages.len();
    assert!(
        matches!(&seen[1].messages[n - 2], Msg::Assistant { blocks } if blocks == &vec![Block::Text { text: first.into() }])
    );
    assert!(
        matches!(&seen[1].messages[n - 1], Msg::User { text } if text == reactor_agent::budget::SUMMARY_VERIFY)
    );
}

#[tokio::test]
async fn a_check_that_fails_or_loses_the_notes_leaves_them_as_they_were() {
    use reactor_agent::llm::LlmSummarizer;
    let first = "## Goal\nfind the C2 and the key\n## Established\nit is 10.1.2.3 and the XOR key is 0x5a\n## Next\nconfirm both";
    for second in [
        Err(Error::Model("quota".into())),
        ScriptedLlm::say(""),
        ScriptedLlm::say("## Goal\nshort\n## Established\nx\n## Next\ny"),
        ScriptedLlm::say(
            "the same, but without headings, as one long paragraph about the sample and its key",
        ),
    ] {
        let llm = Arc::new(ScriptedLlm::new([ScriptedLlm::say(first), second]));
        let notes = LlmSummarizer::new(llm.clone())
            .summarize(verifying(summary_request()))
            .await
            .unwrap();
        assert_eq!(notes, first);
        assert_eq!(llm.requests().len(), 2);
    }
}

#[tokio::test]
async fn a_summary_that_is_not_asked_to_be_checked_is_not() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = Arc::new(ScriptedLlm::new([ScriptedLlm::say(NOTES)]));
    LlmSummarizer::new(llm.clone())
        .summarize(summary_request())
        .await
        .unwrap();
    assert_eq!(llm.requests().len(), 1);
}

#[tokio::test]
async fn the_check_is_made_the_main_models_way_too_with_its_tools_and_none_called() {
    use reactor_agent::llm::LlmSummarizer;
    let better =
        "## Goal\nfind the C2\n## Established\nit is 10.1.2.3 and port 4443\n## Next\nconfirm";
    let llm = main_llm([ScriptedLlm::say(NOTES), ScriptedLlm::say(better)]);
    let mut req = verifying(summary_request());
    req.replay = Some(main_replay("main"));
    let notes = LlmSummarizer::new(llm.clone())
        .summarize(req)
        .await
        .unwrap();
    assert_eq!(notes, better);
    let seen = llm.requests();
    assert!(
        seen.iter()
            .all(|q| q.no_tools && q.system == "THE MAIN SYSTEM PROMPT" && !q.tools.is_empty())
    );
}

// -- a reply the provider's filter stopped ---------------------------------------------------------------------

fn filtered(blocks: Vec<Block>) -> Result<Reply> {
    Ok(Reply {
        blocks,
        usage: None,
        stop: Some("contentfilter".into()),
    })
}

#[tokio::test]
async fn a_reply_the_filter_stopped_before_it_said_anything_ends_the_turn_and_is_not_retried() {
    for empty in [
        filtered(vec![]),
        filtered(vec![Block::Thinking {
            text: "let me look at the sample".into(),
            signature: None,
        }]),
    ] {
        let r = rig(
            vec![empty, ScriptedLlm::say("never")],
            Sum::says("-"),
            100_000,
        );
        let e = run(&r, "take apart this sample").await.unwrap_err();
        assert!(matches!(e, Error::Blocked(_)), "{e}");
        assert!(
            e.to_string().contains("filter stopped the reply")
                && e.to_string().contains("not retried"),
            "{e}"
        );
        assert_eq!(
            r.llm.requests().len(),
            1,
            "the same request would be stopped again"
        );
        assert_eq!(assistants(&r), 0, "and an empty reply is not recorded");
    }
}

#[tokio::test]
async fn what_a_reply_said_before_the_filter_stopped_it_is_kept_and_said_to_be_cut() {
    let mut r = rig(
        vec![filtered(vec![Block::Text {
            text: "The unpacker reads the key from".into(),
        }])],
        Sum::says("-"),
        100_000,
    );
    run(&r, "what does it do").await.unwrap();
    assert_eq!(assistants(&r), 1);
    assert!(
        notices(&drain(&mut r.events))
            .iter()
            .any(|n| n.contains("filter stopped the reply") && n.contains("kept"))
    );
}

#[tokio::test]
async fn a_tool_call_in_a_reply_the_filter_stopped_is_not_run_and_the_model_is_told() {
    let mut r = rig(
        vec![
            filtered(vec![Block::ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                arguments: json!({"command": "echo half"}),
            }]),
            ScriptedLlm::say("a different approach"),
        ],
        Sum::says("-"),
        100_000,
    );
    let out = run(&r, "do the step").await.unwrap();
    assert_eq!(out.tool_calls, 0);
    let res = results(&r);
    assert_eq!(res.len(), 1, "it was answered, so the log replays");
    assert!(
        res[0].contains("not run: the provider's filter stopped your reply"),
        "{res:?}"
    );
    assert_eq!(r.llm.requests().len(), 2, "and the model asked again");
    assert!(
        notices(&drain(&mut r.events))
            .iter()
            .any(|n| n.contains("filter stopped the reply in the middle of a tool call"))
    );
}

#[tokio::test]
async fn replies_the_filter_keeps_stopping_in_a_tool_call_end_the_turn_as_blocked() {
    let call = |id: &str| {
        filtered(vec![Block::ToolCall {
            id: id.into(),
            name: "bash".into(),
            arguments: json!({"command": "echo half"}),
        }])
    };
    let r = rig(
        vec![
            call("c1"),
            call("c2"),
            call("c3"),
            ScriptedLlm::say("never"),
        ],
        Sum::says("-"),
        100_000,
    );
    let e = run(&r, "do the step").await.unwrap_err();
    assert!(matches!(e, Error::Blocked(_)), "{e}");
    assert!(
        e.to_string()
            .contains("3 replies in a row were stopped by the provider's filter"),
        "{e}"
    );
    assert_eq!(results(&r).len(), 3, "every call still has its result");
}

#[tokio::test]
async fn a_summary_the_filter_stopped_is_blocked_not_a_model_that_wrote_nothing() {
    use reactor_agent::llm::LlmSummarizer;
    let llm = Arc::new(ScriptedLlm::new([filtered(vec![]), filtered(vec![])]));
    let e = LlmSummarizer::new(llm.clone())
        .summarize(summary_request())
        .await
        .unwrap_err();
    assert!(matches!(e, Error::Blocked(_)), "{e}");
    assert!(e.to_string().contains("filter stopped the summary"), "{e}");

    // The main model gets a try when the configured summarizer's filter stopped it.
    let cheap = Arc::new(ScriptedLlm::new([filtered(vec![])]).named("cheap"));
    let main = Arc::new(ScriptedLlm::new([ScriptedLlm::say(NOTES)]).named("main"));
    let notes = LlmSummarizer::new(cheap)
        .with_fallback(main)
        .summarize(summary_request())
        .await
        .unwrap();
    assert_eq!(notes, NOTES);
}

// -- typed input waits in the log and is delivered by its class (ADR-0061) ------------------------------------

use reactor_agent::inbox::Class;

fn user_texts(req: &reactor_agent::llm::LlmRequest) -> Vec<String> {
    req.messages
        .iter()
        .filter_map(|m| match m {
            Msg::User { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_steer_is_delivered_after_the_tool_call_settles_and_before_the_next_request() {
    let r = rig(
        vec![bash_call("c1", "echo one"), ScriptedLlm::say("done")],
        Sum::says("-"),
        100_000,
    );
    assert!(r.agent.admit("s1", Class::Steer, "also look at the imports").unwrap());
    run(&r, "start").await.unwrap();
    let reqs = r.llm.requests();
    assert_eq!(reqs.len(), 2, "the turn went on; nothing was interrupted");
    assert_eq!(user_texts(&reqs[0]), vec!["start"], "the first request did not have it");
    assert_eq!(
        user_texts(&reqs[1]),
        vec!["start", "also look at the imports"],
        "the second did, after the result of the call in flight"
    );
    assert!(r.agent.pending_inbox().is_empty());
}

#[tokio::test]
async fn a_steer_that_arrives_with_the_final_answer_gets_one_more_round_not_a_new_turn() {
    let r = rig(
        vec![ScriptedLlm::say("all done"), ScriptedLlm::say("and the imports")],
        Sum::says("-"),
        100_000,
    );
    r.agent.admit("s1", Class::Steer, "what about the imports?").unwrap();
    let out = run(&r, "go").await.unwrap();
    assert_eq!(out.rounds, 2);
    assert!(r.agent.pending_inbox().is_empty());
}

#[tokio::test]
async fn a_queued_message_waits_for_the_turn_to_end_and_then_runs_as_a_turn_of_its_own() {
    let r = rig(
        vec![
            bash_call("c1", "echo one"),
            ScriptedLlm::say("first done"),
            ScriptedLlm::say("second done"),
        ],
        Sum::says("-"),
        100_000,
    );
    r.agent.admit("q1", Class::Queue, "now the strings").unwrap();
    run(&r, "start").await.unwrap();
    assert!(
        r.llm.requests().iter().all(|q| !user_texts(q).contains(&"now the strings".to_string())),
        "not seen by the turn it waited behind"
    );
    assert_eq!(r.agent.pending_inbox().len(), 1, "and still waiting");

    let out = r
        .agent
        .prompt_pending(CancellationToken::new())
        .await
        .expect("something was waiting")
        .unwrap();
    assert_eq!(out.rounds, 1);
    assert!(r.agent.pending_inbox().is_empty());
    assert!(user_texts(r.llm.requests().last().unwrap()).contains(&"now the strings".to_string()));
    assert!(
        r.agent.prompt_pending(CancellationToken::new()).await.is_none(),
        "nothing left to run"
    );
}

#[tokio::test]
async fn taking_input_back_is_a_tombstone_and_it_is_never_sent() {
    let r = rig(vec![ScriptedLlm::say("done")], Sum::says("-"), 100_000);
    r.agent.admit("a", Class::Steer, "first thought").unwrap();
    r.agent.admit("b", Class::Queue, "second thought").unwrap();
    assert_eq!(r.agent.withdraw(None).unwrap().as_deref(), Some("second thought"), "the latest");
    assert_eq!(r.agent.withdraw(Some("zzz")).unwrap(), None, "an unknown item is nothing");
    assert_eq!(r.agent.withdraw_all().unwrap(), vec!["first thought".to_string()]);
    run(&r, "go").await.unwrap();
    assert!(user_texts(&r.llm.requests()[0]) == vec!["go"]);
    // The entries are still in the log: nothing was deleted.
    let n = r
        .agent
        .store()
        .lock()
        .unwrap()
        .all()
        .iter()
        .filter(|e| matches!(&e.kind, Kind::Custom { key, .. } if key.starts_with("inbox")))
        .count();
    assert_eq!(n, 4, "two admissions and two tombstones");
}

#[tokio::test]
async fn a_steer_names_a_file_with_an_at_like_any_message_does() {
    let r = rig(
        vec![bash_call("c1", "echo one"), ScriptedLlm::say("done")],
        Sum::says("-"),
        100_000,
    );
    std::fs::write(r.root.join("work").join("notes.txt"), "the sample was packed").unwrap();
    r.agent.admit("a", Class::Steer, "see @notes.txt").unwrap();
    run(&r, "go").await.unwrap();
    let second = &r.llm.requests()[1];
    assert!(
        second
            .messages
            .iter()
            .any(|m| matches!(m, Msg::User { text } if text.contains("the sample was packed"))),
        "the attachment is in the request that carries the steer"
    );
}

// -- a subtask goes to a child and only its report comes back (ADR-0062) -----------------------------------

fn agent_call(id: &str, args: serde_json::Value) -> Result<Reply> {
    ScriptedLlm::call("", &[(id, "agent", args)])
}

fn names_of(req: &reactor_agent::llm::LlmRequest) -> Vec<String> {
    req.tools.iter().map(|t| t.name.clone()).collect()
}

#[tokio::test]
async fn a_child_works_in_a_session_of_its_own_and_only_its_report_returns() {
    // The one scripted model answers whoever asks, in order: the parent, then the child, then the
    // parent again.
    let mut r = rig(
        vec![
            agent_call("a1", json!({"task": "list the imports of dropper.bin"})),
            ScriptedLlm::say("It imports 3 functions: connect, send, CreateProcess."),
            ScriptedLlm::say("The sample networks and spawns."),
        ],
        Sum::says("-"),
        100_000,
    );
    r.agent.enable_children();
    run(&r, "triage the sample").await.unwrap();

    let reqs = r.llm.requests();
    assert_eq!(reqs.len(), 3);
    assert!(names_of(&reqs[0]).contains(&"agent".to_string()), "the parent has it");
    assert_eq!(
        user_texts(&reqs[1]),
        vec!["list the imports of dropper.bin"],
        "the child sees its task and nothing of the parent's conversation"
    );
    assert!(
        !names_of(&reqs[1]).contains(&"agent".to_string()),
        "and does not start children of its own"
    );
    assert!(names_of(&reqs[1]).contains(&"bash".to_string()));

    let result = results(&r).pop().unwrap();
    assert!(
        result.starts_with("It imports 3 functions") && result.contains("[agent session "),
        "the report and where the rest is: {result}"
    );

    // The child is a session of the case, with its task and its parent in its log.
    let case = r.agent.store().lock().unwrap().case().clone();
    let all = reactor_db::sessions::list(case.db()).unwrap();
    let child = all.iter().find(|s| s.id != "s").expect("a second session");
    let log = Store::open(&case, &child.id).unwrap();
    assert!(log.all().iter().any(|e| matches!(&e.kind, Kind::User { text } if text.contains("dropper.bin"))));
    assert!(log.all().iter().any(|e| matches!(&e.kind, Kind::Custom { key, data } if key == "origin" && data["parent"] == "s")));
    assert!(result.contains(&format!("[agent session {}", child.id)));
}

#[tokio::test]
async fn a_child_gets_only_the_tools_it_was_given_and_is_told_of_the_ones_that_do_not_exist() {
    let mut r = rig(
        vec![
            agent_call("a1", json!({"task": "read it", "tools": ["read", "no_such_tool"]})),
            ScriptedLlm::say("read nothing"),
            ScriptedLlm::say("done"),
        ],
        Sum::says("-"),
        100_000,
    );
    r.agent.enable_children();
    run(&r, "go").await.unwrap();
    assert_eq!(names_of(&r.llm.requests()[1]), vec!["read".to_string()]);
    assert!(results(&r).pop().unwrap().contains("left out: no_such_tool"));
}

#[tokio::test]
async fn a_session_without_children_is_not_offered_the_tool() {
    let r = rig(vec![ScriptedLlm::say("hi")], Sum::says("-"), 100_000);
    run(&r, "go").await.unwrap();
    assert!(!names_of(&r.llm.requests()[0]).contains(&"agent".to_string()));
}

#[tokio::test]
async fn a_child_that_fails_gives_the_parent_an_error_result_not_a_failed_turn() {
    let mut r = rig(
        vec![
            agent_call("a1", json!({"task": "do it"})),
            Err(Error::Model("the model is down".into())),
            ScriptedLlm::say("I will do it myself"),
        ],
        Sum::says("-"),
        100_000,
    );
    r.agent.enable_children();
    run(&r, "go").await.unwrap();
    let result = results(&r).pop().unwrap();
    assert!(result.contains("it stopped") && result.contains("the model is down"), "{result}");
}

// -- children in parallel, in the background, and linked to their parent (ADR-0063) -------------------------

use reactor_agent::llm::{Delta, Llm, LlmRequest};

/// Answers the parent from one script and every child from another, so that children running
/// together cannot take each other's replies.
struct Split {
    parent: ScriptedLlm,
    child: ScriptedLlm,
}

impl Llm for Split {
    async fn complete(
        &self,
        req: LlmRequest,
        on_delta: &mut (dyn FnMut(Delta) + Send),
    ) -> Result<Reply> {
        if req.tools.iter().any(|t| t.name == "agent") {
            self.parent.complete(req, on_delta).await
        } else {
            self.child.complete(req, on_delta).await
        }
    }

    fn name(&self) -> String {
        "split".into()
    }
}

struct SplitRig {
    _dir: tempfile::TempDir,
    agent: Agent<Arc<Split>, Arc<Sum>>,
    llm: Arc<Split>,
}

fn split_rig(parent: Vec<Result<Reply>>, child: Vec<Result<Reply>>) -> SplitRig {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let case = reactor_db::Case::create(&cwd, None).unwrap();
    let store = Store::create(&case, "s").unwrap();
    let paths = Paths::new(dir.path().join("cfg"), Shipped::Embedded);
    let mut cfg = AgentConfig::new(cwd.clone(), paths, 100_000);
    cfg.registry = registry("## Available tools (this machine)\n\nsh   shell", &["sh"]);
    cfg.scenarios_dir = dir.path().join("scenarios");
    cfg.budget.reserve = 100;
    let llm = Arc::new(Split {
        parent: ScriptedLlm::new(parent),
        child: ScriptedLlm::new(child),
    });
    let (mut agent, _events) = Agent::new(llm.clone(), Sum::says("-"), store, Tools::standard(cwd), cfg);
    agent.enable_children();
    SplitRig {
        _dir: dir,
        agent,
        llm,
    }
}

fn split_results(r: &SplitRig) -> Vec<String> {
    r.agent
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

fn two_agents() -> Result<Reply> {
    ScriptedLlm::call(
        "",
        &[
            ("a1", "agent", json!({"task": "first"})),
            ("a2", "agent", json!({"task": "second"})),
        ],
    )
}

#[tokio::test]
async fn children_asked_for_together_run_together_and_are_entered_in_the_order_they_were_asked() {
    // Each child waits for a file the other writes. One after another, the first would wait
    // for ever; together, both get through.
    let wait_for = |mine: &str, theirs: &str| {
        bash_call(
            "b",
            &format!(
                "touch {mine}; for i in $(seq 1 100); do [ -f {theirs} ] && exit 0; sleep 0.05; done; echo never met; exit 1"
            ),
        )
    };
    let r = split_rig(
        vec![two_agents(), ScriptedLlm::say("both back")],
        vec![
            wait_for("one.flag", "two.flag"),
            wait_for("two.flag", "one.flag"),
            ScriptedLlm::say("report A"),
            ScriptedLlm::say("report B"),
        ],
    );
    r.agent
        .prompt("go", CancellationToken::new())
        .await
        .unwrap();
    let res = split_results(&r);
    assert_eq!(res.len(), 2);
    assert!(res.iter().all(|c| c.contains("[agent session ")), "{res:?}");
    assert!(
        res.iter().any(|c| c.starts_with("report A")) && res.iter().any(|c| c.starts_with("report B")),
        "{res:?}"
    );
    assert_eq!(r.llm.parent.requests().len(), 2, "the parent asked twice");
}

#[tokio::test]
async fn switched_to_serial_they_run_one_after_another_in_order() {
    let r = split_rig(
        vec![two_agents(), ScriptedLlm::say("both back")],
        vec![ScriptedLlm::say("report A"), ScriptedLlm::say("report B")],
    );
    let said = r.agent.command("children", "serial").unwrap();
    assert!(said.notices[0].message.contains("one after another"));
    assert!(!reactor_context::settings::Settings::load(&r.agent.config().paths).children.parallel);
    r.agent
        .prompt("go", CancellationToken::new())
        .await
        .unwrap();
    let res = split_results(&r);
    assert!(res[0].starts_with("report A") && res[1].starts_with("report B"), "{res:?}");
    // And back on, with a limit.
    r.agent.command("children", "parallel on").unwrap();
    r.agent.command("children", "max 2").unwrap();
    let s = reactor_context::settings::Settings::load(&r.agent.config().paths).children;
    assert!(s.parallel && s.max_parallel == 2);
    assert!(r.agent.command("children", "max zero").is_err());
}

#[tokio::test]
async fn a_child_started_in_the_background_is_collected_with_wait() {
    let r = split_rig(
        vec![
            ScriptedLlm::call("", &[("a1", "agent", json!({"task": "enumerate", "background": true}))]),
            ScriptedLlm::call("", &[("w1", "wait", json!({}))]),
            ScriptedLlm::say("collected"),
        ],
        vec![ScriptedLlm::say("the enumeration")],
    );
    r.agent
        .prompt("go", CancellationToken::new())
        .await
        .unwrap();
    let res = split_results(&r);
    assert!(res[0].contains("started in the background as session"), "{res:?}");
    assert!(res[1].starts_with("the enumeration"), "{res:?}");
}

#[tokio::test]
async fn wait_names_a_child_or_takes_the_next_and_says_when_there_is_none() {
    let r = split_rig(
        vec![
            ScriptedLlm::call("", &[("w0", "wait", json!({}))]),
            ScriptedLlm::call("", &[("a1", "agent", json!({"task": "one", "background": true}))]),
            ScriptedLlm::call("", &[("w1", "wait", json!({"session": "999"}))]),
            ScriptedLlm::say("done"),
        ],
        vec![ScriptedLlm::say("report")],
    );
    r.agent
        .prompt("go", CancellationToken::new())
        .await
        .unwrap();
    let res = split_results(&r);
    assert!(res[0].contains("no agent is running in the background"), "{res:?}");
    assert!(res[2].contains("no agent `999`"), "{res:?}");
}

#[tokio::test]
async fn a_childs_session_is_linked_to_its_parent() {
    let r = split_rig(
        vec![
            ScriptedLlm::call("", &[("a1", "agent", json!({"task": "list the imports"}))]),
            ScriptedLlm::say("done"),
        ],
        vec![ScriptedLlm::say("three imports")],
    );
    r.agent
        .prompt("go", CancellationToken::new())
        .await
        .unwrap();
    let case = r.agent.store().lock().unwrap().case().clone();
    let all = reactor_db::sessions::list(case.db()).unwrap();
    let child = all.iter().find(|s| s.id != "s").unwrap();
    assert_eq!(child.parent.as_deref(), Some("s"));
    assert_eq!(all.iter().find(|s| s.id == "s").unwrap().parent, None);
}

// -- stopping and following children, and a turn that panics (ADR-0064) ---------------------------------

#[tokio::test]
async fn a_running_child_is_listed_and_can_be_stopped_without_ending_the_parents_turn() {
    let r = split_rig(
        vec![
            ScriptedLlm::call("", &[("a1", "agent", json!({"task": "wait a long time"}))]),
            ScriptedLlm::say("it was stopped, so I go on"),
        ],
        vec![bash_call("b", "sleep 30")],
    );
    let prompt = r.agent.prompt("go", CancellationToken::new());
    let control = async {
        let mut listed = None;
        for _ in 0..200 {
            let running = r.agent.running_children();
            if !running.is_empty() {
                listed = Some(running);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let stopped = r.agent.stop_children(None);
        (listed, stopped)
    };
    let (out, (listed, stopped)) = tokio::join!(prompt, control);
    out.unwrap();
    let listed = listed.expect("the child was listed while it ran");
    assert_eq!(listed.len(), 1);
    assert!(listed[0].task.contains("wait a long time") && !listed[0].background);
    assert_eq!(stopped, 1);
    let res = split_results(&r);
    assert!(res[0].contains("cancelled before it finished"), "{res:?}");
    assert!(r.agent.running_children().is_empty(), "and it is off the list");
    assert_eq!(r.llm.parent.requests().len(), 2, "the parent went on to its next reply");
}

#[tokio::test]
async fn stopping_names_a_session_and_an_unknown_one_stops_nothing() {
    let r = split_rig(vec![ScriptedLlm::say("hi")], vec![]);
    assert_eq!(r.agent.stop_children(Some("99")), 0);
    assert_eq!(r.agent.stop_children(None), 0);
    let said = r.agent.command("agents", "").unwrap();
    assert!(said.notices[0].message.contains("no agent is running"));
    assert!(r.agent.command("agents", "stop 99").unwrap().notices[0].message.contains("no agent of session 99"));
}

#[tokio::test]
async fn a_child_that_keeps_going_is_stopped_at_its_round_limit_and_returns_what_it_said() {
    let r = split_rig(
        vec![
            ScriptedLlm::call("", &[("a1", "agent", json!({"task": "never finish"}))]),
            ScriptedLlm::say("done"),
        ],
        (0..10)
            .map(|i| ScriptedLlm::call("still working", &[(&format!("b{i}"), "bash", json!({"command": format!("echo {i}")}))]))
            .collect(),
    );
    let mut settings = reactor_context::settings::Settings::load(&r.agent.config().paths);
    settings.children.max_rounds = 3;
    settings.save(&r.agent.config().paths).unwrap();
    r.agent.prompt("go", CancellationToken::new()).await.unwrap();
    let res = split_results(&r);
    assert!(
        res[0].contains("it stopped: stopped after 3 model round trips")
            && res[0].contains("What it had said last:\nstill working"),
        "{res:?}"
    );
}

/// Panics on its first request, answers after.
struct PanicsOnce {
    done: std::sync::atomic::AtomicBool,
    inner: ScriptedLlm,
}

impl Llm for PanicsOnce {
    async fn complete(
        &self,
        req: LlmRequest,
        on_delta: &mut (dyn FnMut(Delta) + Send),
    ) -> Result<Reply> {
        if !self.done.swap(true, std::sync::atomic::Ordering::SeqCst) {
            panic!("boom");
        }
        self.inner.complete(req, on_delta).await
    }

    fn name(&self) -> String {
        "panics-once".into()
    }
}

#[tokio::test]
async fn a_turn_that_panics_ends_with_an_error_and_the_agent_goes_on_working() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let case = reactor_db::Case::create(&cwd, None).unwrap();
    let store = Store::create(&case, "s").unwrap();
    let mut cfg = AgentConfig::new(cwd.clone(), Paths::new(dir.path().join("cfg"), Shipped::Embedded), 100_000);
    cfg.registry = registry("## Available tools (this machine)\n\nsh   shell", &["sh"]);
    cfg.scenarios_dir = dir.path().join("scenarios");
    let llm = Arc::new(PanicsOnce {
        done: false.into(),
        inner: ScriptedLlm::new(vec![ScriptedLlm::say("back again")]),
    });
    let (agent, _events) = Agent::new(llm, Sum::says("-"), store, Tools::standard(cwd), cfg);
    let e = agent.prompt("first", CancellationToken::new()).await.unwrap_err();
    assert!(e.to_string().contains("internal error: boom"), "{e}");
    // The log is whole and the next turn works.
    agent.prompt("second", CancellationToken::new()).await.unwrap();
}
