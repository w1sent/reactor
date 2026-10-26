//! The MCP client against a real server process: `examples/mcp_fixture.rs`, spoken to over
//! stdio exactly as a user's server would be.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use reactor_agent::agent::{Agent, AgentConfig};
use reactor_agent::budget::{Summarizer, SummaryRequest};
use reactor_agent::llm::ScriptedLlm;
use reactor_agent::mcp::{McpHost, Standing};
use reactor_agent::prompt::RegistryView;
use reactor_agent::store::Store;
use reactor_agent::tools::Tools;
use reactor_core::Paths;
use reactor_core::catalogue::load_catalogue;
use reactor_core::mcp::{Cache, Config, Reach, Server, Transport};
use reactor_core::paths::Shipped;
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// `cargo test` builds the example next to the test binaries.
fn fixture() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap().join("examples");
    let path = dir.join("mcp_fixture");
    assert!(path.is_file(), "{} was not built", path.display());
    path
}

fn server(name: &str, args: &[&str], env: &[(&str, &str)]) -> Server {
    Server {
        name: name.into(),
        transport: Transport::Stdio {
            command: fixture().display().to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            env: env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
        },
        tags: vec!["lab".into()],
        enabled: true,
    }
}

struct Fx {
    _dir: tempfile::TempDir,
    paths: Paths,
}

fn fx(servers: Vec<Server>) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path().join("cfg"), Shipped::Embedded);
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    // The shipped catalogue is not what these tests are about.
    std::fs::write(paths.live("tools.toml"), "version = 1\n").unwrap();
    std::fs::write(paths.live("toolsets.toml"), "version = 1\n").unwrap();
    Config {
        prefix: true,
        servers,
    }
    .save(&paths)
    .unwrap();
    Fx { _dir: dir, paths }
}

/// Until every server has settled, one way or the other.
async fn settled(host: &Arc<McpHost>) {
    for _ in 0..200 {
        if host
            .status()
            .iter()
            .all(|s| s.standing != Standing::Connecting)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("still connecting: {:?}", host.status());
}

fn ids(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| n.to_string()).collect()
}

#[tokio::test]
async fn a_server_is_connected_its_tools_listed_and_the_catalogue_told() {
    let fx = fx(vec![server("fx", &[], &[])]);
    let host = McpHost::new(fx.paths.clone());
    host.sync();
    settled(&host).await;

    let status = host.status();
    assert_eq!(status[0].standing, Standing::Connected);
    assert_eq!(status[0].tools, 6);

    // Only what is active is advertised, in id order, with the schema the server gave.
    let specs = host.specs(&ids(&[
        "mcp__fx__echo",
        "mcp__fx__fail",
        "mcp__fx__not-there",
    ]));
    assert_eq!(
        specs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["mcp__fx__echo", "mcp__fx__fail"]
    );
    assert_eq!(specs[0].parameters["properties"]["text"]["type"], "string");
    assert_eq!(specs[0].parameters["type"], "object");
    assert!(host.specs(&ids(&[])).is_empty());

    // The cache is what the catalogue reads: the tools, with the tag and a cost.
    let cache = Cache::load(&fx.paths);
    let cached = &cache.servers["fx"];
    assert_eq!(cached.reach, Reach::Connected);
    let echo = cached.tools.iter().find(|t| t.name == "echo").unwrap();
    assert_eq!(echo.desc, "Say the text back.");
    assert!(echo.tokens > 10, "{}", echo.tokens);
    let cat = load_catalogue(&fx.paths).unwrap();
    assert_eq!(cat.get("mcp__fx__echo").unwrap().tags, ["mcp", "fx", "lab"]);
    // A name a model API refuses is made one; the server's own name is kept for calling it.
    let odd = cat.get("mcp__fx__odd_name_v2").unwrap();
    assert_eq!(odd.mcp.as_ref().unwrap().tool, "odd name.v2");
}

