//! The pieces of `reactor-context` the golden replays do not isolate: JS string
//! arithmetic, the settings cascade, and the shipped scenarios against the
//! shipped toolsets.

use std::fs;
use std::path::Path;

use reactor_context::identity::{self, UserIdentities};
use reactor_context::manifest::{self, Step};
use reactor_context::reporting::{self, SessionState, Tracker};
use reactor_context::scenario;
use reactor_context::settings::{Enforcement, Origin, Settings, resolve};
use reactor_context::text::{js_len, js_prefix, truncate, truncate_words};
use reactor_core::Paths;
use reactor_core::catalogue::load_toolsets;
use reactor_core::paths::Shipped;
use serde_json::json;

// -- text ---------------------------------------------------------------------

#[test]
fn length_counts_utf16_units_like_javascript() {
    assert_eq!(js_len("abc"), 3);
    assert_eq!(js_len("é"), 1);
    assert_eq!(js_len("😀"), 2);
    assert_eq!(js_len("a😀b"), 4);
}

#[test]
fn truncate_keeps_whole_strings_and_ellipsizes_the_rest() {
    assert_eq!(truncate("short", 80), "short");
    assert_eq!(truncate("abcdef", 6), "abcdef");
    assert_eq!(truncate("abcdefg", 6), "abcde…");
    assert_eq!(
        truncate("abc", 0),
        "abc".chars().take(0).collect::<String>() + "…"
    );
    // Emoji count as two units, so this is over the limit at 5 chars.
    assert_eq!(truncate("ab😀cd", 5), "ab😀…");
}

#[test]
fn a_cut_that_would_split_a_surrogate_pair_stops_before_it() {
    // JS would keep the lone high surrogate; a Rust String cannot.
    assert_eq!(js_prefix("ab😀", 3), "ab");
    assert_eq!(js_prefix("ab😀", 4), "ab😀");
}

#[test]
fn truncate_words_takes_the_first_words_and_squeezes_whitespace() {
    assert_eq!(
        truncate_words("  one   two\tthree four ", 3),
        "one two three"
    );
    assert_eq!(truncate_words("one", 3), "one");
    assert_eq!(truncate_words("   ", 3), "");
    assert_eq!(truncate_words("", 3), "");
    assert_eq!(truncate_words("a b", 0), "");
}

// -- settings -----------------------------------------------------------------

#[test]
fn a_session_override_wins_and_absence_inherits() {
    assert_eq!(resolve(&1, Some(&2)).value, 2);
    assert_eq!(resolve(&1, Some(&2)).origin, Origin::Session);
    assert_eq!(resolve(&1, None).value, 1);
    assert_eq!(resolve(&1, None).origin, Origin::Default);
    // An override equal to the default is still an override: the scope is visible.
    assert_eq!(resolve(&1, Some(&1)).origin, Origin::Session);
}

#[test]
fn settings_default_to_what_the_extensions_defaulted_to() {
    let s = Settings::default();
    assert!(s.toolbox);
    assert!(s.hidden_services.is_empty());
    assert_eq!(
        (
            s.manifest.soft_step_limit,
            s.manifest.max_description,
            s.manifest.status_words
        ),
        (20, 80, 3)
    );
    assert_eq!(s.manifest.derive_context_chars, 24_000);
    assert_eq!(s.identity.default, "");
    assert_eq!(
        (s.reporting.level, s.reporting.folder.as_str()),
        (Enforcement::OFF, "report")
    );
    assert_eq!(
        (s.reporting.step_threshold, s.reporting.max_reverts),
        (8, 3)
    );
    assert_eq!(s.reporting.template_path, None);
}

