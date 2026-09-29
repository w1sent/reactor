//! Catalogue loading, toolsets, activation, override editing and recipe
//! ranking. Ports of TestCatalogue / TestToolsets / TestActivation /
//! TestOverrideEditing / TestRecipeRanking / TestSourceField.

mod common;

use common::*;
use reactor_core::catalogue::{DetectKind, find_toolset, load_catalogue, load_toolsets};
use reactor_core::commands::{ToolsListFlags, ProbeFlags, doctor, DoctorFlags, tools_list};
use reactor_core::recipes::{available_managers, rank_recipes};
use reactor_core::state::{active_ids, load_state, toggle, toolset_members};

// -- TestCatalogue ----------------------------------------------------------

#[test]
fn loads_every_detect_kind() {
    let fx = Fx::new();
    let cat = load_catalogue(&fx.paths).unwrap();
    assert_eq!(cat.ids(), ["alpha", "beta", "gamma"]);
    assert_eq!(cat.get("alpha").unwrap().detect_kind, DetectKind::Binary);
    assert_eq!(cat.get("beta").unwrap().detect_kind, DetectKind::PythonModule);
    assert_eq!(cat.get("gamma").unwrap().service_probe.as_deref(), Some(&ids(&["gamma", "status"])[..]));
}

#[test]
fn declaration_order_is_preserved() {
    // The registry's ordering is the catalogue's ordering, and a stable order
    // is what keeps the rendered block cache-friendly.
    let fx = Fx::new();
    assert_eq!(load_catalogue(&fx.paths).unwrap().order(), ["alpha", "beta", "gamma"]);
}

#[test]
fn declaration_order_is_not_alphabetical() {
    let fx = Fx::with(
        "version = 1\n\
         [tool.zed]\nname=\"Z\"\ndesc=\"d\"\ninvoke=\"z\"\ndetect={ binary = \"z\" }\n\
         [tool.abe]\nname=\"A\"\ndesc=\"d\"\ninvoke=\"a\"\ndetect={ binary = \"a\" }\n",
        FIXTURE_TOOLSETS,
    );
    assert_eq!(load_catalogue(&fx.paths).unwrap().order(), ["zed", "abe"]);
}

#[test]
fn rejects_unknown_detect_kind() {
    let fx = Fx::new();
    fx.write_tools("version = 1\n[tool.x]\nname=\"X\"\ndesc=\"d\"\ninvoke=\"x\"\ndetect={ magic = \"x\" }\n");
    let e = load_catalogue(&fx.paths).unwrap_err();
    assert!(e.to_string().contains("unknown kind"), "{e}");
}

#[test]
fn rejects_missing_required_field() {
    let fx = Fx::new();
    fx.write_tools("version = 1\n[tool.x]\nname=\"X\"\ninvoke=\"x\"\ndetect={ binary = \"x\" }\n");
    let e = load_catalogue(&fx.paths).unwrap_err();
    assert!(e.to_string().contains("desc"), "{e}");
}

#[test]
fn rejects_wrong_version() {
    let fx = Fx::new();
    fx.write_tools("version = 99\n");
    assert!(load_catalogue(&fx.paths).is_err());
}

#[test]
fn rejects_a_detect_table_with_two_kinds() {
    let fx = Fx::new();
    fx.write_tools("version = 1\n[tool.x]\nname=\"X\"\ndesc=\"d\"\ninvoke=\"x\"\ndetect={ binary = \"x\", python_module = \"x\" }\n");
    let e = load_catalogue(&fx.paths).unwrap_err();
    assert!(e.to_string().contains("exactly one"), "{e}");
}

#[test]
fn a_toml_syntax_error_names_the_file() {
    let fx = Fx::new();
    fx.write_tools("version = 1\n[tool.x\n");
    let e = load_catalogue(&fx.paths).unwrap_err();
    assert!(e.to_string().contains("tools.toml"), "{e}");
}

#[test]
fn falls_back_to_the_shipped_copy_and_says_so() {
    let fx = Fx::new();
    std::fs::remove_file(fx.dir.path().join("tools.toml")).unwrap();
    // The shipped dir is the same dir in this fixture, so put a copy elsewhere.
    let shipped = tempfile::tempdir().unwrap();
    std::fs::write(shipped.path().join("tools.toml"), FIXTURE_TOOLS).unwrap();
    std::fs::write(shipped.path().join("toolsets.toml"), FIXTURE_TOOLSETS).unwrap();
    let paths = reactor_core::Paths::new(
        fx.dir.path(),
        reactor_core::paths::Shipped::Dir(shipped.path().to_path_buf()),
    );
    let cat = load_catalogue(&paths).unwrap();
    assert!(cat.shipped);
    assert_eq!(cat.ids(), ["alpha", "beta", "gamma"]);
}

