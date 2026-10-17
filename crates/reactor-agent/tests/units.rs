//! The smaller pieces: truncation, skills, prompt assembly, provider selection.

use std::collections::HashSet;

use reactor_agent::context::SessionState;
use reactor_agent::prompt::{self, Inputs, RegistryView};
use reactor_agent::provider::{AnyLlm, PROVIDERS, default_window};
use reactor_agent::skills::{self, Skill};
use reactor_agent::truncate::{
    self, CUT, HEAD_BYTES, Limit, MAX_INLINE_BYTES, MAX_REQUEST_BYTES, MIN_INLINE_BYTES,
    TAIL_BYTES, Whole,
};
use reactor_context::settings::Settings;
use serde_json::json;

// -- truncation ----------------------------------------------------------------------

#[test]
fn output_under_the_limit_is_not_cut() {
    let limit = Limit::default();
    assert!(!limit.needs_cut(MAX_INLINE_BYTES as u64));
    assert!(limit.needs_cut(MAX_INLINE_BYTES as u64 + 1));
}

#[test]
fn a_call_may_ask_for_its_own_limit_within_the_bounds() {
    let default = Limit::new(4096);
    assert_eq!(Limit::for_call(&json!({}), default), default);
    assert_eq!(
        Limit::for_call(&json!({"max_output_bytes": "lots"}), default),
        default
    );
    assert_eq!(
        Limit::for_call(&json!({"max_output_bytes": 100_000}), default).bytes(),
        100_000
    );
    assert_eq!(
        Limit::for_call(&json!({"max_output_bytes": 1}), default).bytes(),
        MIN_INLINE_BYTES
    );
    assert_eq!(
        Limit::for_call(&json!({"max_output_bytes": u64::MAX}), default).bytes(),
        MAX_REQUEST_BYTES
    );
    // The split keeps the default's shape: a quarter head, the rest tail.
    let l = Limit::new(100_000);
    assert_eq!((l.head(), l.tail()), (25_000, 75_000));
    assert_eq!(
        (Limit::default().head(), Limit::default().tail()),
        (HEAD_BYTES, TAIL_BYTES)
    );
}

#[test]
fn head_and_tail_end_on_line_boundaries_and_never_split_a_character() {
    let text: String = (0..5_000).map(|i| format!("line {i}\n")).collect();
    let h = truncate::head(&text, HEAD_BYTES);
    assert!(h.len() <= HEAD_BYTES && h.ends_with('\n'));
    let t = truncate::tail(&text, TAIL_BYTES);
    assert!(t.len() <= TAIL_BYTES && t.starts_with("line "));
    assert!(t.ends_with("line 4999\n"));

    // Multi-byte text: any cut lands on a char boundary (this would panic otherwise).
    let wide = "é😀日本語".repeat(20_000);
    let _ = truncate::head(&wide, 8_000);
    let _ = truncate::tail(&wide, 24_000);
    assert!(truncate::cut(&wide, Limit::default()).contains(CUT));
}

#[test]
fn the_tail_gets_the_larger_share() {
    // The end of a dump is where the error, the prompt and the last line are.
    const { assert!(TAIL_BYTES > HEAD_BYTES * 2) };
    assert_eq!(HEAD_BYTES + TAIL_BYTES, MAX_INLINE_BYTES);
}

#[test]
fn the_view_says_it_was_cut_and_gives_the_call_for_the_rest() {
    let text = "a".repeat(100_000);
    let whole = Whole {
        entry: 42,
        lines: 1,
    };
    let v = truncate::view(&text, whole, Limit::default());
    assert!(!v.contains(CUT), "the placeholder never reaches the model");
    let marker = "\n… [output truncated; the rest: history_read {\"id\": 42, \"offset\": 1, \"limit\": 1}] …\n";
    assert!(v.contains(marker), "{v:.200}");
    assert_eq!(v.len() - marker.len(), HEAD_BYTES + TAIL_BYTES);
}

