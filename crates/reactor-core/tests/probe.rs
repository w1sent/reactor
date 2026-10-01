//! Probing, services and the cache. Ports of TestServices / TestHelpers
//! (service_detail) / TestAtomicWrites, plus the cache behaviours the Python
//! suite only exercised through the CLI.

mod common;

use std::time::Duration;

use common::*;
use reactor_core::catalogue::load_catalogue;
use reactor_core::catalogue::{DetectKind, ServiceCount, Tool};
use reactor_core::commands::{ProbeFlags, services};
use reactor_core::json::{to_string_pretty, to_string_sorted, write_json_atomic};
use reactor_core::probe::{ProbeOpts, ServiceState, Status, probe, service_detail};
use reactor_core::util::{par_map, run};

const SERVICE_TOOLS: &str = r#"
version = 1

[probe]
timeout = 5.0

[tool.answering]
name    = "Answering"
desc    = "a service that answers"
invoke  = "sh"
detect  = { binary = "sh" }
service = { probe = ["sh", "-c", "echo 'a device'; echo 'b device'"], label = "answering", count = { pattern = 'device$', noun = "device" } }

[tool.refusing]
name    = "Refusing"
desc    = "a service that is not running"
invoke  = "sh"
detect  = { binary = "sh" }
service = { probe = ["sh", "-c", "exit 3"], label = "refusing" }

[tool.uninstalled]
name    = "Uninstalled"
desc    = "declares a service but is not here"
invoke  = "reactor-absent-by-design"
detect  = { binary = "reactor-absent-by-design" }
service = { probe = ["sh", "-c", "exit 0"], label = "uninstalled" }

[tool.plain]
name   = "Plain"
desc   = "no service at all"
invoke = "sh"
detect = { binary = "sh" }
"#;

fn svc_fx() -> Fx {
    Fx::with(SERVICE_TOOLS, FIXTURE_TOOLSETS)
}

fn report(fx: &Fx, cached: bool) -> reactor_core::commands::ServicesReport {
    services(
        &fx.paths,
        ProbeFlags {
            refresh: false,
            cached,
        },
    )
    .unwrap()
    .report
}

fn by_id(
    r: &reactor_core::commands::ServicesReport,
    id: &str,
) -> reactor_core::commands::ServiceRow {
    r.services.iter().find(|s| s.id == id).cloned().unwrap()
}

// -- TestServices -----------------------------------------------------------

#[test]
fn only_tools_with_a_service_probe_are_reported() {
    // `plain` is installed and irrelevant here. Listing it would make the status
    // line a second, worse copy of the registry.
    let ids: Vec<_> = report(&svc_fx(), false)
        .services
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(ids, ["answering", "refusing", "uninstalled"]);
}

#[test]
fn a_running_service_carries_its_count() {
    let r = report(&svc_fx(), false);
    let a = by_id(&r, "answering");
    assert_eq!(a.state, ServiceState::Up);
    assert_eq!(a.detail.as_deref(), Some("2 devices"));
    assert_eq!(a.label, "answering");
}

#[test]
fn a_refused_probe_is_down_with_no_detail() {
    let r = report(&svc_fx(), false);
    let s = by_id(&r, "refusing");
    assert_eq!(s.state, ServiceState::Down);
    assert_eq!(s.detail, None);
}

#[test]
fn an_uninstalled_tool_is_unknown_not_down() {
    // "down" is a claim that something exists and is not running. For a tool
    // that is not installed, that claim is false and misleading -- it would send
    // the agent looking for a service to start.
    let r = report(&svc_fx(), false);
    let s = by_id(&r, "uninstalled");
    assert_eq!(s.state, ServiceState::Unknown);
    assert_eq!(s.status, Status::Absent);
}