#[tokio::test]
async fn a_call_gets_the_servers_answer_whether_or_not_the_tool_is_active() {
    let fx = fx(vec![server(
        "fx",
        &[],
        &[("FIXTURE_TOKEN", "${REACTOR_MCP_AGENT_TEST}")],
    )]);
    // SAFETY: a variable only this test reads.
    unsafe { std::env::set_var("REACTOR_MCP_AGENT_TEST", "hunter2") };
    let host = McpHost::new(fx.paths.clone());
    host.sync();
    settled(&host).await;
    let never = CancellationToken::new();

    let out = host
        .call("mcp__fx__echo", json!({"text": "hi"}), &never)
        .await;
    assert_eq!((out.text.as_str(), out.is_error), ("echo: hi", false));

    let out = host.call("mcp__fx__fail", json!({}), &never).await;
    assert_eq!((out.text.as_str(), out.is_error), ("it broke", true));

    // What a model cannot be handed is said in words.
    let out = host.call("mcp__fx__image", json!(null), &never).await;
    assert!(
        out.text.starts_with("here\n\n[image: image/png, 6 bytes"),
        "{}",
        out.text
    );

    // The environment of a server is expanded from ours.
    let out = host
        .call("mcp__fx__env", json!({"name": "FIXTURE_TOKEN"}), &never)
        .await;
    assert_eq!(out.text, "hunter2");

    let out = host
        .call("mcp__fx__echo", json!("not an object"), &never)
        .await;
    assert!(
        out.is_error && out.text.contains("JSON object"),
        "{}",
        out.text
    );
    assert!(host.call("mcp__fx__nope", json!({}), &never).await.is_error);
    assert!(host.knows("mcp__fx__echo") && !host.knows("bash") && !host.knows("mcp__fx__nope"));
}

#[tokio::test]
async fn cancelling_ends_a_call_that_never_answers() {
    let fx = fx(vec![server("fx", &[], &[])]);
    let host = McpHost::new(fx.paths.clone());
    host.sync();
    settled(&host).await;
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        c.cancel();
    });
    let out = tokio::time::timeout(
        Duration::from_secs(10),
        host.call("mcp__fx__slow", json!({}), &cancel),
    )
    .await
    .expect("cancelling ends the call");
    assert!(out.is_error && out.text == "cancelled");
}

#[tokio::test]
async fn a_server_that_cannot_start_says_why_and_keeps_what_it_last_offered() {
    let fx = fx(vec![server("fx", &[], &[])]);
    let host = McpHost::new(fx.paths.clone());
    host.sync();
    settled(&host).await;
    assert_eq!(Cache::load(&fx.paths).servers["fx"].tools.len(), 6);

    // Now it fails to start: the same name, a changed command line.
    Config {
        prefix: true,
        servers: vec![server("fx", &["--die"], &[])],
    }
    .save(&fx.paths)
    .unwrap();
    host.sync();
    settled(&host).await;
    let Standing::Failed(why) = &host.status()[0].standing else {
        panic!("{:?}", host.status());
    };
    assert!(
        why.contains("refusing to start"),
        "the server's own words: {why}"
    );

    let cache = Cache::load(&fx.paths);
    assert_eq!(cache.servers["fx"].reach, Reach::Failed);
    assert!(
        cache.servers["fx"]
            .error
            .as_deref()
            .unwrap()
            .contains("refusing")
    );
    // Its tools stay listed -- as unavailable -- so what the user activated is not forgotten.
    assert_eq!(cache.servers["fx"].tools.len(), 6);
    let cat = load_catalogue(&fx.paths).unwrap();
    assert!(!cat.get("mcp__fx__echo").unwrap().mcp.as_ref().unwrap().up);
    // And nothing is advertised, nor does a call pretend.
    assert!(host.specs(&ids(&["mcp__fx__echo"])).is_empty());
    let out = host
        .call(
            "mcp__fx__echo",
            json!({"text": "x"}),
            &CancellationToken::new(),
        )
        .await;
    assert!(out.is_error);
}

#[tokio::test]
async fn a_command_that_does_not_exist_is_a_failure_not_a_hang() {
    let mut s = server("ghost", &[], &[]);
    s.transport = Transport::Stdio {
        command: "/nonexistent/reactor-mcp-server".into(),
        args: vec![],
        env: BTreeMap::new(),
    };
    let fx = fx(vec![s]);
    let host = McpHost::new(fx.paths.clone());
    host.sync();
    settled(&host).await;
    let Standing::Failed(why) = &host.status()[0].standing else {
        panic!("{:?}", host.status());
    };
    assert!(why.contains("cannot start"), "{why}");
}