#[test]
fn a_producer_that_cut_its_own_output_is_respected() {
    // `bash` spills huge output to disk and hands over head+CUT+tail.
    let composite = format!("HEAD{CUT}TAIL");
    let whole = Whole {
        entry: 7,
        lines: 500,
    };
    let v = truncate::view(&composite, whole, Limit::default());
    assert!(v.starts_with("HEAD") && v.ends_with("TAIL"));
    assert!(
        v.contains("output truncated") && v.contains("{\"id\": 7, \"offset\": 1, \"limit\": 500}")
    );
}

#[test]
fn the_marker_says_which_lines_are_missing_and_how_to_read_them() {
    let text: String = (1..=10_000).map(|i| format!("line {i}\n")).collect();
    let whole = Whole {
        entry: 9,
        lines: truncate::count_lines(text.as_bytes()),
    };
    assert_eq!(whole.lines, 10_000);
    let v = truncate::view(&text, whole, Limit::new(4096));
    let shown: Vec<&str> = v.lines().filter(|l| l.starts_with("line ")).collect();
    // The lines named as missing are exactly the ones the view skips.
    let marker = v.lines().find(|l| l.contains("truncated")).unwrap();
    let call = marker.split("history_read ").nth(1).unwrap();
    let call: serde_json::Value = serde_json::from_str(&call[..=call.find('}').unwrap()]).unwrap();
    assert_eq!(call["id"], 9);
    let (offset, limit) = (
        call["offset"].as_u64().unwrap(),
        call["limit"].as_u64().unwrap(),
    );
    let mut seen: Vec<u64> = shown.iter().map(|l| l[5..].parse().unwrap()).collect();
    seen.extend(offset..offset + limit);
    seen.sort();
    seen.dedup();
    assert_eq!(
        seen,
        (1..=10_000).collect::<Vec<_>>(),
        "no line is lost between the view and the call"
    );
}

// -- skills ----------------------------------------------------------------------------

fn skill_file(dir: &std::path::Path, name: &str, frontmatter: &str) -> std::path::PathBuf {
    let d = dir.join(name);
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("SKILL.md");
    std::fs::write(&f, format!("---\n{frontmatter}\n---\nbody\n")).unwrap();
    f
}

#[test]
fn a_skill_is_a_name_a_description_and_optional_requirements() {
    let dir = tempfile::tempdir().unwrap();
    let f = skill_file(
        dir.path(),
        "a",
        "name: \"quoted\"\ndescription: 'does things'\nrequires: [bindiff, bn]",
    );
    let s = skills::parse(&f).unwrap();
    assert_eq!(
        (s.name.as_str(), s.description.as_str()),
        ("quoted", "does things")
    );
    assert_eq!(s.requires, ["bindiff", "bn"]);
    assert_eq!(
        skills::parse(&skill_file(dir.path(), "b", "name: b\ndescription: d"))
            .unwrap()
            .requires,
        Vec::<String>::new()
    );
}

#[test]
fn a_skill_without_a_description_or_frontmatter_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        skills::parse(&skill_file(dir.path(), "a", "name: a")).is_none(),
        "nothing to choose it by"
    );
    assert!(skills::parse(&skill_file(dir.path(), "b", "description: d")).is_none());
    let f = dir.path().join("plain.md");
    std::fs::write(&f, "no frontmatter at all").unwrap();
    assert!(skills::parse(&f).is_none());
    assert!(skills::parse(&dir.path().join("missing/SKILL.md")).is_none());
}

#[test]
fn discovery_is_sorted_by_name_and_skips_what_is_not_a_skill() {
    let dir = tempfile::tempdir().unwrap();
    skill_file(dir.path(), "zeta", "name: zeta\ndescription: z");
    skill_file(dir.path(), "alpha", "name: alpha\ndescription: a");
    std::fs::create_dir_all(dir.path().join("empty")).unwrap();
    std::fs::write(dir.path().join("stray.md"), "x").unwrap();
    let found: Vec<String> = skills::discover(dir.path())
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(found, ["alpha", "zeta"]);
    assert!(skills::discover(std::path::Path::new("/definitely/not/here")).is_empty());
}