#[test]
fn loading_is_forgiving_field_by_field() {
    let s = Settings::from_json(&json!({
        "toolbox": "yes",                      // wrong shape: default
        "hiddenServices": ["adb", 7, "bn"],    // the strings survive
        "manifest": { "softStepLimit": 5, "maxDescription": "wide", "statusWords": -1 },
        "identity": { "default": "publisher", "user": { "a": "A", "b": 3 } },
        "reporting": { "level": 2, "folder": "out", "stepThreshold": 0, "maxReverts": 0, "templatePath": "t.md" },
    }));
    assert!(s.toolbox);
    assert_eq!(s.hidden_services, ["adb", "bn"]);
    assert_eq!(s.manifest.soft_step_limit, 5);
    assert_eq!(s.manifest.max_description, 80);
    assert_eq!(s.manifest.status_words, 3);
    assert_eq!(s.identity.default, "publisher");
    assert_eq!(s.identity.user.names().collect::<Vec<_>>(), ["a"]);
    assert_eq!(s.reporting.level, Enforcement::STRICT);
    assert_eq!(s.reporting.folder, "out");
    assert_eq!(
        s.reporting.step_threshold, 8,
        "a zero threshold means nothing"
    );
    assert_eq!(s.reporting.max_reverts, 0, "zero reverts is a real choice");
    assert_eq!(s.reporting.template_path.as_deref(), Some("t.md"));
}

#[test]
fn a_file_that_is_not_json_or_not_an_object_means_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path(), Shipped::Embedded);
    assert_eq!(Settings::load(&paths), Settings::default());
    fs::write(Settings::path(&paths), "{ not json").unwrap();
    assert_eq!(Settings::load(&paths), Settings::default());
    fs::write(Settings::path(&paths), "[1, 2]").unwrap();
    assert_eq!(Settings::load(&paths), Settings::default());
}

#[test]
fn saving_round_trips_and_writes_sorted_keys() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path(), Shipped::Embedded);
    let mut s = Settings {
        hidden_services: vec!["adb".into()],
        ..Settings::default()
    };
    s.identity.user =
        UserIdentities::from_pairs([("zeta".into(), "Z".into()), ("alpha".into(), "A".into())]);
    s.save(&paths).unwrap();
    // JSON files are written with sorted keys, so saved identities come back in
    // alphabetical order rather than the order they were saved in.
    let mut sorted = s.clone();
    sorted.identity.user =
        UserIdentities::from_pairs([("alpha".into(), "A".into()), ("zeta".into(), "Z".into())]);
    assert_eq!(Settings::load(&paths), sorted);

    let text = fs::read_to_string(Settings::path(&paths)).unwrap();
    assert!(text.ends_with("}\n"));
    let top: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("  \""))
        .map(|l| l.trim().split('"').nth(1).unwrap())
        .collect();
    assert_eq!(
        top,
        [
            "context",
            "defaultModel",
            "hiddenServices",
            "identity",
            "manifest",
            "models",
            "reporting",
            "toolbox"
        ],
        "keys are sorted"
    );
}

#[test]
fn patching_one_key_keeps_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path(), Shipped::Embedded);
    let mut s = Settings::default();
    s.reporting.folder = "kept".into();
    s.save(&paths).unwrap();

    let mut patch = serde_json::Map::new();
    patch.insert("hiddenServices".into(), json!(["adb"]));
    let next = Settings::patch(&paths, patch).unwrap();
    assert_eq!(next.hidden_services, ["adb"]);
    assert_eq!(next.reporting.folder, "kept");
    assert_eq!(Settings::load(&paths), next);
}

// -- manifest -------------------------------------------------------------------

#[test]
fn normalizing_a_stored_entry_keeps_only_what_has_the_right_shape() {
    let s = manifest::State::normalize(&json!({
        "enabled": "no", "goal": 3, "guidelines": "g",
        "steps": [{"summary": "a", "status": "b"}, {"summary": "no status"}, {"status": "no summary"}, 7],
    }));
    assert_eq!(s.enabled, None);
    assert_eq!(s.goal, None);
    assert_eq!(s.guidelines.as_deref(), Some("g"));
    assert_eq!(
        s.steps,
        [
            Step {
                summary: "a".into(),
                status: "b".into()
            },
            Step {
                summary: "no status".into(),
                status: "".into()
            }
        ]
    );
    assert_eq!(
        manifest::State::normalize(&json!(null)),
        manifest::State::default()
    );
}

