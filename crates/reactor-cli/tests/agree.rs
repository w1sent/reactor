//! The GUI's two catalogue paths — `reactor-core` in-process and the `reactor`
//! binary over `--format json` — must give the same answers to every question
//! the GUI asks (ADR-0034: "if the linked library and the binary ever disagree,
//! that is a bug in the library boundary").
//!
//! Lives here rather than in `reactor-client` because only this package can see
//! the built binary (`CARGO_BIN_EXE_reactor`).

use std::path::Path;

use reactor_client::{CliClient, Client, LibClient, ReactorClient};
use reactor_core::paths::{Paths, Shipped};
use serde_json::Value;
use tempfile::TempDir;

/// The golden fixture plus a tool whose service is up, so `services` has all
/// three states to disagree about.
fn fixture() -> TempDir {
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let dir = tempfile::Builder::new().prefix("reactor-agree-").tempdir().unwrap();
    let mut tools = std::fs::read_to_string(golden.join("tools.toml")).unwrap();
    tools.push_str(
        r#"
[tool.answering]
name    = "Answering"
desc    = "a service that answers"
source  = "https://example.invalid/answering"
invoke  = "sh"
detect  = { binary = "sh" }
tags    = ["static"]
service = { probe = ["sh", "-c", "echo 'a device'; echo 'b device'"], label = "answering", count = { pattern = 'device$', noun = "device" } }
"#,
    );
    std::fs::write(dir.path().join("tools.toml"), tools).unwrap();
    std::fs::copy(golden.join("toolsets.toml"), dir.path().join("toolsets.toml")).unwrap();
    dir
}

fn pair(dir: &TempDir) -> (LibClient, CliClient) {
    let lib = LibClient::with_paths(Paths::new(dir.path(), Shipped::Embedded).with_cwd(dir.path()));
    let mut cli = CliClient::new(Some(dir.path().to_path_buf()));
    cli.program = env!("CARGO_BIN_EXE_reactor").to_string();
    cli.envs = vec![
        ("REACTOR_CONFIG_DIR".into(), dir.path().display().to_string()),
        ("HOME".into(), dir.path().display().to_string()),
    ];
    (lib, cli)
}

fn same<T: std::fmt::Debug, E: std::fmt::Display>(what: &str, lib: Result<T, E>, cli: Result<T, E>) {
    let (lib, cli) = (lib.unwrap_or_else(|e| panic!("{what}: lib failed: {e}")), cli.unwrap_or_else(|e| panic!("{what}: cli failed: {e}")));
    assert_eq!(format!("{lib:#?}"), format!("{cli:#?}"), "{what}: the two paths disagree");
}

fn read_everything(a: &dyn ReactorClient, b: &dyn ReactorClient) {
    same("tools", a.tools(), b.tools());
    same("toolsets", a.toolsets(), b.toolsets());
    same("services", a.services(false), b.services(false));
    same("state", a.state(), b.state());
    same("registry", a.registry(), b.registry());
    for id in ["alpha", "beta", "gamma", "answering"] {
        same(&format!("tool_detail({id})"), a.tool_detail(id), b.tool_detail(id));
    }
}

#[test]
fn every_read_agrees_whichever_path_probes_first() {
    // The two paths share one cache. Whoever runs second reads what the first
    // wrote, so each order is its own test of the boundary.
    let dir = fixture();
    let (lib, cli) = pair(&dir);
    read_everything(&lib, &cli);

    let dir = fixture();
    let (lib, cli) = pair(&dir);
    read_everything(&cli, &lib);
}

#[test]
fn a_real_service_reads_as_up_on_both_paths() {
    let dir = fixture();
    let (lib, cli) = pair(&dir);
    for (name, services) in [("lib", lib.services(true).unwrap()), ("cli", cli.services(true).unwrap())] {
        let row = services.services.iter().find(|s| s.id == "answering").unwrap_or_else(|| panic!("{name}: no row"));
        assert_eq!(row.state, "up", "{name}");
        assert_eq!(row.detail.as_deref(), Some("2 devices"), "{name}");
    }
}

/// Activation writes: two identical config dirs, one driven through each path,
/// must end in the same state and answer each step alike.
#[test]
fn writes_agree_and_leave_the_same_state() {
    let (dir_lib, dir_cli) = (fixture(), fixture());
    let (lib, _) = pair(&dir_lib);
    let (_, cli) = pair(&dir_cli);

    let norm = |v: Value, dir: &TempDir| Value::String(v.to_string().replace(&dir.path().display().to_string(), "<CFG>"));
    let steps: Vec<Box<dyn Fn(&dyn ReactorClient) -> Value>> = vec![
        Box::new(|c| c.set_toolset("static", true).unwrap()),
        Box::new(|c| c.set_tool("beta", true).unwrap()),
        Box::new(|c| c.set_tool("alpha", false).unwrap()),
        Box::new(|c| c.set_toolset("static", false).unwrap()),
    ];
    for (i, step) in steps.iter().enumerate() {
        assert_eq!(norm(step(&lib), &dir_lib), norm(step(&cli), &dir_cli), "step {i}");
    }
    assert_eq!(
        std::fs::read_to_string(dir_lib.path().join("state.json")).unwrap(),
        std::fs::read_to_string(dir_cli.path().join("state.json")).unwrap(),
        "state.json differs byte-for-byte"
    );
    assert_eq!(lib.state().unwrap().active, cli.state().unwrap().active);
}

#[test]
fn errors_agree_that_they_are_errors_and_say_why() {
    let dir = fixture();
    let (lib, cli) = pair(&dir);
    let (l, c) = (lib.set_tool("ghost", true).unwrap_err().to_string(), cli.set_tool("ghost", true).unwrap_err().to_string());
    assert!(l.contains("unknown tool(s): ghost"), "{l}");
    // The CLI's json-mode error arrives on stdout; the client must surface it.
    assert!(c.contains("unknown tool(s): ghost"), "{c}");
    assert!(lib.tool_detail("ghost").is_err() && cli.tool_detail("ghost").is_err());
}

#[test]
fn a_project_scoped_state_file_is_seen_from_the_clients_cwd_on_both_paths() {
    let dir = fixture();
    let project = dir.path().join("work");
    std::fs::create_dir_all(project.join(".reactor")).unwrap();
    std::fs::write(project.join(".reactor/state.json"), r#"{"version":1,"toolsets":["pair"],"tools":{"enabled":[],"disabled":[]}}"#).unwrap();

    let lib = LibClient::with_paths(Paths::new(dir.path(), Shipped::Embedded).with_cwd(&project));
    let mut cli = CliClient::new(Some(project.clone()));
    cli.program = env!("CARGO_BIN_EXE_reactor").to_string();
    cli.envs = vec![("REACTOR_CONFIG_DIR".into(), dir.path().display().to_string())];

    let (l, c) = (lib.state().unwrap(), cli.state().unwrap());
    assert_eq!(l.scope.as_deref(), Some("project"));
    assert_eq!(format!("{l:?}"), format!("{c:?}"));
}

#[test]
fn the_client_enum_defaults_to_the_library_and_falls_back_on_request() {
    // Process-global env, so both assertions live in one test.
    unsafe { std::env::remove_var(reactor_client::CLIENT_ENV) };
    assert_eq!(Client::from_env(None).kind(), "lib");
    unsafe { std::env::set_var(reactor_client::CLIENT_ENV, "cli") };
    assert_eq!(Client::from_env(None).kind(), "cli");
    unsafe { std::env::remove_var(reactor_client::CLIENT_ENV) };
}