#[test]
fn authored_skills_need_every_required_tool_present_and_active() {
    let mk = |name: &str, requires: &[&str]| Skill {
        name: name.into(),
        description: format!("{name} desc"),
        requires: requires.iter().map(|s| s.to_string()).collect(),
        file: format!("/skills/{name}/SKILL.md").into(),
    };
    let authored = [
        mk("free", &[]),
        mk("both", &["a", "b"]),
        mk("half", &["a", "z"]),
    ];
    let usable: HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
    let names: Vec<String> = skills::offered(&authored, &usable, &[])
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, ["free", "both"]);
}

#[test]
fn the_skills_block_lists_name_description_and_path_or_is_absent() {
    assert_eq!(skills::block(&[]), None);
    let b = skills::block(&[Skill {
        name: "n".into(),
        description: "does n".into(),
        requires: vec![],
        file: "/s/n/SKILL.md".into(),
    }])
    .unwrap();
    assert!(b.starts_with("## Skills") && b.contains("- **n** -- does n (`/s/n/SKILL.md`)"));
}

// -- the system prompt --------------------------------------------------------------------

fn inputs<'a>(
    settings: &'a Settings,
    state: &'a SessionState,
    registry: &'a RegistryView,
    skills: &'a [Skill],
) -> Inputs<'a> {
    Inputs {
        base: "BASE",
        settings,
        state,
        registry,
        skills,
        scenarios_dir: std::path::Path::new("/nonexistent"),
        rules: &[],
        deferred: &[],
    }
}

#[test]
fn an_untouched_session_is_the_base_the_registry_and_the_case_graph_and_nothing_else() {
    let (settings, mut state) = (Settings::default(), SessionState::default());
    let registry = RegistryView {
        block: "## Available tools (this machine)\n\nsh  shell".into(),
        ..Default::default()
    };
    // Attached to the case's graph by default (ADR-0047): the block is static text, so
    // the prompt is still the same bytes from one request to the next.
    assert_eq!(
        prompt::system_prompt(&inputs(&settings, &state, &registry, &[])),
        format!(
            "BASE\n\n## Available tools (this machine)\n\nsh  shell\n\n{}",
            prompt::CASE_GRAPH
        )
    );
    // Detached, nothing about the graph is said at all.
    state.memory.case_graph = Some(false);
    assert_eq!(
        prompt::system_prompt(&inputs(&settings, &state, &registry, &[])),
        "BASE\n\n## Available tools (this machine)\n\nsh  shell"
    );
}

#[test]
fn the_blocks_come_in_a_fixed_order_so_an_unchanged_session_sends_the_same_bytes() {
    let mut settings = Settings::default();
    settings.reporting.folder = "notes".into();
    let mut state = SessionState::default();
    state.identity.active = Some("publisher".into());
    state.manifest.goal = Some("the goal".into());
    state.reporting.enabled = Some(true);
    let registry = RegistryView {
        block: "## Available tools (this machine)".into(),
        ..Default::default()
    };
    let skills = [Skill {
        name: "s".into(),
        description: "d".into(),
        requires: vec![],
        file: "/s".into(),
    }];

    let a = prompt::system_prompt(&inputs(&settings, &state, &registry, &skills));
    let b = prompt::system_prompt(&inputs(&settings, &state, &registry, &skills));
    assert_eq!(a, b);
    let at = |needle: &str| {
        a.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing from:\n{a}"))
    };
    assert!(
        at("BASE") < at("## Identity")
            && at("## Identity") < at("## Available tools")
            && at("## Available tools") < at("## Skills")
    );
    assert!(
        at("## Skills") < at("## Session Manifest")
            && at("## Session Manifest") < at("## Reporting mode")
    );
}

#[test]
fn the_default_base_prompt_is_short_and_leaves_context_and_permission_alone() {
    let base = prompt::DEFAULT_BASE;
    assert!(base.len() < 600, "{} bytes", base.len());
    // How a cut or dropped entry is read back is said where the cut is (the truncation
    // note, a reduction's stand-in) and by the history tools, not here.
    for word in ["history_", "context", "permission", "destructive"] {
        assert!(!base.contains(word), "the base prompt mentions {word}");
    }
}

// -- providers -------------------------------------------------------------------------------

