//! MCP servers in the catalogue (ADR-0048): the config file, the cache the agent writes, and
//! what the two make of a tool -- listed, tagged, activated and put into toolsets like any other.

mod common;

use std::collections::BTreeMap;

use common::*;
use reactor_core::catalogue::{DetectKind, load_catalogue, load_toolsets};
use reactor_core::commands::{
    DoctorFlags, ProbeFlags, ToolsListFlags, doctor, registry, tools_list,
};
use reactor_core::mcp::{
    self, Cache, CachedServer, CachedTool, Config, Reach, Server, Transport, expand, tool_id,
};
use reactor_core::probe::Status;
use reactor_core::state::{active_ids, load_state, toolset_members};
use serde_json::json;

fn stdio(name: &str, tags: &[&str]) -> Server {
    Server {
        name: name.into(),
        transport: Transport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "some-server".into()],
            env: BTreeMap::from([("TOKEN".into(), "${SOME_TOKEN}".into())]),
        },
        tags: tags.iter().map(|t| t.to_string()).collect(),
        enabled: true,
    }
}

fn found(reach: Reach, tools: &[(&str, &str, u64)]) -> CachedServer {
    CachedServer {
        reach,
        error: None,
        tools: tools
            .iter()
            .map(|(name, desc, tokens)| CachedTool {
                name: name.to_string(),
                desc: desc.to_string(),
                tokens: *tokens,
            })
            .collect(),
    }
}

/// A fixture with one server, `docs`, that was reached and has two tools.
fn with_server(tags: &[&str]) -> Fx {
    let fx = Fx::new();
    Config {
        prefix: true,
        servers: vec![stdio("docs", tags)],
    }
    .save(&fx.paths)
    .unwrap();
    let mut cache = Cache::default();
    cache.servers.insert(
        "docs".into(),
        found(
            Reach::Connected,
            &[
                ("search", "Search the docs", 180),
                ("fetch page", "Fetch one page", 95),
            ],
        ),
    );
    cache.save(&fx.paths).unwrap();
    fx
}

// -- the config file --------------------------------------------------------

#[test]
fn the_config_round_trips_in_the_mcp_servers_shape() {
    let fx = Fx::new();
    let config = Config {
        prefix: true,
        servers: vec![
            stdio("docs", &["research"]),
            Server {
                name: "remote".into(),
                transport: Transport::Http {
                    url: "https://example.invalid/mcp".into(),
                    headers: BTreeMap::from([("Authorization".into(), "Bearer ${T}".into())]),
                },
                tags: vec![],
                enabled: false,
            },
        ],
    };
    config.save(&fx.paths).unwrap();
    assert_eq!(Config::load(&fx.paths).unwrap(), config);

    let text = std::fs::read_to_string(fx.dir.path().join("mcp.json")).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(doc["mcpServers"]["docs"]["command"], "npx");
    assert_eq!(
        doc["mcpServers"]["remote"]["url"],
        "https://example.invalid/mcp"
    );
    assert_eq!(doc["mcpServers"]["remote"]["enabled"], false);
}

#[test]
fn no_file_is_no_servers() {
    assert_eq!(Config::load(&Fx::new().paths).unwrap(), Config::default());
}

#[test]
fn a_file_that_cannot_be_read_is_an_error_not_an_empty_list() {
    // Saving from an empty list would overwrite what the user wrote by hand.
    let fx = Fx::new();
    let write = |text: &str| std::fs::write(fx.dir.path().join("mcp.json"), text).unwrap();
    for bad in [
        "{ not json",
        r#"{"mcpServers": []}"#,
        r#"{"mcpServers": {"a": {}}}"#,
        r#"{"mcpServers": {"a": {"command": "x", "url": "https://x"}}}"#,
        r#"{"mcpServers": {"a_b": {"command": "x"}}}"#,
        r#"{"mcpServers": {"a": {"command": "x", "args": [1]}}}"#,
        r#"{"mcpServers": {"a": {"command": "x", "tags": ["two words"]}}}"#,
    ] {
        write(bad);
        assert!(Config::load(&fx.paths).is_err(), "{bad}");
    }
}

#[test]
fn environment_references_are_expanded() {
    // SAFETY: a variable only this test reads.
    unsafe { std::env::set_var("REACTOR_MCP_TEST_VAR", "s3cret") };
    assert_eq!(expand("Bearer ${REACTOR_MCP_TEST_VAR}!"), "Bearer s3cret!");
    assert_eq!(expand("${REACTOR_MCP_TEST_UNSET}"), "");
    assert_eq!(expand("no refs"), "no refs");
    assert_eq!(expand("open ${ never closed"), "open ${ never closed");
}