// -- TestToolsets -----------------------------------------------------------

fn members(fx: &Fx, set: &str) -> Vec<String> {
    let cat = load_catalogue(&fx.paths).unwrap();
    let sets = load_toolsets(&fx.paths).unwrap();
    toolset_members(find_toolset(&sets, set).unwrap(), &cat)
}

#[test]
fn all_covers_every_tool_whatever_its_tags() {
    // gamma's only tag is "odd", which no toolset names. A group built from
    // tags would drop it either way; `all = true` must not.
    assert_eq!(members(&Fx::new(), "all"), ["alpha", "beta", "gamma"]);
}

#[test]
fn tag_selection() {
    assert_eq!(members(&Fx::new(), "static"), ["alpha"]);
}

#[test]
fn explicit_selection() {
    assert_eq!(members(&Fx::new(), "pair"), ["alpha", "gamma"]);
}

#[test]
fn tags_intersect_rather_than_union() {
    // ADR-0013. beta is the only "python" tool that is also "dynamic"; alpha is
    // python but static. A union would return both, which is how
    // [toolset.native] once collected every static tool there is.
    let fx = Fx::new();
    assert_eq!(members(&fx, "narrow"), ["alpha"]);
    assert_eq!(members(&fx, "plus"), ["beta", "gamma"]);
}

#[test]
fn tags_nothing_carries_together_select_nothing() {
    // An over-specified list is now empty rather than over-wide. That is the
    // better failure -- visibly nothing beats quietly everything -- but it is a
    // failure, so `reactor doctor` reports it.
    assert!(members(&Fx::new(), "miss").is_empty());
}

fn list_ids(tags: &[&str]) -> Vec<String> {
    let fx = Fx::new();
    let flags = ToolsListFlags {
        tags: ids(tags),
        probe: ProbeFlags { cached: true, refresh: false },
        ..Default::default()
    };
    tools_list(&fx.paths, &flags).unwrap().report.tools.into_iter().map(|t| t.id).collect()
}

#[test]
fn repeating_the_tag_filter_narrows() {
    // `--tag` repeats, so it was the other place a list of tags had a meaning --
    // and it had the other one (ADR-0013).
    assert_eq!(list_ids(&["python"]), ["alpha", "beta"]);
    assert_eq!(list_ids(&["python", "static"]), ["alpha"]);
    assert!(list_ids(&["python", "odd"]).is_empty());
}

#[test]
fn doctor_reports_a_toolset_that_selects_nothing() {
    let fx = Fx::new();
    let report = doctor(&fx.paths, DoctorFlags { cached: true, check_skills: false }).unwrap().report;
    let empty: Vec<_> = report.problems.iter().filter(|p| p.kind == "toolset-empty").map(|p| p.toolset.clone().unwrap()).collect();
    assert_eq!(empty, ["miss"]);
}

// -- TestActivation ---------------------------------------------------------

fn active(fx: &Fx, toolsets: &[&str], enabled: &[&str], disabled: &[&str]) -> Vec<String> {
    let cat = load_catalogue(&fx.paths).unwrap();
    let sets = load_toolsets(&fx.paths).unwrap();
    active_ids(&cat, &sets, &fx.state(toolsets, enabled, disabled)).into_iter().collect()
}

#[test]
fn no_toolsets_means_everything() {
    assert_eq!(active(&Fx::new(), &[], &[], &[]), ["alpha", "beta", "gamma"]);
}

#[test]
fn toolset_narrows() {
    assert_eq!(active(&Fx::new(), &["static"], &[], &[]), ["alpha"]);
}

#[test]
fn enable_adds_outside_the_toolset() {
    assert_eq!(active(&Fx::new(), &["static"], &["beta"], &[]), ["alpha", "beta"]);
}

#[test]
fn disable_wins_over_enable() {
    assert_eq!(active(&Fx::new(), &["static"], &["beta"], &["beta"]), ["alpha"]);
}

#[test]
fn unknown_toolset_is_ignored_not_fatal() {
    // A stale state.json naming a toolset the user has since deleted must not
    // make every command fail.
    assert_eq!(active(&Fx::new(), &["static", "ghost"], &[], &[]), ["alpha"]);
}

#[test]
fn enabling_an_unknown_tool_activates_nothing() {
    assert_eq!(active(&Fx::new(), &["static"], &["ghost"], &[]), ["alpha"]);
}