#[test]
fn a_model_spec_must_name_a_known_provider() {
    let e = |s: &str| {
        AnyLlm::from_spec(s)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default()
    };
    assert!(
        e("claude-sonnet").contains("provider/name"),
        "{}",
        e("claude-sonnet")
    );
    assert!(e("nope/model").contains("unknown provider `nope`"));
    for p in PROVIDERS {
        assert!(e("nope/model").contains(p), "the error should list {p}");
    }
}

#[test]
fn windows_default_per_provider_and_ollama_is_conservative() {
    assert_eq!(default_window("anthropic"), 200_000);
    assert_eq!(default_window("gemini"), 1_000_000);
    assert_eq!(default_window("ollama"), 32_000);
    assert_eq!(default_window("anything-else"), 128_000);
}

// -- usage by kind ---------------------------------------------------------------------------

mod usage {
    use reactor_agent::inspect::{AssistantParts, ContextPreview, Kind, Origin, Section, Segment};

    fn seg(origin: Origin, tokens: u64, parts: Option<AssistantParts>) -> Segment {
        Segment {
            section: Section::Messages,
            origin,
            label: String::new(),
            text: String::new(),
            tokens,
            entry: None,
            parts,
        }
    }

    #[test]
    fn every_origin_belongs_to_a_kind_and_every_kind_has_somewhere_to_look() {
        for origin in Origin::ALL {
            assert!(
                Kind::ALL.iter().any(|k| k.origins().contains(&origin)),
                "{origin:?} is in no kind"
            );
        }
        for kind in Kind::ALL {
            assert!(!kind.origins().is_empty(), "{kind:?} shows nothing");
        }
    }

    #[test]
    fn kinds_come_in_the_bars_order_without_the_empty_ones() {
        let p = ContextPreview {
            segments: vec![
                seg(Origin::ToolResult, 50, None),
                seg(Origin::Base, 10, None),
                seg(Origin::McpDefinition, 7, None),
                seg(Origin::Identity, 5, None),
            ],
            total_tokens: 72,
            calibration: 1.0,
            warnings: vec![],
        };
        assert_eq!(
            p.by_kind(),
            [(Kind::System, 15), (Kind::Mcp, 7), (Kind::ToolResults, 50)]
        );
    }

    #[test]
    fn an_assistant_message_is_split_and_its_overhead_is_the_replys() {
        let parts = AssistantParts {
            reply: 10,
            thinking: 30,
            calls: 20,
        };
        // 8 tokens are the message's own: they are counted as reply, so the kinds add up.
        let p = ContextPreview {
            segments: vec![
                seg(Origin::Assistant, 68, Some(parts)),
                seg(Origin::Assistant, 9, None),
            ],
            total_tokens: 77,
            calibration: 1.0,
            warnings: vec![],
        };
        let kinds = p.by_kind();
        assert_eq!(
            kinds,
            [
                (Kind::Reply, 10 + 8 + 9),
                (Kind::Thinking, 30),
                (Kind::ToolCalls, 20)
            ]
        );
        assert_eq!(kinds.iter().map(|k| k.1).sum::<u64>(), 77);
    }
}

// -- tool rules and deferred groups (ADR-0055) ----------------------------------------------------

#[test]
fn the_rules_of_the_tools_in_the_request_come_after_the_registry_and_say_the_graph_is_not_loaded() {
    let (settings, state) = (Settings::default(), SessionState::default());
    let registry = RegistryView {
        block: "## Available tools (this machine)".into(),
        ..Default::default()
    };
    let rules = [
        "Use `read` to look at a file.".to_string(),
        "Second.".to_string(),
    ];
    let deferred = ["graph_add".to_string()];
    let mut i = inputs(&settings, &state, &registry, &[]);
    i.rules = &rules;
    i.deferred = &deferred;
    let p = prompt::system_prompt(&i);
    let at = |n: &str| {
        p.find(n)
            .unwrap_or_else(|| panic!("{n} missing from:\n{p}"))
    };
    assert!(at("## Available tools") < at("## Tool rules"));
    assert!(at("## Tool rules") < at("## Case graph"));
    assert!(p.contains("- Use `read` to look at a file.\n- Second."));
    assert!(p.contains("`tool_search` with `select:graph_add"));
    // With nothing deferred the graph block is the plain text.
    i.deferred = &[];
    assert!(!prompt::system_prompt(&i).contains("tool_search"));
}