#[test]
fn an_id_is_the_name_a_model_api_accepts() {
    assert_eq!(tool_id("docs", "search"), "mcp__docs__search");
    assert_eq!(tool_id("docs", "fetch page.v2"), "mcp__docs__fetch_page_v2");
    assert!(tool_id("docs", &"x".repeat(200)).len() <= 64);
    assert!(mcp::valid_name("docs-2"));
    for bad in ["", "-x", "a_b", "a b", "a.b", &"x".repeat(25)] {
        assert!(!mcp::valid_name(bad), "{bad}");
    }
}

// -- in the catalogue -------------------------------------------------------

#[test]
fn tools_of_a_reached_server_join_the_catalogue_after_everything_else() {
    let fx = with_server(&["research"]);
    let cat = load_catalogue(&fx.paths).unwrap();
    assert_eq!(
        cat.ids(),
        [
            "alpha",
            "beta",
            "gamma",
            "mcp__docs__search",
            "mcp__docs__fetch_page"
        ]
    );
    let t = cat.get("mcp__docs__search").unwrap();
    assert_eq!(t.detect_kind, DetectKind::Mcp);
    assert_eq!(t.tags, ["mcp", "docs", "research"]);
    let m = t.mcp.as_ref().unwrap();
    assert_eq!(
        (m.server.as_str(), m.tool.as_str(), m.tokens),
        ("docs", "search", 180)
    );
    // The server's own name for a tool survives the id's clean-up.
    assert_eq!(
        cat.get("mcp__docs__fetch_page")
            .unwrap()
            .mcp
            .as_ref()
            .unwrap()
            .tool,
        "fetch page"
    );
}

#[test]
fn a_server_not_yet_reached_or_switched_off_lists_nothing() {
    let fx = with_server(&[]);
    let mut config = Config::load(&fx.paths).unwrap();
    config.put(stdio("new", &[]));
    let mut docs = config.get("docs").unwrap().clone();
    docs.enabled = false;
    config.put(docs);
    config.save(&fx.paths).unwrap();
    assert_eq!(
        load_catalogue(&fx.paths).unwrap().ids(),
        ["alpha", "beta", "gamma"]
    );
}

#[test]
fn a_config_that_cannot_be_read_does_not_take_the_catalogue_down() {
    let fx = with_server(&[]);
    std::fs::write(fx.dir.path().join("mcp.json"), "{ not json").unwrap();
    assert_eq!(
        load_catalogue(&fx.paths).unwrap().ids(),
        ["alpha", "beta", "gamma"]
    );
}

#[test]
fn nothing_is_active_until_asked_for_and_all_is_everything() {
    let fx = with_server(&[]);
    let cat = load_catalogue(&fx.paths).unwrap();
    let sets = load_toolsets(&fx.paths).unwrap();

    let none = fx.state(&[], &[], &[]);
    assert!(active_ids(&cat, &sets, &none).is_empty());

    let all = fx.state(&["all"], &[], &[]);
    assert_eq!(active_ids(&cat, &sets, &all).len(), 5);

    let one = fx.state(&[], &["mcp__docs__search"], &[]);
    assert_eq!(
        active_ids(&cat, &sets, &one)
            .into_iter()
            .collect::<Vec<_>>(),
        ["mcp__docs__search"]
    );
}

#[test]
fn a_toolset_picks_a_servers_tools_up_by_tag() {
    let toolsets = format!(
        "{FIXTURE_TOOLSETS}\n[toolset.research]\ndesc = \"docs\"\ntags = [\"research\"]\n\n\
         [toolset.every-mcp]\ndesc = \"all mcp\"\ntags = [\"mcp\"]\n"
    );
    let fx = Fx::with(FIXTURE_TOOLS, &toolsets);
    Config {
        prefix: true,
        servers: vec![stdio("docs", &["research"]), stdio("other", &[])],
    }
    .save(&fx.paths)
    .unwrap();
    let mut cache = Cache::default();
    cache.servers.insert(
        "docs".into(),
        found(Reach::Connected, &[("search", "s", 10)]),
    );
    cache
        .servers
        .insert("other".into(), found(Reach::Connected, &[("ping", "p", 5)]));
    cache.save(&fx.paths).unwrap();

    let cat = load_catalogue(&fx.paths).unwrap();
    let sets = load_toolsets(&fx.paths).unwrap();
    let ids = |toolset: &str| {
        active_ids(&cat, &sets, &fx.state(&[toolset], &[], &[]))
            .into_iter()
            .collect::<Vec<_>>()
    };
    assert_eq!(ids("research"), ["mcp__docs__search"]);
    assert_eq!(ids("every-mcp"), ["mcp__docs__search", "mcp__other__ping"]);
}

