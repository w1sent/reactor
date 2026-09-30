//! The smaller pieces: truncation, skills, prompt assembly, provider selection.

use std::collections::HashSet;

use reactor_agent::context::SessionState;
use reactor_agent::prompt::{self, Inputs, RegistryView};
use reactor_agent::provider::{AnyLlm, PROVIDERS, default_window};
use reactor_agent::skills::{self, Skill};
use reactor_agent::truncate::{self, CUT, HEAD_BYTES, MAX_INLINE_BYTES, TAIL_BYTES};
use reactor_context::settings::Settings;

// -- truncation ----------------------------------------------------------------------

#[test]
fn output_under_the_limit_is_not_cut() {
    assert!(!truncate::needs_cut(MAX_INLINE_BYTES as u64));
    assert!(truncate::needs_cut(MAX_INLINE_BYTES as u64 + 1));
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
    assert!(truncate::cut(&wide).contains(CUT));
}

#[test]
fn the_tail_gets_the_larger_share() {
    // The end of a dump is where the error, the prompt and the last line are.
    const { assert!(TAIL_BYTES > HEAD_BYTES * 2) };
    assert_eq!(HEAD_BYTES + TAIL_BYTES, MAX_INLINE_BYTES);
}

#[test]
fn the_view_names_the_entry_and_counts_what_was_elided() {
    let text = "a".repeat(100_000);
    let v = truncate::view(&text, text.len() as u64, 42);
    assert!(v.contains("the whole output is #42"));
    assert!(v.contains("history_read") && v.contains("history_search"));
    assert!(!v.contains(CUT), "the placeholder never reaches the model");
    let elided: u64 = v.split("… [").nth(1).unwrap().split(" of ").next().unwrap().parse().unwrap();
    assert_eq!(elided + (HEAD_BYTES + TAIL_BYTES) as u64, 100_000);
}

#[test]
fn a_producer_that_cut_its_own_output_is_respected() {
    // `bash` spills huge output to disk and hands over head+CUT+tail.
    let composite = format!("HEAD{CUT}TAIL");
    let v = truncate::view(&composite, 10_000_000, 7);
    assert!(v.starts_with("HEAD") && v.ends_with("TAIL"));
    assert!(v.contains("of 10000000 bytes elided") && v.contains("#7"));
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
    let f = skill_file(dir.path(), "a", "name: \"quoted\"\ndescription: 'does things'\nrequires: [bindiff, bn]");
    let s = skills::parse(&f).unwrap();
    assert_eq!((s.name.as_str(), s.description.as_str()), ("quoted", "does things"));
    assert_eq!(s.requires, ["bindiff", "bn"]);
    assert_eq!(skills::parse(&skill_file(dir.path(), "b", "name: b\ndescription: d")).unwrap().requires, Vec::<String>::new());
}

#[test]
fn a_skill_without_a_description_or_frontmatter_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    assert!(skills::parse(&skill_file(dir.path(), "a", "name: a")).is_none(), "nothing to choose it by");
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
    let found: Vec<String> = skills::discover(dir.path()).into_iter().map(|s| s.name).collect();
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
    let authored = [mk("free", &[]), mk("both", &["a", "b"]), mk("half", &["a", "z"])];
    let usable: HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
    let names: Vec<String> = skills::offered(&authored, &usable, &[]).into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["free", "both"]);
}

#[test]
fn the_skills_block_lists_name_description_and_path_or_is_absent() {
    assert_eq!(skills::block(&[]), None);
    let b = skills::block(&[Skill { name: "n".into(), description: "does n".into(), requires: vec![], file: "/s/n/SKILL.md".into() }]).unwrap();
    assert!(b.starts_with("## Skills") && b.contains("- **n** -- does n (`/s/n/SKILL.md`)"));
}

// -- the system prompt --------------------------------------------------------------------

fn inputs<'a>(settings: &'a Settings, state: &'a SessionState, registry: &'a RegistryView, skills: &'a [Skill]) -> Inputs<'a> {
    Inputs { base: "BASE", settings, state, registry, skills, scenarios_dir: std::path::Path::new("/nonexistent") }
}

#[test]
fn an_untouched_session_is_the_base_and_the_registry_and_nothing_else() {
    let (settings, state) = (Settings::default(), SessionState::default());
    let registry = RegistryView { block: "## Available RE tools (this machine)\n\nsh  shell".into(), ..Default::default() };
    assert_eq!(prompt::system_prompt(&inputs(&settings, &state, &registry, &[])), "BASE\n\n## Available RE tools (this machine)\n\nsh  shell");
}

#[test]
fn the_blocks_come_in_a_fixed_order_so_an_unchanged_session_sends_the_same_bytes() {
    let mut settings = Settings::default();
    settings.reporting.folder = "notes".into();
    let mut state = SessionState::default();
    state.identity.active = Some("publisher".into());
    state.manifest.goal = Some("the goal".into());
    state.reporting.enabled = Some(true);
    let registry = RegistryView { block: "## Available RE tools (this machine)".into(), ..Default::default() };
    let skills = [Skill { name: "s".into(), description: "d".into(), requires: vec![], file: "/s".into() }];

    let a = prompt::system_prompt(&inputs(&settings, &state, &registry, &skills));
    let b = prompt::system_prompt(&inputs(&settings, &state, &registry, &skills));
    assert_eq!(a, b);
    let at = |needle: &str| a.find(needle).unwrap_or_else(|| panic!("{needle} missing from:\n{a}"));
    assert!(at("BASE") < at("## Identity") && at("## Identity") < at("## Available RE tools") && at("## Available RE tools") < at("## Skills"));
    assert!(at("## Skills") < at("## Session Manifest") && at("## Session Manifest") < at("## Reporting mode"));
}

#[test]
fn the_default_base_prompt_tells_the_agent_where_history_went() {
    // The context-management contract: cut and dropped material must be findable.
    for needle in ["history_read", "history_search", "history_index", "#id", "nothing is ever deleted"] {
        assert!(prompt::DEFAULT_BASE.contains(needle), "{needle}");
    }
}

// -- providers -------------------------------------------------------------------------------

#[test]
fn a_model_spec_must_name_a_known_provider() {
    let e = |s: &str| AnyLlm::from_spec(s).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(e("claude-sonnet").contains("provider/name"), "{}", e("claude-sonnet"));
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