#[test]
fn state_serializes_without_unset_fields() {
    let mut s = manifest::State::default();
    assert_eq!(serde_json::to_value(&s).unwrap(), json!({"steps": []}));
    s.enabled = Some(false);
    s.goal = Some("g".into());
    assert_eq!(
        serde_json::to_value(&s).unwrap(),
        json!({"enabled": false, "goal": "g", "steps": []})
    );
}

#[test]
fn an_untouched_session_adds_nothing_to_the_prompt() {
    // The property the prompt cache depends on.
    let cfg = Settings::default();
    assert_eq!(
        manifest::block(&manifest::State::default(), &cfg.manifest),
        None
    );
    assert_eq!(
        identity::block(&identity::State::default(), &cfg.identity),
        None
    );
    assert_eq!(
        Tracker::new()
            .clone()
            .before_agent_start(&SessionState::default(), &cfg.reporting, "p"),
        None
    );
}

#[test]
fn extract_json_survives_fences_prose_and_nonsense() {
    assert_eq!(
        manifest::extract_json("```json\n{\"a\": 1}\n```"),
        Some(json!({"a": 1}))
    );
    assert_eq!(
        manifest::extract_json("here: {\"a\": {\"b\": 2}} done"),
        Some(json!({"a": {"b": 2}}))
    );
    assert_eq!(manifest::extract_json("no braces"), None);
    assert_eq!(manifest::extract_json("} {"), None);
    assert_eq!(manifest::extract_json("{not json}"), None);
}

// -- identity -------------------------------------------------------------------

#[test]
fn the_builtins_are_the_six_the_extension_shipped_in_order() {
    let names: Vec<&str> = identity::BUILTINS.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        [
            "reverse-engineer",
            "cyber-forensics",
            "forensics",
            "software-engineer",
            "infrastructure",
            "publisher"
        ]
    );
    for (n, text) in identity::BUILTINS {
        assert!(text.starts_with("You are the "), "{n}");
        assert!(
            !text.ends_with('\n'),
            "{n}: the texts carry no trailing newline, as in the TS"
        );
        assert!(identity::builtin_description(n).is_some(), "{n}");
    }
}