#[test]
fn an_unreachable_server_s_tools_are_absent_and_a_reached_one_s_present() {
    let fx = with_server(&[]);
    let mut cache = Cache::load(&fx.paths);
    cache
        .servers
        .insert("docs".into(), found(Reach::Failed, &[("search", "s", 1)]));
    cache.save(&fx.paths).unwrap();

    let status = |fx: &Fx| {
        tools_list(
            &fx.paths,
            &ToolsListFlags {
                probe: ProbeFlags {
                    cached: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap()
        .report
        .tools
        .into_iter()
        .find(|t| t.id == "mcp__docs__search")
        .unwrap()
        .status
    };
    assert_eq!(status(&fx), Status::Absent);

    cache.servers.insert(
        "docs".into(),
        found(Reach::Connected, &[("search", "s", 1)]),
    );
    cache.save(&fx.paths).unwrap();
    assert_eq!(status(&fx), Status::Present);
}

#[test]
fn the_entry_says_what_the_tool_costs() {
    let fx = with_server(&[]);
    let report = tools_list(&fx.paths, &ToolsListFlags::default())
        .unwrap()
        .report;
    let t = report
        .tools
        .iter()
        .find(|t| t.id == "mcp__docs__search")
        .unwrap();
    let m = t.mcp.as_ref().unwrap();
    assert_eq!((m.server.as_str(), m.tokens), ("docs", 180));
    // A catalogue tool has no such field at all: the JSON is unchanged for it.
    let json = serde_json::to_value(&report.tools).unwrap();
    assert_eq!(json[0].get("mcp"), None);
    assert_eq!(json[3]["mcp"], json!({"server": "docs", "tokens": 180}));
}

#[test]
fn the_registry_block_never_lists_an_mcp_tool() {
    // Its definition goes to the model as a tool; a line in the prompt would say it twice.
    let fx = with_server(&[]);
    fx.state(&["all"], &[], &[]).save().unwrap();
    let report = registry(
        &fx.paths,
        ProbeFlags {
            cached: true,
            ..Default::default()
        },
    )
    .unwrap()
    .report;
    assert!(
        report
            .tools
            .iter()
            .any(|t| t.id == "mcp__docs__search" && t.active)
    );
    assert!(!report.block.contains("mcp__"), "{}", report.block);
    assert!(!report.block.contains("Search the docs"));
}

#[test]
fn doctor_does_not_diagnose_an_mcp_tool_as_a_missing_program() {
    let fx = with_server(&[]);
    let mut cache = Cache::load(&fx.paths);
    cache
        .servers
        .insert("docs".into(), found(Reach::Failed, &[("search", "s", 1)]));
    cache.save(&fx.paths).unwrap();
    let report = doctor(
        &fx.paths,
        DoctorFlags {
            cached: true,
            ..Default::default()
        },
    )
    .unwrap()
    .report;
    assert!(
        report.tools.iter().all(|t| t.mcp.is_none()),
        "{:?}",
        report.tools.len()
    );
    let _ = load_state(&fx.paths);
}

#[test]
fn each_server_has_a_toolset_for_all_its_tools_at_once() {
    let fx = with_server(&["research"]);
    let mut config = Config::load(&fx.paths).unwrap();
    config.put(stdio("other", &[]));
    config.save(&fx.paths).unwrap();
    let mut cache = Cache::load(&fx.paths);
    cache
        .servers
        .insert("other".into(), found(Reach::Connected, &[("ping", "p", 5)]));
    cache.save(&fx.paths).unwrap();

    let cat = load_catalogue(&fx.paths).unwrap();
    let sets = load_toolsets(&fx.paths).unwrap();
    let ids: Vec<&str> = sets.iter().map(|s| s.id.as_str()).collect();
    assert!(
        ids.contains(&"mcp-docs") && ids.contains(&"mcp-other"),
        "{ids:?}"
    );

    let docs = sets.iter().find(|s| s.id == "mcp-docs").unwrap();
    assert_eq!(docs.tags, ["mcp", "docs"]);
    assert!(docs.desc.contains("275 tokens"), "{}", docs.desc);
    assert_eq!(
        toolset_members(docs, &cat),
        ["mcp__docs__fetch_page", "mcp__docs__search"]
    );
    // Activating it activates those tools and no others.
    assert_eq!(
        active_ids(&cat, &sets, &fx.state(&["mcp-docs"], &[], &[]))
            .into_iter()
            .collect::<Vec<_>>(),
        ["mcp__docs__fetch_page", "mcp__docs__search"]
    );
}

#[test]
fn a_server_without_tools_or_switched_off_has_no_toolset_and_the_users_own_wins() {
    let toolsets =
        format!("{FIXTURE_TOOLSETS}\n[toolset.mcp-docs]\ndesc = \"mine\"\ntools = [\"alpha\"]\n");
    let fx = Fx::with(FIXTURE_TOOLS, &toolsets);
    let mut off = stdio("off", &[]);
    off.enabled = false;
    Config {
        prefix: true,
        servers: vec![stdio("docs", &[]), stdio("new", &[]), off],
    }
    .save(&fx.paths)
    .unwrap();
    let mut cache = Cache::default();
    cache.servers.insert(
        "docs".into(),
        found(Reach::Connected, &[("search", "s", 1)]),
    );
    cache
        .servers
        .insert("off".into(), found(Reach::Connected, &[("x", "x", 1)]));
    cache.save(&fx.paths).unwrap();

    let sets = load_toolsets(&fx.paths).unwrap();
    assert!(!sets.iter().any(|s| s.id == "mcp-new" || s.id == "mcp-off"));
    let docs: Vec<_> = sets.iter().filter(|s| s.id == "mcp-docs").collect();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].desc, "mine");
}

// -- the prefix setting -------------------------------------------------------

type Offered<'a> = &'a [(&'a str, &'a str, u64)];

fn unprefixed(servers: Vec<Server>, cache: &[(&str, Offered)]) -> Fx {
    let fx = Fx::new();
    Config {
        servers,
        prefix: false,
    }
    .save(&fx.paths)
    .unwrap();
    let mut c = Cache::default();
    for (name, tools) in cache {
        c.servers
            .insert(name.to_string(), found(Reach::Connected, tools));
    }
    c.save(&fx.paths).unwrap();
    fx
}

#[test]
fn without_the_prefix_a_tool_is_called_by_its_own_name_and_that_is_the_default() {
    let fx = unprefixed(
        vec![stdio("docs", &[])],
        &[("docs", &[("search", "s", 1), ("fetch page", "f", 1)])],
    );
    assert!(!Config::load(&fx.paths).unwrap().prefix);
    let cat = load_catalogue(&fx.paths).unwrap();
    assert_eq!(
        cat.ids(),
        ["alpha", "beta", "gamma", "search", "fetch_page"]
    );
    // Tags still say where it comes from.
    assert_eq!(cat.get("search").unwrap().tags, ["mcp", "docs"]);
    assert_eq!(
        cat.get("fetch_page").unwrap().mcp.as_ref().unwrap().tool,
        "fetch page"
    );
}

#[test]
fn the_setting_round_trips_and_is_only_written_when_on() {
    let fx = Fx::new();
    let mut config = Config {
        servers: vec![stdio("docs", &[])],
        prefix: false,
    };
    config.save(&fx.paths).unwrap();
    let text = std::fs::read_to_string(fx.dir.path().join("mcp.json")).unwrap();
    assert!(!text.contains("prefixToolNames"), "{text}");
    config.prefix = true;
    config.save(&fx.paths).unwrap();
    assert!(Config::load(&fx.paths).unwrap().prefix);
    std::fs::write(
        fx.dir.path().join("mcp.json"),
        r#"{"prefixToolNames": "yes"}"#,
    )
    .unwrap();
    assert!(Config::load(&fx.paths).is_err());
}

#[test]
fn a_name_that_is_taken_keeps_its_prefix_so_nothing_is_shadowed() {
    // `alpha` is a tool of tools.toml, `bash` is one of the agent's own, and the second
    // `search` is another server's.
    let fx = unprefixed(
        vec![stdio("one", &[]), stdio("two", &[])],
        &[
            (
                "one",
                &[("alpha", "a", 1), ("bash", "b", 1), ("search", "s", 1)],
            ),
            ("two", &[("search", "s", 1), ("own", "o", 1)]),
        ],
    );
    let ids = load_catalogue(&fx.paths).unwrap().ids();
    assert_eq!(
        ids[3..],
        [
            "mcp__one__alpha",
            "mcp__one__bash",
            "search",
            "mcp__two__search",
            "own"
        ]
    );
}

#[test]
fn toolsets_by_server_do_not_depend_on_the_prefix() {
    let fx = unprefixed(vec![stdio("docs", &[])], &[("docs", &[("search", "s", 1)])]);
    let cat = load_catalogue(&fx.paths).unwrap();
    let sets = load_toolsets(&fx.paths).unwrap();
    let docs = sets.iter().find(|s| s.id == "mcp-docs").unwrap();
    assert_eq!(toolset_members(docs, &cat), ["search"]);
}