#[test]
fn the_summary_counts_every_reported_service() {
    let r = report(&svc_fx(), false);
    assert_eq!((r.summary.up, r.summary.down, r.summary.unknown), (1, 1, 1));
    assert_eq!(
        r.summary.up + r.summary.down + r.summary.unknown,
        r.services.len()
    );
}

#[test]
fn cached_never_probes_and_says_unknown_instead() {
    // The cache is the whole sharing mechanism between extensions (ADR-0014), so
    // `--cached` has to be honest about a cold one rather than reporting a state
    // nobody measured.
    let r = report(&svc_fx(), true);
    assert!(r.services.iter().all(|s| s.state == ServiceState::Unknown));
}

#[test]
fn cached_reads_what_a_previous_probe_left() {
    let fx = svc_fx();
    report(&fx, false);
    let r = report(&fx, true);
    assert_eq!(by_id(&r, "answering").state, ServiceState::Up);
    assert_eq!(by_id(&r, "refusing").state, ServiceState::Down);
}

#[test]
fn text_output_names_every_service() {
    use reactor_core::Report;
    let text = report(&svc_fx(), false).human();
    for tid in ["answering", "refusing", "uninstalled"] {
        assert!(text.contains(tid), "{text}");
    }
    assert!(!text.contains("plain"));
}

#[test]
fn a_slow_probe_is_unknown_not_absent_or_down() {
    let fx = Fx::with(
        "version = 1\n[probe]\ntimeout = 0.3\n\
         [tool.slow]\nname=\"S\"\ndesc=\"d\"\ninvoke=\"sh\"\ndetect={ binary = \"sh\" }\n\
         service = { probe = [\"sh\", \"-c\", \"sleep 5\"], label = \"slow\" }\n",
        FIXTURE_TOOLSETS,
    );
    let started = std::time::Instant::now();
    let r = report(&fx, false);
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "the probe was not cut off"
    );
    assert_eq!(by_id(&r, "slow").state, ServiceState::Unknown);
}

// -- detection ----------------------------------------------------------------

#[test]
fn binaries_are_detected_and_python_modules_asked_of_the_interpreter() {
    let fx = Fx::with(
        "version = 1\n\
         [tool.have]\nname=\"H\"\ndesc=\"d\"\ninvoke=\"sh\"\ndetect={ binary = \"sh\" }\n\
         [tool.lack]\nname=\"L\"\ndesc=\"d\"\ninvoke=\"nope-xyz\"\ndetect={ binary = \"nope-xyz\" }\n\
         [tool.stdlib]\nname=\"S\"\ndesc=\"d\"\ninvoke=\"python3 -c 'import json'\"\ndetect={ python_module = \"json\" }\n\
         [tool.nomod]\nname=\"N\"\ndesc=\"d\"\ninvoke=\"python3 -c 'import zzz_nope'\"\ndetect={ python_module = \"zzz_nope\" }\n",
        FIXTURE_TOOLSETS,
    );
    let cat = load_catalogue(&fx.paths).unwrap();
    let r = probe(&fx.paths, &cat, &cat.ids(), ProbeOpts::new());
    assert_eq!(r["have"].status, Status::Present);
    assert!(
        r["have"]
            .path
            .as_deref()
            .is_some_and(|p| p.ends_with("/sh"))
    );
    assert_eq!(r["lack"].status, Status::Absent);
    if reactor_core::util::which("python3").is_some() {
        assert_eq!(r["stdlib"].status, Status::Present);
        assert_eq!(r["nomod"].status, Status::Absent);
    }
}

#[test]
fn no_interpreter_means_unknown_never_absent() {
    let fx = Fx::with(
        "version = 1\n[probe]\npython = \"reactor-no-such-python\"\n\
         [tool.m]\nname=\"M\"\ndesc=\"d\"\ninvoke=\"python3 -c 'import m'\"\ndetect={ python_module = \"m\" }\n",
        FIXTURE_TOOLSETS,
    );
    let cat = load_catalogue(&fx.paths).unwrap();
    let r = probe(&fx.paths, &cat, &cat.ids(), ProbeOpts::new());
    assert_eq!(r["m"].status, Status::Unknown);
}