#[test]
fn saved_identities_keep_file_order_and_overwrite_in_place() {
    let mut u =
        UserIdentities::from_pairs([("zeta".into(), "Z".into()), ("alpha".into(), "A".into())]);
    assert_eq!(u.names().collect::<Vec<_>>(), ["zeta", "alpha"]);
    u.set("zeta".into(), "Z2".into());
    assert_eq!(u.names().collect::<Vec<_>>(), ["zeta", "alpha"]);
    assert_eq!(u.get("zeta"), Some("Z2"));
    assert!(u.remove("zeta"));
    assert!(!u.remove("zeta"));
    assert_eq!(serde_json::to_string(&u).unwrap(), r#"{"alpha":"A"}"#);
}

// -- reporting ------------------------------------------------------------------

#[test]
fn a_missing_folder_snapshots_as_empty_not_as_an_error() {
    assert!(reporting::take_snapshot(Path::new("/definitely/not/here")).is_empty());
}

#[test]
fn the_first_tool_call_only_takes_a_baseline() {
    let cfg = Settings::default().reporting;
    let on = SessionState {
        enabled: Some(true),
        level: None,
    };
    let mut t = Tracker::new();
    let snap = reporting::Snapshot::new();
    t.tool_end(&on, snap.clone());
    assert_eq!(t.steps_since_change, 0, "the baseline is not a step");
    t.tool_end(&on, snap);
    assert_eq!(t.steps_since_change, 1);
    let _ = cfg;
}

#[test]
fn a_disabled_session_counts_nothing() {
    let off = SessionState::default();
    let mut t = Tracker::new();
    for _ in 0..5 {
        t.tool_end(&off, reporting::Snapshot::new());
    }
    assert_eq!(t.steps_since_change, 0);
}

#[test]
fn the_level_is_the_sessions_else_the_globals() {
    let mut cfg = Settings::default().reporting;
    cfg.level = Enforcement::NAG;
    assert_eq!(SessionState::default().level(&cfg), Enforcement::NAG);
    assert_eq!(
        SessionState {
            enabled: None,
            level: Some(Enforcement::STRICT)
        }
        .level(&cfg),
        Enforcement::STRICT
    );
    assert_eq!(Enforcement::new(3), None);
}

// -- scenario -------------------------------------------------------------------

#[test]
fn frontmatter_is_two_fields_and_everything_else_is_prose() {
    let s = scenario::parse_step("---\ntitle: T\ntoolset: triage\nother: x\n---\nbody\n\nmore\n");
    assert_eq!(
        (s.title.as_str(), s.toolset.as_deref(), s.body.as_str()),
        ("T", Some("triage"), "body\n\nmore")
    );
    let none = scenario::parse_step("just prose\n");
    assert_eq!(
        (none.title.as_str(), none.toolset, none.body.as_str()),
        ("", None, "just prose")
    );
    let unterminated = scenario::parse_step("---\ntitle: T\nbody but no closing fence\n");
    assert_eq!(unterminated.title, "", "an unterminated block is prose");
}

#[test]
fn a_crlf_frontmatter_loses_every_field_but_the_last_exactly_as_the_extension_does() {
    // JS `.` excludes \r, so a field line that still ends in \r fails the field
    // regex. The closing `\r?\n---` swallows the *last* line's \r, so that one
    // survives. The golden capture records the same (its `## Phase 3/3: step 3`,
    // with the toolset still activated); this pins the reason.
    let s = scenario::parse_step("---\r\ntitle: X\r\ntoolset: native\r\n---\r\nbody");
    assert_eq!(s.title, "");
    assert_eq!(s.toolset.as_deref(), Some("native"));
    assert_eq!(s.body, "body");
    // With one field there is only a last line.
    assert_eq!(
        scenario::parse_step("---\r\ntitle: X\r\n---\r\nbody").title,
        "X"
    );
}

#[test]
fn the_shipped_scenarios_load_and_name_only_real_toolsets() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scenarios = root.join("prompts/scenarios");
    let paths = Paths::new("/nonexistent", Shipped::Dir(root.clone()));
    let toolsets: Vec<String> = load_toolsets(&paths)
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();

    let ids = scenario::list_scenarios(&scenarios);
    assert!(ids.contains(&"investigation".to_string()), "{ids:?}");
    for id in ids {
        let steps = scenario::load_steps(&scenarios, &id);
        assert!(!steps.is_empty(), "{id}");
        for (i, step) in steps.iter().enumerate() {
            assert!(!step.title.is_empty(), "{id} phase {}: no title", i + 1);
            assert!(!step.body.is_empty(), "{id} phase {}: no body", i + 1);
            if let Some(t) = &step.toolset {
                assert!(
                    toolsets.contains(t),
                    "{id} phase {}: toolset {t:?} is not in toolsets.toml",
                    i + 1
                );
            }
        }
    }
}

#[test]
fn an_unreadable_scenarios_directory_lists_nothing() {
    assert!(scenario::list_scenarios(Path::new("/definitely/not/here")).is_empty());
    assert!(scenario::load_steps(Path::new("/definitely/not/here"), "x").is_empty());
}

// -- context settings and the model list ---------------------------------------------------

mod context_settings {
    use reactor_context::settings::{ContextSettings, Origin, Settings};
    use serde_json::json;