// -- state scope -------------------------------------------------------------

#[test]
fn a_project_state_file_wins_over_the_machine_one() {
    let fx = Fx::new();
    std::fs::write(fx.dir.path().join("state.json"), r#"{"version":1,"toolsets":["pair"],"tools":{}}"#).unwrap();
    let project = tempfile::tempdir().unwrap();
    let nested = project.path().join("a").join("b");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir_all(project.path().join(".reactor")).unwrap();
    std::fs::write(project.path().join(".reactor/state.json"), r#"{"version":1,"toolsets":["static"],"tools":{}}"#).unwrap();

    let paths = fx.paths.clone().with_cwd(&nested);
    let state = load_state(&paths).unwrap();
    assert_eq!(state.scope.as_str(), "project");
    assert_eq!(state.toolsets, ["static"]);

    let machine = load_state(&fx.paths).unwrap();
    assert_eq!(machine.scope.as_str(), "machine");
    assert_eq!(machine.toolsets, ["pair"]);
}

#[test]
fn no_state_file_is_the_default_scope() {
    let fx = Fx::new();
    let state = load_state(&fx.paths).unwrap();
    assert_eq!(state.scope.as_str(), "default");
    assert!(state.toolsets.is_empty());
}

// -- TestOverrideEditing ----------------------------------------------------
//
// `enable`/`disable`/`reset` write the smallest override that works (ADR-0011).
// The property that matters is the round trip: toggling a tool off and back on
// must leave state.json exactly as it was, or a selector accrues a pin per
// idle keystroke and the toolsets stop meaning anything.

fn toggle_tools(fx: &Fx, verb: &str, targets: &[&str]) -> reactor_core::Result<reactor_core::state::State> {
    let cat = load_catalogue(&fx.paths).unwrap();
    let sets = load_toolsets(&fx.paths).unwrap();
    let mut state = load_state(&fx.paths).unwrap();
    let on = match verb {
        "enable" => Some(true),
        "disable" => Some(false),
        _ => None,
    };
    toggle(&cat, &sets, &mut state, &ids(targets), true, on)?;
    state.save()?;
    Ok(state)
}

#[test]
fn enabling_what_the_base_already_gives_writes_nothing() {
    let fx = Fx::new();
    let state = toggle_tools(&fx, "enable", &["beta"]).unwrap();
    assert!(state.enabled.is_empty());
}

#[test]
fn disabling_what_the_base_does_not_give_writes_nothing() {
    // Base is {alpha} here, so "off" for beta is already true.
    let fx = Fx::new();
    fx.state(&["static"], &[], &[]).save().unwrap();
    let state = toggle_tools(&fx, "disable", &["beta"]).unwrap();
    assert!(state.disabled.is_empty());
}

#[test]
fn off_then_on_is_a_round_trip() {
    let fx = Fx::new();
    let before = fx.read_state_file();
    toggle_tools(&fx, "disable", &["alpha"]).unwrap();
    assert_eq!(fx.read_state_file().unwrap()["tools"]["disabled"], serde_json::json!(["alpha"]));
    toggle_tools(&fx, "enable", &["alpha"]).unwrap();
    let after = fx.read_state_file().unwrap();
    assert_eq!(after["tools"], serde_json::json!({"enabled": [], "disabled": []}));
    if let Some(before) = before {
        assert_eq!(before, after);
    }
}

#[test]
fn an_override_is_only_stored_against_the_toolsets() {
    // With `static` active the base is {alpha}. Turning beta on is a real
    // deviation and is stored; turning alpha on is not.
    let fx = Fx::new();
    fx.state(&["static"], &[], &[]).save().unwrap();
    assert_eq!(toggle_tools(&fx, "enable", &["beta"]).unwrap().enabled, ["beta"]);
    assert_eq!(toggle_tools(&fx, "enable", &["alpha"]).unwrap().enabled, ["beta"]);
}

#[test]
fn reset_drops_an_override_without_asserting_anything() {
    let fx = Fx::new();
    fx.state(&["static"], &["beta"], &["gamma"]).save().unwrap();
    let state = toggle_tools(&fx, "reset", &["beta", "gamma"]).unwrap();
    assert!(state.enabled.is_empty() && state.disabled.is_empty());
    assert_eq!(active(&fx, &["static"], &[], &[]), ["alpha"]);
}

#[test]
fn unknown_tool_is_refused_before_anything_is_written() {
    let fx = Fx::new();
    assert!(toggle_tools(&fx, "disable", &["alpha", "ghost"]).is_err());
    assert!(fx.read_state_file().is_none());
}

#[test]
fn toggling_a_toolset_on_and_off() {
    let fx = Fx::new();
    let cat = load_catalogue(&fx.paths).unwrap();
    let sets = load_toolsets(&fx.paths).unwrap();
    let mut state = load_state(&fx.paths).unwrap();
    toggle(&cat, &sets, &mut state, &ids(&["static"]), false, Some(true)).unwrap();
    assert_eq!(state.toolsets, ["static"]);
    // Enabling twice does not duplicate.
    toggle(&cat, &sets, &mut state, &ids(&["static"]), false, Some(true)).unwrap();
    assert_eq!(state.toolsets, ["static"]);
    toggle(&cat, &sets, &mut state, &ids(&["static"]), false, Some(false)).unwrap();
    assert!(state.toolsets.is_empty());
    assert!(toggle(&cat, &sets, &mut state, &ids(&["ghost"]), false, Some(true)).is_err());
}

// -- TestRecipeRanking ------------------------------------------------------

fn present(items: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
    items.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[test]
fn prefer_order_wins() {
    let fx = Fx::new();
    let cat = load_catalogue(&fx.paths).unwrap();
    let (candidates, notes) = rank_recipes(cat.get("alpha").unwrap(), &cat, &present(&[("uv", "/bin/uv"), ("pacman", "/bin/pacman")]));
    assert_eq!(candidates.iter().map(|c| c.method.as_str()).collect::<Vec<_>>(), ["pacman", "uv"]);
    assert!(candidates[0].sudo);
    assert_eq!(notes.0, [("manual".to_string(), "https://example.invalid/alpha".to_string())]);
}

#[test]
fn absent_manager_is_not_a_candidate() {
    let fx = Fx::new();
    let cat = load_catalogue(&fx.paths).unwrap();
    let (candidates, notes) = rank_recipes(cat.get("alpha").unwrap(), &cat, &present(&[("uv", "/bin/uv")]));
    assert_eq!(candidates.iter().map(|c| c.method.as_str()).collect::<Vec<_>>(), ["uv"]);
    assert!(notes.contains_key("pacman"));
}

#[test]
fn unknown_key_is_a_note_never_a_candidate() {
    // This is what stops `reactor install` ever running free text.
    let fx = Fx::new();
    let cat = load_catalogue(&fx.paths).unwrap();
    let (candidates, notes) = rank_recipes(cat.get("alpha").unwrap(), &cat, &present(&[]));
    assert!(candidates.is_empty());
    assert!(notes.contains_key("manual"));
}

#[test]
fn ties_between_unranked_managers_break_by_declaration_order() {
    let fx = Fx::with(
        "version = 1\n[platform]\nprefer = []\n[platform.manager]\nm1 = { binary = \"m1\" }\nm2 = { binary = \"m2\" }\n\
         [tool.t]\nname=\"T\"\ndesc=\"d\"\ninvoke=\"t\"\ndetect={ binary = \"t\" }\n\
         [tool.t.install]\nm2 = \"m2 i t\"\nm1 = \"m1 i t\"\n",
        FIXTURE_TOOLSETS,
    );
    let cat = load_catalogue(&fx.paths).unwrap();
    let (c, _) = rank_recipes(cat.get("t").unwrap(), &cat, &present(&[("m1", "/m1"), ("m2", "/m2")]));
    assert_eq!(c.iter().map(|c| c.method.as_str()).collect::<Vec<_>>(), ["m2", "m1"]);
}

#[test]
fn os_constrained_manager_is_filtered() {
    // `brew` is declared darwin-only: it may be reported available only on
    // macOS, whatever else is installed.
    let fx = Fx::new();
    let cat = load_catalogue(&fx.paths).unwrap();
    let available = available_managers(&cat);
    if !cfg!(target_os = "macos") {
        assert!(!available.contains_key("brew"));
    }
}

// -- TestSourceField --------------------------------------------------------

#[test]
fn http_source_is_rejected_by_the_loader() {
    // ADR-0028: `source` is provenance, https only.
    let fx = Fx::new();
    let mut text = FIXTURE_TOOLS.to_string();
    text.push_str(
        "\n[tool.demo-source]\nname = \"demo\"\ndesc = \"http source demo\"\nsource = \"http://example.com/demo\"\ninvoke = \"demo-source\"\ndetect = { binary = \"demo-bin\" }\n",
    );
    fx.write_tools(&text);
    let e = load_catalogue(&fx.paths).unwrap_err();
    assert!(e.to_string().contains("https"), "{e}");
}