#[test]
fn versions_are_probed_for_present_tools_only() {
    let fx = Fx::with(
        "version = 1\n\
         [tool.have]\nname=\"H\"\ndesc=\"d\"\ninvoke=\"sh\"\ndetect={ binary = \"sh\" }\n\
         version = [\"sh\", \"-c\", \"echo 'thing 3.14.15 (built 2031-01-01 on host)'\"]\n\
         [tool.lack]\nname=\"L\"\ndesc=\"d\"\ninvoke=\"nope-xyz\"\ndetect={ binary = \"nope-xyz\" }\n\
         version = [\"sh\", \"-c\", \"echo 9.9.9\"]\n",
        FIXTURE_TOOLSETS,
    );
    let cat = load_catalogue(&fx.paths).unwrap();
    let r = probe(&fx.paths, &cat, &cat.ids(), ProbeOpts::new());
    assert_eq!(r["have"].version.as_deref(), Some("3.14.15"));
    assert_eq!(r["lack"].version, None);
}

#[test]
fn a_config_dir_that_does_not_exist_is_not_created_by_probing() {
    // Reading the shipped fallback must not conjure the config dir out of a
    // plain `reactor doctor`.
    let fx = Fx::new();
    let gone = fx.dir.path().join("not-yet");
    let paths = reactor_core::Paths::new(
        &gone,
        reactor_core::paths::Shipped::Dir(fx.dir.path().to_path_buf()),
    );
    let cat = load_catalogue(&paths).unwrap();
    probe(&paths, &cat, &cat.ids(), ProbeOpts::new());
    assert!(!gone.exists());
}

#[test]
fn the_cache_is_invalidated_when_the_catalogue_changes() {
    let fx = svc_fx();
    report(&fx, false);
    // Change the file: a different stamp, so the old cache must not be believed.
    std::thread::sleep(Duration::from_millis(20));
    let mut text = SERVICE_TOOLS.to_string();
    text.push_str("\n# touched\n");
    fx.write_tools(&text);
    let r = report(&fx, true);
    assert!(r.services.iter().all(|s| s.state == ServiceState::Unknown));
}

// -- service_detail ---------------------------------------------------------

fn tool_with_count(pattern: &str, noun: &str) -> Tool {
    let mut t = Tool::simple("x", "x", DetectKind::Binary, "x");
    t.service_count = Some(ServiceCount {
        pattern: Some(pattern.into()),
        noun: Some(noun.into()),
    });
    t
}

#[test]
fn service_detail_counts_matching_lines() {
    let t = tool_with_count(r"\sdevice$", "device");
    let out = "List of devices attached\nemulator-5554\tdevice\nRZ8N\tdevice\n";
    assert_eq!(service_detail(&t, out).as_deref(), Some("2 devices"));
}

#[test]
fn service_detail_singular() {
    assert_eq!(
        service_detail(&tool_with_count(r"\sdevice$", "device"), "a\tdevice\n").as_deref(),
        Some("1 device")
    );
}

#[test]
fn service_detail_is_none_without_a_count_spec() {
    // No spec means state only. Free-text service output must never reach the
    // registry (ADR-0006).
    let t = Tool::simple("x", "x", DetectKind::Binary, "x");
    assert_eq!(service_detail(&t, "anything at all"), None);
}

#[test]
fn service_detail_survives_a_bad_pattern() {
    assert_eq!(
        service_detail(&tool_with_count("(unclosed", "thing"), "x"),
        None
    );
}

// -- TestAtomicWrites -------------------------------------------------------

