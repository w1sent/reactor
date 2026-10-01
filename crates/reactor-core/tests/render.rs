//! The registry block — the load-bearing property of this project.
//!
//! Replacing the system prompt invalidates the provider's cached prefix, so the
//! rendered block must be byte-identical across turns when nothing about the
//! machine changed (ADR-0006). That is a property, not an intention, so it gets
//! tests: rendering (TestRegistryRendering), determinism
//! (TestRegistryDeterminism), and golden bytes captured from the Python
//! renderer this one replaces.

mod common;

use common::*;
use reactor_core::probe::{ServiceState, Status, version_of};
use reactor_core::render::render_registry;

// -- TestRegistryRendering --------------------------------------------------

#[test]
fn only_present_and_active_tools_are_listed() {
    let mut beta = entry("beta");
    beta.status = Status::Absent;
    let mut gamma = entry("gamma");
    gamma.active = false;
    let block = render_registry(&[entry("alpha"), beta, gamma]);
    assert!(block.contains("alpha"));
    assert!(!block.contains("beta"));
    assert!(!block.contains("gamma"));
}

#[test]
fn empty_registry_says_so_and_points_somewhere() {
    let mut a = entry("alpha");
    a.status = Status::Absent;
    assert!(render_registry(&[a]).contains("reactor doctor"));
}

#[test]
fn python_modules_show_their_module_not_the_usage_example() {
    let block = render_registry(&[python_module(entry("beta"))]);
    assert!(block.lines().any(|l| l.starts_with("beta ")), "{block}");
    assert!(block.contains("(python module)"));
}

#[test]
fn service_state_is_annotated() {
    let mut adb = entry("adb");
    adb.service = service("adb", ServiceState::Up, Some("2 devices"));
    assert!(render_registry(&[adb]).contains("[adb: 2 devices]"));
}

#[test]
fn service_down_is_shown_not_hidden() {
    // A tool whose service is down is still installed and still worth knowing
    // about -- and `bn` reporting "down" is how the agent learns to start Binary
    // Ninja rather than concluding it does not exist.
    let mut bn = entry("bn");
    bn.service = service("BN", ServiceState::Down, None);
    assert!(render_registry(&[bn]).contains("[BN: down]"));
}

#[test]
fn no_line_has_trailing_whitespace() {
    let mut a = entry("a");
    a.desc = "short".into();
    a.version = Some("1.0".into());
    let mut b = entry("bbbbbb");
    b.desc = "a much longer description here".into();
    for line in render_registry(&[a, b]).lines() {
        assert_eq!(line, line.trim_end(), "trailing whitespace: {line:?}");
    }
}

// -- TestRegistryDeterminism ------------------------------------------------

fn entries() -> Vec<reactor_core::model::ToolEntry> {
    let mut bn = entry("bn");
    bn.desc = "reverse engineering framework".into();
    bn.service = service("BN session", ServiceState::Up, None);
    let mut frida = entry("frida");
    frida.desc = "dynamic instrumentation".into();
    frida.version = Some("17.2".into());
    let mut jadx = entry("jadx");
    jadx.desc = "decompile Android DEX/APK to Java".into();
    let mut lief = python_module(entry("lief"));
    lief.desc = "parse ELF/PE/Mach-O".into();
    vec![bn, frida, jadx, lief]
}

#[test]
fn identical_input_renders_identical_bytes() {
    assert_eq!(
        render_registry(&entries()).as_bytes(),
        render_registry(&entries().clone()).as_bytes()
    );
}

#[test]
fn rendering_carries_nothing_time_derived() {
    let block = render_registry(&entries());
    // Any four-digit year, clock time, or epoch-scale integer would mean a new
    // system prompt every turn.
    let has = |re: &str| regex::Regex::new(re).unwrap().is_match(&block);
    assert!(!has(r"\b(19|20)\d{2}\b"));
    assert!(!has(r"\b\d{2}:\d{2}\b"));
    assert!(!has(r"\b1[6-9]\d{8}\b"));
}

#[test]
fn a_real_change_does_change_the_bytes() {
    // The flip side: caching must not be bought by ignoring reality.
    let before = render_registry(&entries());
    let mut changed = entries();
    changed[0].service = service("BN session", ServiceState::Down, None);
    assert_ne!(before, render_registry(&changed));
}

#[test]
fn deactivating_a_tool_changes_the_bytes() {
    let before = render_registry(&entries());
    let mut changed = entries();
    changed[1].active = false;
    assert_ne!(before, render_registry(&changed));
}

// -- golden bytes -------------------------------------------------------------

/// The Python renderer's output for this entry set, frozen. If this fails the
/// prompt cache is being invalidated for every user on upgrade.
#[test]
fn the_block_is_byte_identical_to_the_python_renderers() {
    let mut list = Vec::new();

    let mut bn = entry("bn");
    bn.desc = "reverse engineering framework".into();
    bn.service = service("BN session", ServiceState::Up, None);
    list.push(bn);

    let mut frida = entry("frida");
    frida.desc = "dynamic instrumentation".into();
    frida.version = Some("17.2".into());
    list.push(frida);

    let mut jadx = entry("jadx");
    jadx.desc = "decompile Android DEX/APK to Java".into();
    list.push(jadx);

    let mut lief = python_module(entry("lief"));
    lief.desc = "parse ELF/PE/Mach-O".into();
    list.push(lief);

    let mut adb = entry("adb");
    adb.desc = "Android debug bridge".into();
    adb.service = service("adb", ServiceState::Up, Some("2 devices"));
    list.push(adb);

    let mut ghidra = entry("ghidra-headless");
    ghidra.desc = "détecte ✓ 😀 non-ASCII width".into();
    ghidra.version = Some("11.4.1-rc2".into());
    list.push(ghidra);

    let mut r2 = entry("radare2");
    r2.desc = "disassembler".into();
    r2.service = service("r2", ServiceState::Down, None);
    list.push(r2);

    let mut scapy = python_module(entry("scapy"));
    scapy.desc = "packet crafting".into();
    scapy.service = service("scapy", ServiceState::Unknown, None);
    list.push(scapy);

    let mut hidden = entry("hidden");
    hidden.active = false;
    list.push(hidden);
    let mut gone = entry("gone");
    gone.status = Status::Absent;
    list.push(gone);

    assert_eq!(
        render_registry(&list),
        include_str!("golden/registry-block.txt")
    );

    let mut x = entry("x");
    x.status = Status::Absent;
    assert_eq!(
        render_registry(&[x]),
        include_str!("golden/registry-empty.txt")
    );
}

// -- TestHelpers ------------------------------------------------------------

#[test]
fn version_extraction_drops_the_banner() {
    // Full --version banners carry build dates and hostnames; letting one into
    // the registry would rewrite the system prompt for no reason.
    assert_eq!(
        version_of("ripgrep 14.1.1 (rev abc123)").as_deref(),
        Some("14.1.1")
    );
    assert_eq!(version_of("GNU gdb (GDB) 16.2").as_deref(), Some("16.2"));
    assert_eq!(version_of("frida 17.2").as_deref(), Some("17.2"));
}

#[test]
fn version_extraction_keeps_a_suffix() {
    assert_eq!(
        version_of("ghidra 11.4.1-rc2 built").as_deref(),
        Some("11.4.1-rc2")
    );
}

#[test]
fn version_extraction_survives_no_number() {
    assert_eq!(version_of(""), None);
    assert_eq!(
        version_of("unknown build").as_deref(),
        Some("unknown build")
    );
    // Truncated by characters, never bytes.
    let long = "é".repeat(40);
    assert_eq!(version_of(&long).unwrap().chars().count(), 32);
}