    #[test]
    fn nothing_is_set_until_someone_sets_it() {
        // The built-in defaults live in the budget manager; a layer only says what it changes.
        assert!(Settings::default().context.is_empty());
    }

    #[test]
    fn global_context_settings_load_leniently() {
        let s = Settings::from_json(&json!({
            "context": { "mode": "fade", "pct": 0.8, "keep": 2.0, "reserve": "lots", "window": 32000, "summarizer": "ollama/small" },
            "models": ["ollama/qwen", "no-slash", 3, "anthropic/claude"],
            "defaultModel": "ollama/qwen",
        }));
        assert_eq!(s.context.mode.as_deref(), Some("fade"));
        assert_eq!(s.context.pct, Some(0.8));
        assert_eq!(s.context.keep, None, "keep must be a fraction below 1");
        assert_eq!(
            s.context.reserve, None,
            "wrong shape is unset, not a default"
        );
        assert_eq!(s.context.window, Some(32_000));
        assert_eq!(s.context.summarizer.as_deref(), Some("ollama/small"));
        assert_eq!(
            s.models,
            ["ollama/qwen", "anthropic/claude"],
            "only provider/name specs"
        );
        assert_eq!(s.default_model.as_deref(), Some("ollama/qwen"));
        assert_eq!(
            Settings::from_json(&json!({"context": {"mode": "bogus"}}))
                .context
                .mode,
            None
        );
    }

    #[test]
    fn a_session_layer_wins_field_by_field_and_reports_where_each_came_from() {
        let global = ContextSettings {
            mode: Some("auto".into()),
            window: Some(32_000),
            pct: Some(0.8),
            ..Default::default()
        };
        let session = ContextSettings::normalize(
            &json!({ "mode": "fade", "reserve": 4096, "summarizer": "ollama/small" }),
        );
        let eff = session.over(&global);
        assert_eq!(
            (eff.mode.as_deref(), eff.reserve),
            (Some("fade"), Some(4096))
        );
        assert_eq!(
            eff.pct,
            Some(0.8),
            "everything the session does not set comes from the global"
        );
        assert_eq!(eff.window, Some(32_000));
        assert_eq!(eff.summarizer.as_deref(), Some("ollama/small"));
        assert_eq!(
            eff.keep, None,
            "and what neither sets is left to the built-in default"
        );

        let origins: std::collections::HashMap<_, _> = session.origins().into_iter().collect();
        assert_eq!(origins["mode"], Origin::Session);
        assert_eq!(origins["reserve"], Origin::Session);
        assert_eq!(origins["pct"], Origin::Default);
        assert_eq!(origins["window"], Origin::Default);
    }

    #[test]
    fn an_empty_layer_changes_nothing() {
        let g = ContextSettings {
            pct: Some(0.7),
            ..Default::default()
        };
        assert_eq!(ContextSettings::default().over(&g), g);
        assert_eq!(
            ContextSettings::normalize(&json!(null)),
            ContextSettings::default()
        );
        // Out-of-range values are dropped, not applied.
        assert!(
            ContextSettings::normalize(&json!({"pct": 5, "keep": 0, "mode": "x", "reserve": -1}))
                .is_empty()
        );
    }

    #[test]
    fn context_and_models_survive_a_save_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let paths = reactor_core::Paths::new(dir.path(), reactor_core::paths::Shipped::Embedded);
        let mut s = Settings::default();
        s.context.mode = Some("compact".into());
        s.context.window = Some(65_536);
        s.models = vec!["ollama/qwen".into()];
        s.default_model = Some("ollama/qwen".into());
        s.save(&paths).unwrap();
        assert_eq!(Settings::load(&paths), s);
        // Unset fields are not written at all.
        let text = std::fs::read_to_string(Settings::path(&paths)).unwrap();
        assert!(!text.contains("\"pct\""), "{text}");
    }
}