#[test]
fn the_temp_file_is_not_shared_between_processes() {
    // A fixed `thing.json.tmp` is what lets two writers interleave into one
    // buffer and then rename the mixture into place (ADR-0014).
    let fx = Fx::new();
    let path = fx.dir.path().join("thing.json");
    write_json_atomic(&path, &serde_json::json!({"a": 1})).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&path).unwrap())
            .unwrap(),
        serde_json::json!({"a": 1})
    );
    let names: Vec<_> = std::fs::read_dir(fx.dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!names.contains(&"thing.json.tmp".to_string()), "{names:?}");
    assert!(
        !names.iter().any(|n| n.ends_with(".tmp")),
        "temp file left behind: {names:?}"
    );
}

#[test]
fn a_failed_write_leaves_no_temp_file_behind() {
    use std::os::unix::fs::PermissionsExt;
    if reactor_core::util::is_root() {
        return; // root writes anywhere; nothing to assert
    }
    let fx = Fx::new();
    let locked = fx.dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    let result = write_json_atomic(&locked.join("thing.json"), &serde_json::json!({"a": 1}));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert_eq!(std::fs::read_dir(&locked).unwrap().count(), 0);
}

// -- json bytes ---------------------------------------------------------------

#[test]
fn json_escapes_non_ascii_the_way_pythons_ensure_ascii_does() {
    // é, ✓ and an astral-plane emoji (a surrogate pair), plus DEL and a control.
    let s = to_string_pretty(&serde_json::json!({"d": "é✓😀\u{7f}\u{1}\n\"\\"}));
    assert_eq!(
        s,
        "{\n  \"d\": \"\\u00e9\\u2713\\ud83d\\ude00\\u007f\\u0001\\n\\\"\\\\\"\n}"
    );
}

#[test]
fn file_json_has_sorted_keys_at_every_depth() {
    // Pinned because serde_json's `preserve_order` is on for this very test
    // binary (see Cargo.toml) — as it is inside the GUI — and would otherwise
    // silently reorder every state.json and cache.json.
    let s =
        to_string_sorted(&serde_json::json!({"b": {"z": 1, "a": 2}, "a": [ {"y": 1, "x": 2} ]}));
    assert_eq!(
        s,
        "{\n  \"a\": [\n    {\n      \"x\": 2,\n      \"y\": 1\n    }\n  ],\n  \"b\": {\n    \"a\": 2,\n    \"z\": 1\n  }\n}"
    );
}

#[test]
fn empty_containers_print_the_way_python_prints_them() {
    assert_eq!(
        to_string_pretty(&serde_json::json!({"a": [], "b": {}})),
        "{\n  \"a\": [],\n  \"b\": {}\n}"
    );
}

// -- util -------------------------------------------------------------------

#[test]
fn run_merges_stderr_into_stdout_and_reports_the_code() {
    let r = run(
        &ids(&["sh", "-c", "echo out; echo err >&2; exit 4"]),
        Duration::from_secs(5),
    );
    assert!(r.completed);
    assert_eq!(r.code, Some(4));
    assert!(
        r.output.contains("out") && r.output.contains("err"),
        "{:?}",
        r.output
    );
}

#[test]
fn run_of_a_missing_binary_is_127_not_a_panic() {
    let r = run(
        &ids(&["reactor-no-such-binary-xyz"]),
        Duration::from_secs(5),
    );
    assert!(r.completed);
    assert_eq!(r.code, Some(127));
}

#[test]
fn run_kills_on_timeout_and_does_not_wait_for_grandchildren() {
    let started = std::time::Instant::now();
    let r = run(
        &ids(&["sh", "-c", "sleep 30 & sleep 30"]),
        Duration::from_millis(200),
    );
    assert!(!r.completed);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn par_map_keeps_input_order() {
    let items: Vec<u32> = (0..50).collect();
    let out = par_map(&items, 8, |n| {
        std::thread::sleep(Duration::from_millis((50 - n) as u64 % 5));
        n * 2
    });
    assert_eq!(out, items.iter().map(|n| n * 2).collect::<Vec<_>>());
}