mod deferral {
    use std::collections::BTreeSet;

    use reactor_agent::deferral::{self, Candidate, TOOL};
    use reactor_agent::inspect::Origin;
    use reactor_agent::llm::ToolSpec;
    use serde_json::json;

    fn tool(name: &str, description: &str, params: usize) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: description.into(),
            parameters: json!({"type": "object", "properties": {"p": {"description": "x".repeat(params)}}}),
        }
    }

    fn cand(group: Option<&str>, spec: ToolSpec) -> Candidate {
        Candidate {
            origin: Origin::ToolDefinition,
            group: group.map(str::to_string),
            spec,
        }
    }

    fn names(p: &deferral::Plan) -> Vec<&str> {
        p.advertised.iter().map(|(_, s)| s.name.as_str()).collect()
    }

    fn candidates() -> Vec<Candidate> {
        vec![
            cand(None, tool("bash", "Run it.", 10)),
            cand(Some("big"), tool("big_a", "Find files. More text.", 2000)),
            cand(Some("big"), tool("big_b", "Show a file.", 2000)),
            cand(Some("small"), tool("small_a", "Tiny.", 20)),
        ]
    }

    #[test]
    fn only_a_group_over_the_threshold_is_left_for_the_search_tool() {
        let p = deferral::plan(candidates(), &BTreeSet::new(), 1_000);
        assert_eq!(names(&p), ["bash", "small_a", TOOL]);
        let left: Vec<&str> = p.deferred.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(left, ["big_a", "big_b"]);
        let search = &p.advertised.last().unwrap().1;
        assert!(search.description.contains("- big_a: Find files"));
        assert!(
            !search.description.contains("More text"),
            "a first sentence each"
        );
    }

    #[test]
    fn a_group_that_fits_is_advertised_and_there_is_no_search_tool() {
        let p = deferral::plan(candidates(), &BTreeSet::new(), 100_000);
        assert_eq!(names(&p), ["bash", "big_a", "big_b", "small_a"]);
        assert!(p.deferred.is_empty());
    }

    #[test]
    fn zero_defers_every_group_but_never_a_tool_that_has_none() {
        let p = deferral::plan(candidates(), &BTreeSet::new(), 0);
        assert_eq!(names(&p), ["bash", TOOL]);
        assert_eq!(p.deferred.len(), 3);
    }

    #[test]
    fn a_loaded_tool_is_advertised_and_the_search_tool_does_not_change() {
        let none = deferral::plan(candidates(), &BTreeSet::new(), 1_000);
        let loaded: BTreeSet<String> = ["big_b".to_string()].into();
        let p = deferral::plan(candidates(), &loaded, 1_000);
        assert_eq!(names(&p), ["bash", "big_b", "small_a", TOOL]);
        assert_eq!(
            p.advertised.last().unwrap().1,
            none.advertised.last().unwrap().1,
            "the listing is of what exists, not of what is left"
        );
    }

    #[test]
    fn select_names_tools_and_words_rank_a_name_over_a_description() {
        let d = vec![
            tool("net_hosts", "List the hosts a capture talked to.", 5),
            tool("file_hash", "Hash a file. Names a host if it knows one.", 5),
            tool("zip_open", "Open an archive.", 5),
        ];
        let got = |q: &str| -> Vec<&str> {
            deferral::find(&d, q, 5)
                .into_iter()
                .map(|t| t.name.as_str())
                .collect()
        };
        assert_eq!(
            got("select:zip_open, FILE_HASH, nope"),
            ["zip_open", "file_hash"]
        );
        assert_eq!(got("hosts"), ["net_hosts", "file_hash"]);
        assert_eq!(got("hash"), ["file_hash"]);
        assert!(got("nothing like it").is_empty());
        assert_eq!(deferral::find(&d, "host", 1).len(), 1);
    }
}
