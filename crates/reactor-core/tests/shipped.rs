//! The real `tools.toml` and `toolsets.toml`, through the real loader.
//! Port of TestShippedConfig.

use reactor_core::catalogue::{load_catalogue, load_toolsets};
use reactor_core::paths::{Paths, Shipped};
use reactor_core::state::toolset_members;

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// Reads the checkout's files, so this fails the moment someone edits them
/// wrongly — and the compiled-in copies are checked to be the same bytes.
fn paths() -> Paths {
    let none = repo_root().join("does-not-exist");
    Paths::new(none, Shipped::Dir(repo_root()))
}

#[test]
fn the_compiled_in_copies_are_the_checkouts_files() {
    let embedded = Paths::new("/nonexistent", Shipped::Embedded);
    let disk = paths();
    for name in reactor_core::paths::CONFIG_FILES {
        assert_eq!(embedded.shipped_bytes(name).unwrap(), disk.shipped_bytes(name).unwrap(), "{name}");
    }
}

#[test]
fn shipped_catalogue_is_valid() {
    let cat = load_catalogue(&paths()).unwrap();
    assert!(!cat.tools.is_empty());
    assert!(!cat.managers.is_empty());
    assert!(cat.shipped);
}

#[test]
fn every_desc_fits_the_registry_budget() {
    // desc lands in every system prompt for as long as the tool is installed,
    // so it gets a length budget (ADR-0003/0006).
    for t in &load_catalogue(&paths()).unwrap().tools {
        assert!(t.desc.chars().count() <= 80, "{}: desc is {} chars", t.id, t.desc.chars().count());
        assert!(!t.desc.contains('\n'), "{}: desc has a newline", t.id);
    }
}

#[test]
fn every_tool_declares_an_https_source() {
    // ADR-0028: provenance is not decoration. It is what a manual install path
    // (note or oneliner) is trusted relative to, so every shipped entry must
    // have decided where its tool comes from, over https.
    for t in &load_catalogue(&paths()).unwrap().tools {
        let src = t.source.as_deref().unwrap_or_else(|| panic!("{}: no source declared", t.id));
        assert!(src.starts_with("https://"), "{}: source is not https: {src}", t.id);
    }
}

#[test]
fn every_install_key_is_a_manager_or_deliberately_free_text() {
    let cat = load_catalogue(&paths()).unwrap();
    for t in &cat.tools {
        for key in t.install.keys() {
            assert!(
                cat.managers.contains_key(key) || key == "manual" || key == "manual-install-oneliner",
                "{}.install.{key}: neither a declared manager nor `manual` -- a distro name here would never be selected (ADR-0010)",
                t.id
            );
        }
    }
}

#[test]
fn every_prefer_entry_names_a_declared_manager() {
    let cat = load_catalogue(&paths()).unwrap();
    for mid in &cat.prefer {
        assert!(cat.managers.contains_key(mid), "[platform].prefer names undeclared manager {mid:?}");
    }
}

#[test]
fn shipped_toolsets_reference_real_tools_and_tags() {
    let cat = load_catalogue(&paths()).unwrap();
    let tags: std::collections::HashSet<_> = cat.tools.iter().flat_map(|t| t.tags.iter().cloned()).collect();
    for ts in load_toolsets(&paths()).unwrap() {
        for tid in &ts.tools {
            assert!(cat.contains(tid), "toolset {} names unknown tool {tid:?}", ts.id);
        }
        for tag in &ts.tags {
            assert!(tags.contains(tag), "toolset {} names unused tag {tag:?}", ts.id);
        }
    }
}

#[test]
fn no_shipped_toolset_is_empty() {
    let cat = load_catalogue(&paths()).unwrap();
    for ts in load_toolsets(&paths()).unwrap() {
        assert!(!toolset_members(&ts, &cat).is_empty(), "toolset {} selects nothing", ts.id);
    }
}

#[test]
fn a_toolset_named_after_a_tag_selects_only_that_tag() {
    // Where a toolset's name is also a tag, that name is a claim about what is
    // inside it, and this checks the claim -- catching the toolset that reads as
    // narrow but is selected on some other tag entirely.
    let cat = load_catalogue(&paths()).unwrap();
    let tags: std::collections::HashSet<_> = cat.tools.iter().flat_map(|t| t.tags.iter().cloned()).collect();
    for ts in load_toolsets(&paths()).unwrap() {
        if ts.everything || !tags.contains(&ts.id) {
            continue;
        }
        for tid in toolset_members(&ts, &cat) {
            // A tool named in `tools` is a deliberate exception -- frida is in
            // [toolset.android] precisely because it is not tagged `android`.
            // Writing the name is what makes that a decision rather than an
            // accident, so it is allowed and the tag rule still binds the rest.
            if ts.tools.contains(&tid) {
                continue;
            }
            assert!(
                cat.get(&tid).unwrap().tags.contains(&ts.id),
                "toolset {:?} includes {tid:?}, which is not tagged {:?} and is not named in its `tools` list",
                ts.id,
                ts.id
            );
        }
    }
}

#[test]
fn every_tool_belongs_to_a_toolset_other_than_all() {
    // `all` is a catch-all, not a home. A tool reachable only through it is one
    // nobody decided where to put, which is a real state to be in mid-edit but
    // not one to ship: narrowing to any working set would hide a tool the user
    // has installed.
    let cat = load_catalogue(&paths()).unwrap();
    let mut placed = std::collections::HashSet::new();
    for ts in load_toolsets(&paths()).unwrap() {
        if !ts.everything {
            placed.extend(toolset_members(&ts, &cat));
        }
    }
    for t in &cat.tools {
        assert!(placed.contains(&t.id), "{} is in no toolset but `all`", t.id);
    }
}

#[test]
fn every_service_count_pattern_compiles_under_the_regex_crate() {
    // The patterns were written against Python's `re`. The two dialects agree on
    // everything a count pattern needs; this is the tripwire for the day one
    // does not.
    for t in &load_catalogue(&paths()).unwrap().tools {
        if let Some(p) = t.service_count.as_ref().and_then(|c| c.pattern.as_ref()) {
            regex::Regex::new(p).unwrap_or_else(|e| panic!("{}: service count pattern {p:?}: {e}", t.id));
        }
    }
}