#[tokio::test]
async fn sync_follows_the_config_servers_come_and_go_and_switch_off() {
    let fx = fx(vec![server("one", &[], &[]), server("two", &[], &[])]);
    let host = McpHost::new(fx.paths.clone());
    let changes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c = changes.clone();
    host.on_change(move || {
        c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    host.sync();
    settled(&host).await;
    assert_eq!(host.status().len(), 2);
    assert!(
        changes.load(std::sync::atomic::Ordering::SeqCst) >= 2,
        "a frontend is told"
    );

    let mut config = Config::load(&fx.paths).unwrap();
    config.remove("one");
    let mut two = config.get("two").unwrap().clone();
    two.enabled = false;
    config.put(two);
    config.put(server("three", &[], &[]));
    config.save(&fx.paths).unwrap();
    host.sync();
    settled(&host).await;
    let names: Vec<_> = host
        .status()
        .into_iter()
        .map(|s| (s.name, s.standing))
        .collect();
    assert_eq!(
        names,
        [
            ("three".to_string(), Standing::Connected),
            ("two".to_string(), Standing::Off)
        ]
    );
    // A server that is off lists nothing; one that is gone has left the cache.
    let cat = load_catalogue(&fx.paths).unwrap();
    assert!(
        cat.ids().iter().all(|i| i.starts_with("mcp__three__")),
        "{:?}",
        cat.ids()
    );
    assert!(!Cache::load(&fx.paths).servers.contains_key("one"));
}

// -- in the loop -----------------------------------------------------------------------------

struct Quiet;
impl Summarizer for Quiet {
    async fn summarize(&self, _: SummaryRequest) -> reactor_agent::Result<String> {
        Ok("-".into())
    }
}

#[tokio::test]
async fn the_model_is_given_the_active_definitions_and_may_call_one() {
    let fx = fx(vec![server("fx", &[], &[])]);
    let host = McpHost::new(fx.paths.clone());
    host.sync();
    settled(&host).await;

    let work = fx._dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let case = reactor_db::Case::create(&work, None).unwrap();
    let store = Store::create(&case, "s").unwrap();
    let mut cfg = AgentConfig::new(work.clone(), fx.paths.clone(), 100_000);
    let view = RegistryView {
        block: "## Available tools (this machine)".into(),
        // `fail` is active too, but the model only calls `echo` -- and `env` is called though
        // it is not active, which is allowed (ADR-0007).
        mcp: ids(&["mcp__fx__echo", "mcp__fx__fail"]),
        ..Default::default()
    };
    cfg.registry = Arc::new(move || view.clone());
    cfg.mcp = Some(host.clone());
    let llm = Arc::new(ScriptedLlm::new(vec![
        ScriptedLlm::call(
            "",
            &[
                ("c1", "mcp__fx__echo", json!({"text": "ping"})),
                ("c2", "mcp__fx__env", json!({"name": "HOME"})),
            ],
        ),
        ScriptedLlm::say("done"),
    ]));
    let (agent, _events) = Agent::new(
        llm.clone(),
        Arc::new(Quiet),
        store,
        Tools::standard(work),
        cfg,
    );
    agent.prompt("go", CancellationToken::new()).await.unwrap();

    let requests = llm.requests();
    let names =
        |i: usize| -> Vec<String> { requests[i].tools.iter().map(|t| t.name.clone()).collect() };
    assert!(
        names(0).contains(&"bash".to_string()),
        "ours are still there"
    );
    assert!(names(0).contains(&"mcp__fx__echo".to_string()));
    assert!(names(0).contains(&"mcp__fx__fail".to_string()));
    assert!(
        !names(0).contains(&"mcp__fx__env".to_string()),
        "inactive: not advertised"
    );

    let results: Vec<_> = requests[1]
        .messages
        .iter()
        .filter_map(|m| match m {
            reactor_agent::context::Msg::ToolResult {
                name,
                content,
                is_error,
                ..
            } => Some((name.clone(), content.clone(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(
        results[0],
        (
            "mcp__fx__echo".into(),
            "echo: ping\n\n[session:s#entry=3]".into(),
            false
        )
    );
    assert_eq!(results[1].0, "mcp__fx__env");
    assert!(!results[1].2, "an inactive tool still runs");
}

// -- the prefix setting -------------------------------------------------------

#[tokio::test]
async fn names_follow_the_prefix_setting_and_the_catalogue_agrees_with_the_host() {
    let fx = fx(vec![server("fx", &[], &[])]);
    // The fixture's `env` and `image` are not taken; make `echo` taken by a catalogue tool.
    std::fs::write(
        fx.paths.live("tools.toml"),
        "version = 1\n[tool.echo]\nname=\"Echo\"\ndesc=\"d\"\ninvoke=\"echo\"\ndetect={binary=\"echo\"}\n",
    )
    .unwrap();
    let mut config = Config::load(&fx.paths).unwrap();
    config.prefix = false;
    config.save(&fx.paths).unwrap();
    let host = McpHost::new(fx.paths.clone());
    host.sync();
    settled(&host).await;

    let cat = load_catalogue(&fx.paths).unwrap();
    // Ours, its own name where free, the prefixed one where `tools.toml` has the name.
    assert!(cat.get("mcp__fx__echo").is_some() && cat.get("fail").is_some());
    let all: BTreeSet<String> = cat.ids().into_iter().collect();
    let specs = host.specs(&all);
    let names: BTreeSet<String> = specs.iter().map(|s| s.name.clone()).collect();
    assert_eq!(names.len(), 6, "{names:?}");
    for t in cat.tools.iter().filter(|t| t.mcp.is_some()) {
        assert!(
            names.contains(&t.id),
            "the host does not know `{}`: {names:?}",
            t.id
        );
    }
    // A call goes by the name the model was given.
    let out = host
        .call("fail", json!({}), &CancellationToken::new())
        .await;
    assert_eq!(out.text, "it broke");
    let out = host
        .call(
            "mcp__fx__echo",
            json!({"text": "x"}),
            &CancellationToken::new(),
        )
        .await;
    assert_eq!(out.text, "echo: x");

    // Switching the prefix on renames everything, in the host and the catalogue alike.
    let mut config = Config::load(&fx.paths).unwrap();
    config.prefix = true;
    config.save(&fx.paths).unwrap();
    host.sync();
    let cat = load_catalogue(&fx.paths).unwrap();
    assert!(cat.get("fail").is_none() && cat.get("mcp__fx__fail").is_some());
    let all: BTreeSet<String> = cat.ids().into_iter().collect();
    assert!(
        host.specs(&all)
            .iter()
            .all(|s| s.name.starts_with("mcp__fx__"))
    );
    assert!(!host.knows("fail") && host.knows("mcp__fx__fail"));
}

#[test]
fn the_names_an_mcp_tool_may_not_take_are_the_agents_own() {
    let mut ours: Vec<String> = Tools::standard(PathBuf::from("."))
        .specs(&reactor_agent::context::SessionState::default())
        .into_iter()
        .map(|s| s.name)
        .collect();
    // Gated tools are advertised only by state, so `specs` may omit them: name the ones that
    // might be, and check they are real by asking `Tools::find`.
    let tools = Tools::standard(PathBuf::from("."));
    for gated in [
        "update_steps",
        "reactor_phase_complete",
        "scenario_amend",
        "graph_write",
        "tool_search",
    ] {
        assert!(
            tools.find(gated).is_some(),
            "{gated} is no longer one of ours"
        );
        ours.push(gated.to_string());
    }
    for name in ours {
        assert!(
            reactor_core::mcp::RESERVED.contains(&name.as_str()),
            "{name} is not reserved"
        );
    }
}

// -- what the request is made of ---------------------------------------------------------

#[tokio::test]
async fn the_usage_by_kind_tells_mcp_tools_apart_and_splits_what_the_agent_said() {
    use reactor_agent::entry::Block;
    use reactor_agent::inspect::Kind;
    use reactor_agent::llm::Reply;

    let fx = fx(vec![server("fx", &[], &[])]);
    let host = McpHost::new(fx.paths.clone());
    host.sync();
    settled(&host).await;

    let work = fx._dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let case = reactor_db::Case::create(&work, None).unwrap();
    let store = Store::create(&case, "s").unwrap();
    let mut cfg = AgentConfig::new(work.clone(), fx.paths.clone(), 100_000);
    let view = RegistryView {
        block: "## Available tools (this machine)".into(),
        mcp: ids(&["mcp__fx__echo", "mcp__fx__fail"]),
        ..Default::default()
    };
    cfg.registry = Arc::new(move || view.clone());
    cfg.mcp = Some(host.clone());
    let llm = Arc::new(ScriptedLlm::new(vec![
        Ok(Reply {
            blocks: vec![
                Block::Thinking {
                    text: "t".repeat(400),
                    signature: None,
                },
                Block::Text {
                    text: "r".repeat(40),
                },
                Block::ToolCall {
                    id: "c1".into(),
                    name: "mcp__fx__echo".into(),
                    arguments: json!({"text": "ping"}),
                },
            ],
            usage: None,
            stop: Some("tool_use".into()),
        }),
        ScriptedLlm::say("done"),
    ]));
    let (agent, _events) = Agent::new(
        llm.clone(),
        Arc::new(Quiet),
        store,
        Tools::standard(work),
        cfg,
    );
    agent.prompt("go", CancellationToken::new()).await.unwrap();

    let preview = agent.context_preview().await.unwrap();
    let kinds: std::collections::HashMap<Kind, u64> = preview.by_kind().into_iter().collect();
    assert!(
        kinds[&Kind::Mcp] > 0 && kinds[&Kind::Tools] > 0,
        "{kinds:?}"
    );
    // The two MCP tools are the ones in that kind, and only they.
    let mcp_segments: Vec<_> = preview
        .segments
        .iter()
        .filter(|s| s.origin == reactor_agent::inspect::Origin::McpDefinition)
        .map(|s| s.label.as_str())
        .collect();
    assert_eq!(mcp_segments, ["mcp__fx__echo", "mcp__fx__fail"]);
    // The assistant's message is split, thinking apart from the reply and the call.
    assert!(kinds[&Kind::Thinking] >= 100, "{kinds:?}");
    assert!(kinds[&Kind::ToolCalls] > 0 && kinds[&Kind::Reply] > 0);
    // The kinds add up to the request, to within the rounding of each system part.
    let sum: u64 = kinds.values().sum();
    assert!(
        sum.abs_diff(preview.total_tokens) <= 8,
        "{kinds:?} vs {}",
        preview.total_tokens
    );
}

// -- loaded on demand (ADR-0055) ------------------------------------------------------------

#[tokio::test]
async fn a_servers_tools_are_left_for_tool_search_when_they_cost_more_than_the_threshold() {
    let fx = fx(vec![server("fx", &[], &[])]);
    let host = McpHost::new(fx.paths.clone());
    host.sync();
    settled(&host).await;
    std::fs::write(
        fx.paths.config_dir.join("settings.json"),
        r#"{"toolSearch":{"deferOverTokens":0}}"#,
    )
    .unwrap();

    let work = fx._dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let case = reactor_db::Case::create(&work, None).unwrap();
    let store = Store::create(&case, "s").unwrap();
    let mut cfg = AgentConfig::new(work.clone(), fx.paths.clone(), 100_000);
    let view = RegistryView {
        block: "## Available tools (this machine)".into(),
        mcp: ids(&["mcp__fx__echo", "mcp__fx__fail"]),
        ..Default::default()
    };
    cfg.registry = Arc::new(move || view.clone());
    cfg.mcp = Some(host.clone());
    let llm = Arc::new(ScriptedLlm::new(vec![
        ScriptedLlm::call(
            "",
            &[(
                "t1",
                "tool_search",
                json!({"query": "select:mcp__fx__echo"}),
            )],
        ),
        ScriptedLlm::call("", &[("c1", "mcp__fx__echo", json!({"text": "ping"}))]),
        ScriptedLlm::say("done"),
    ]));
    let (agent, _events) = Agent::new(
        llm.clone(),
        Arc::new(Quiet),
        store,
        Tools::standard(work),
        cfg,
    );
    agent.prompt("go", CancellationToken::new()).await.unwrap();

    let names = |i: usize| -> Vec<String> {
        llm.requests()[i]
            .tools
            .iter()
            .map(|t| t.name.clone())
            .collect()
    };
    // The first request carries neither definition, and says what the search tool can load.
    assert!(!names(0).contains(&"mcp__fx__echo".to_string()));
    let search = llm.requests()[0]
        .tools
        .iter()
        .find(|t| t.name == "tool_search")
        .unwrap()
        .description
        .clone();
    assert!(search.contains("mcp__fx__echo") && search.contains("mcp__fx__fail"));
    // After the load, the one that was asked for is there, and the other still is not.
    assert!(names(1).contains(&"mcp__fx__echo".to_string()));
    assert!(!names(1).contains(&"mcp__fx__fail".to_string()));
    // The inspector shows what the request carries: the loaded definition, as MCP's.
    let preview = agent.context_preview().await.unwrap();
    let mcp: Vec<_> = preview
        .segments
        .iter()
        .filter(|s| s.origin == reactor_agent::inspect::Origin::McpDefinition)
        .map(|s| s.label.as_str())
        .collect();
    assert_eq!(mcp, ["mcp__fx__echo"]);
}
