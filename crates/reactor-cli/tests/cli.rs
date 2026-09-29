//! The `reactor` binary, spawned for real. Ports of TestJsonContract,
//! TestManualInstall and TestCompletion, plus the byte-for-byte goldens that
//! stand in for the Python CLI's output once it is gone (ADR-0034: `--format
//! json` is frozen at the byte level).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use tempfile::TempDir;

const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

fn repo_file(name: &str) -> String {
    std::fs::read_to_string(Path::new(REPO).join(name)).unwrap()
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{}\nstderr: {}", self.stdout, self.stderr))
    }
}

/// A config dir, a home and a cwd, all throwaway.
struct Env {
    _root: TempDir,
    cfg: PathBuf,
    home: PathBuf,
    cwd: PathBuf,
}

impl Env {
    fn new() -> Self {
        let root = tempfile::Builder::new().prefix("reactor-cli-").tempdir().unwrap();
        let (cfg, home, cwd) = (root.path().join("cfg"), root.path().join("home"), root.path().join("cwd"));
        for d in [&cfg, &home, &cwd] {
            std::fs::create_dir_all(d).unwrap();
        }
        Env { _root: root, cfg, home, cwd }
    }

    /// The shipped catalogue, copied in like `reactor setup` would.
    fn shipped() -> Self {
        let e = Env::new();
        e.write("tools.toml", &repo_file("tools.toml"));
        e.write("toolsets.toml", &repo_file("toolsets.toml"));
        e
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.cfg.join(name), text).unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_reactor"));
        c.args(args)
            .env("REACTOR_CONFIG_DIR", &self.cfg)
            .env("HOME", &self.home)
            .env_remove("REACTOR_PACKAGE_ROOT")
            .env_remove("XDG_DATA_HOME")
            .env_remove("XDG_CONFIG_HOME")
            .current_dir(&self.cwd)
            .stdin(Stdio::null());
        c
    }

    fn run(&self, args: &[&str]) -> Out {
        let o = self.command(args).output().unwrap();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn json(&self, args: &[&str]) -> Out {
        let mut a = args.to_vec();
        a.extend(["--format", "json"]);
        self.run(&a)
    }
}

// -- TestJsonContract -------------------------------------------------------

#[test]
fn registry_shape() {
    let out = Env::shipped().json(&["registry"]);
    let p = out.json();
    assert_eq!(p["schema"], 1);
    for key in ["block", "tools", "summary", "skillPaths"] {
        assert!(p.get(key).is_some(), "missing {key}");
    }
    for key in ["present", "absent", "unknown", "catalogued"] {
        assert!(p["summary"].get(key).is_some(), "summary.{key}");
    }
    for tool in p["tools"].as_array().unwrap() {
        for key in ["id", "desc", "invoke", "status", "active", "detect", "override", "service", "skill", "version", "path", "source", "tags", "name"] {
            assert!(tool.get(key).is_some(), "{}: missing {key}", tool["id"]);
        }
        assert!(["present", "absent", "unknown"].contains(&tool["status"].as_str().unwrap()));
    }
}

#[test]
fn registry_block_is_stable_across_processes() {
    // The end-to-end version of the determinism test: two separate runs,
    // separate probe passes, one cache -- same bytes.
    let env = Env::shipped();
    let first = env.json(&["registry"]).json();
    let second = env.json(&["registry"]).json();
    assert_eq!(first["block"], second["block"]);
}

#[test]
fn registry_block_is_stable_with_a_cold_cache_too() {
    let env = Env::shipped();
    let first = env.json(&["registry", "--refresh"]).json();
    let _ = std::fs::remove_file(env.cfg.join("cache.json"));
    let second = env.json(&["registry", "--refresh"]).json();
    assert_eq!(first["block"], second["block"]);
}

#[test]
fn doctor_shape() {
    let out = Env::shipped().json(&["doctor"]);
    let p = out.json();
    for key in ["platform", "config", "tools", "problems", "skills_stale"] {
        assert!(p.get(key).is_some(), "missing {key}");
    }
    assert_eq!(p["platform"]["sys_platform"], "linux");
    for tool in p["tools"].as_array().unwrap() {
        assert!(tool["install"].get("recommended").is_some());
        assert!(tool["install"].get("candidates").is_some());
        assert!(tool["install"].get("notes").is_some());
    }
}

#[test]
fn json_mode_writes_nothing_to_stdout_but_the_payload() {
    let env = Env::shipped();
    for args in [&["doctor"][..], &["registry"], &["services"], &["tools", "list"], &["toolsets", "list"], &["state"], &["skills", "list"], &["refresh"]] {
        let out = env.json(args);
        // `json()` panics unless the *whole* of stdout is one JSON document.
        out.json();
        assert!(out.stdout.ends_with("}\n"), "{args:?}");
    }
}

#[test]
fn unknown_tool_reports_an_error_not_a_panic() {
    let out = Env::shipped().json(&["tools", "show", "definitely-not-a-tool"]);
    assert_eq!(out.code, 1);
    assert!(out.json().get("error").is_some());
    assert!(!out.stderr.contains("panicked"), "{}", out.stderr);
}

#[test]
fn a_text_mode_error_goes_to_stderr_with_the_program_name() {
    let out = Env::shipped().run(&["tools", "show", "definitely-not-a-tool"]);
    assert_eq!(out.code, 1);
    assert!(out.stdout.is_empty());
    assert!(out.stderr.starts_with("reactor: unknown tool(s): definitely-not-a-tool"), "{}", out.stderr);
}

#[test]
fn install_never_runs_without_confirmation() {
    // json mode has no tty to confirm on, so it must refuse rather than assume
    // yes. Every catalogued tool is a plausible `sudo pacman -S`.
    let out = Env::shipped().json(&["install", "jq"]);
    let p = out.json();
    assert_eq!(p.get("ran").and_then(Value::as_array).map(Vec::len).unwrap_or(0), 0);
}

#[test]
fn install_all_targets_the_whole_catalogue() {
    let env = Env::shipped();
    let cat: Vec<String> = env.json(&["tools", "list", "--cached"]).json()["tools"]
        .as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap().to_string()).collect();
    let out = env.json(&["install", "all", "--dry-run"]);
    assert_eq!(out.code, 0);
    let p = out.json();
    let mut covered: Vec<String> = p["plan"].as_array().unwrap().iter().chain(p["skipped"].as_array().unwrap())
        .map(|e| e["tool"].as_str().unwrap().to_string()).collect();
    covered.sort();
    let mut expect = cat;
    expect.sort();
    assert_eq!(covered, expect);
}

#[test]
fn install_all_cannot_be_mixed_with_a_real_id() {
    // `all` already means everything; a second id has nothing left to narrow, so
    // it is rejected as an unknown tool rather than ignored.
    let out = Env::shipped().json(&["install", "all", "jq", "--dry-run"]);
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("all"));
}

#[test]
fn usage_errors_exit_2() {
    let env = Env::shipped();
    assert_eq!(env.run(&["nonsense"]).code, 2);
    assert_eq!(env.run(&["tools"]).code, 2);
    assert_eq!(env.run(&["install"]).code, 2);
    assert_eq!(env.run(&["tools", "list", "--format", "yaml"]).code, 2);
}

#[test]
fn format_is_accepted_before_or_after_the_subcommand() {
    let env = Env::shipped();
    assert_eq!(env.run(&["--format", "json", "state"]).json()["schema"], 1);
    assert_eq!(env.run(&["state", "--format", "json"]).json()["schema"], 1);
}

// -- activation through the binary ------------------------------------------

#[test]
fn enable_and_disable_round_trip_through_state_json() {
    let env = Env::shipped();
    let before = std::fs::read_to_string(env.cfg.join("state.json")).ok();
    let out = env.json(&["tools", "disable", "jq"]);
    assert_eq!(out.code, 0);
    let p = out.json();
    assert_eq!(p["state"]["tools"]["disabled"], serde_json::json!(["jq"]));
    assert!(!p["active"].as_array().unwrap().contains(&Value::from("jq")));
    let on = env.json(&["tools", "enable", "jq"]).json();
    assert_eq!(on["state"]["tools"]["disabled"], serde_json::json!([]));
    if let Some(before) = before {
        assert_eq!(before, std::fs::read_to_string(env.cfg.join("state.json")).unwrap());
    }
}

#[test]
fn state_json_is_written_with_sorted_keys_and_a_trailing_newline() {
    let env = Env::shipped();
    env.json(&["toolsets", "enable", "triage"]);
    let text = std::fs::read_to_string(env.cfg.join("state.json")).unwrap();
    assert_eq!(
        text,
        "{\n  \"tools\": {\n    \"disabled\": [],\n    \"enabled\": []\n  },\n  \"toolsets\": [\n    \"triage\"\n  ],\n  \"version\": 1\n}\n"
    );
}

#[test]
fn a_project_state_file_is_used_and_written_in_place() {
    let env = Env::shipped();
    let project = env.cwd.join("work");
    std::fs::create_dir_all(project.join(".reactor")).unwrap();
    std::fs::write(project.join(".reactor/state.json"), r#"{"version":1,"toolsets":[],"tools":{}}"#).unwrap();
    let nested = project.join("deep/er");
    std::fs::create_dir_all(&nested).unwrap();

    let mut cmd = env.command(&["state", "--format", "json"]);
    let o = cmd.current_dir(&nested).output().unwrap();
    let p: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(p["scope"], "project");
    assert_eq!(p["state"]["toolsets"], serde_json::json!([]));

    let mut cmd = env.command(&["tools", "disable", "jq", "--format", "json"]);
    cmd.current_dir(&nested).output().unwrap();
    let written: Value = serde_json::from_str(&std::fs::read_to_string(project.join(".reactor/state.json")).unwrap()).unwrap();
    assert_eq!(written["tools"]["disabled"], serde_json::json!(["jq"]));
    assert!(!env.cfg.join("state.json").exists(), "the machine state must not be touched");
}

// -- TestManualInstall ------------------------------------------------------

const DEMO_MANUAL_TOOL: &str = r#"

[tool.demo-manual]
name    = "demo"
desc    = "manual one-liner demo"
source  = "https://example.com/demo"
invoke  = "demo-manual"
detect  = { binary = "demo-bin" }
tags    = ["general"]

[tool.demo-manual.install]
manual = "prose only, never a command"
manual-install-oneliner = "mkdir -p out && touch out/demo-bin && chmod +x out/demo-bin"
"#;

/// Runs against a catalogue carrying one tool with no manager recipes at all, so
/// the tests stay machine-independent: no manager's presence or absence can
/// change which branch executes.
fn manual_env() -> Env {
    let env = Env::new();
    env.write("tools.toml", &(repo_file("tools.toml") + DEMO_MANUAL_TOOL));
    env.write("toolsets.toml", &repo_file("toolsets.toml"));
    env
}

#[test]
fn force_plans_the_one_liner() {
    let out = manual_env().json(&["install", "demo-manual", "--force-install-manual", "--dry-run"]);
    assert_eq!(out.code, 0);
    let p = out.json();
    assert_eq!(p["plan"][0]["method"], "manual");
    assert!(p["plan"][0]["command"].as_str().unwrap().contains("demo-bin"));
    assert_eq!(p["plan"][0]["argv"][0], "sh");
}

#[test]
fn auto_falls_back_when_no_manager_recipe_ran() {
    let out = manual_env().json(&["install", "demo-manual", "--auto-install-manual", "--dry-run"]);
    assert_eq!(out.code, 0);
    assert_eq!(out.json()["plan"][0]["method"], "manual");
}

#[test]
fn without_a_flag_the_oneliner_is_only_a_note() {
    let out = manual_env().json(&["install", "demo-manual", "--dry-run"]);
    assert_eq!(out.code, 0);
    let p = out.json();
    assert_eq!(p["plan"].as_array().unwrap().len(), 0);
    assert!(
        p["skipped"].as_array().unwrap().iter().any(|s| s["reason"].as_str().unwrap().contains("manual-install-oneliner")),
        "skip reasons should point at the oneliner: {}", p["skipped"]
    );
}

#[test]
fn force_without_a_oneliner_skips() {
    // bulk-extractor: has only `brew` + a `manual` note, no oneliner, and is not
    // plausibly installed on a dev machine -- so the skip is the reason we are
    // checking, not "already present".
    let out = manual_env().json(&["install", "bulk-extractor", "--force-install-manual", "--dry-run"]);
    assert_eq!(out.code, 0);
    let p = out.json();
    assert_eq!(p["plan"].as_array().unwrap().len(), 0);
    assert!(p["skipped"].as_array().unwrap().iter().any(|s| s["reason"] == "no manual-install-oneliner"));
}

#[test]
fn flags_are_mutually_exclusive() {
    let out = manual_env().json(&["install", "demo-manual", "--auto-install-manual", "--force-install-manual"]);
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("mutually exclusive"));
}

#[test]
fn method_conflicts_with_the_manual_flags() {
    let out = manual_env().json(&["install", "demo-manual", "--method", "pacman", "--force-install-manual"]);
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("--method"));
}

#[test]
fn run_executes_the_oneliner_and_promotes_onto_path() {
    let env = manual_env();
    let out = env.json(&["install", "demo-manual", "--force-install-manual", "--yes"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let p = out.json();
    let ran = &p["ran"][0];
    assert_eq!(ran["returncode"], 0);
    let link = PathBuf::from(ran["path"].as_str().unwrap());
    assert_eq!(link.parent().unwrap(), env.home.join(".local/bin"));
    assert!(link.is_symlink());
    {
        use std::os::unix::fs::PermissionsExt;
        assert!(std::fs::metadata(&link).unwrap().permissions().mode() & 0o111 != 0);
    }
    // The scratch dir is the install dir, not a temp dir (ADR-0027).
    assert!(env.cfg.join("manual/demo-manual/out/demo-bin").exists());
    // The fake home's .local/bin cannot be in the inherited PATH, so the run must
    // say so rather than claim the binary is reachable.
    assert!(ran.get("hint").is_some());
}

// -- TestCompletion ---------------------------------------------------------

#[test]
fn complete_ids_lists_every_catalogued_tool_in_declaration_order() {
    let env = Env::shipped();
    let out = env.run(&["__complete", "tools"]);
    assert_eq!(out.code, 0);
    let listed: Vec<String> = env.json(&["tools", "list", "--cached"]).json()["tools"]
        .as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(out.stdout.lines().collect::<Vec<_>>(), listed);
}

#[test]
fn complete_ids_lists_every_toolset() {
    let env = Env::shipped();
    let out = env.run(&["__complete", "toolsets"]);
    let listed: std::collections::BTreeSet<String> = env.json(&["toolsets", "list"]).json()["toolsets"]
        .as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(out.stdout.lines().map(str::to_string).collect::<std::collections::BTreeSet<_>>(), listed);
}

#[test]
fn complete_ids_does_not_probe() {
    // The whole point (ADR-0015): a <TAB> press must cost a catalogue load, not a
    // detection sweep. No probe means no cache is written.
    let env = Env::shipped();
    env.run(&["__complete", "tools"]);
    assert!(!env.cfg.join("cache.json").exists());
}

#[test]
fn complete_is_hidden_from_help() {
    let out = Env::shipped().run(&["--help"]);
    assert!(!out.stdout.contains("__complete"));
}

#[test]
fn completion_prints_a_script_per_shell_that_calls_back_in() {
    let env = Env::shipped();
    for shell in ["bash", "zsh", "fish"] {
        let out = env.run(&["completion", shell]);
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert!(out.stdout.contains("__complete"));
        assert!(out.stdout.contains("setup"), "{shell}: the top-level command list must know `setup`");
    }
}

#[test]
fn completion_rejects_an_unknown_shell() {
    assert_ne!(Env::shipped().run(&["completion", "powershell"]).code, 0);
}

#[test]
fn every_top_level_command_is_in_every_completion_script() {
    // ADR-0015: the scripts are the one place the command surface is spelled out
    // by hand, so a command added to the CLI without them is a silent gap.
    let env = Env::shipped();
    let help = env.run(&["--help"]).stdout;
    let commands: Vec<String> = help
        .lines()
        .skip_while(|l| !l.starts_with("Commands:"))
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .filter(|c| c != "help")
        .collect();
    assert!(commands.len() >= 12, "{commands:?}");
    for shell in ["bash", "zsh", "fish"] {
        let script = env.run(&["completion", shell]).stdout;
        for c in &commands {
            assert!(script.contains(c.as_str()), "{shell} completion does not mention `{c}`");
        }
    }
}

// -- goldens ------------------------------------------------------------------

/// The fixture catalogue's outputs as the Python CLI printed them, with the
/// config dir masked. Every one of these is a byte-level promise.
#[test]
fn json_and_text_outputs_are_byte_identical_to_the_python_clis() {
    let env = Env::new();
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    env.write("tools.toml", &std::fs::read_to_string(golden.join("tools.toml")).unwrap());
    env.write("toolsets.toml", &std::fs::read_to_string(golden.join("toolsets.toml")).unwrap());
    let cfg = env.cfg.display().to_string();

    let cases: &[(&[&str], &str)] = &[
        (&["registry", "--cached", "--format", "json"], "registry-cached.json"),
        (&["registry", "--cached"], "registry-cached.txt"),
        (&["tools", "list", "--cached", "--format", "json"], "tools-list-cached.json"),
        (&["tools", "list", "--cached"], "tools-list-cached.txt"),
        (&["toolsets", "list", "--format", "json"], "toolsets-list.json"),
        (&["toolsets", "list"], "toolsets-list.txt"),
        (&["state", "--format", "json"], "state.json.golden"),
        (&["services", "--cached", "--format", "json"], "services-cached.json"),
        (&["services", "--cached"], "services-cached.txt"),
        (&["tools", "show", "gamma", "--cached", "--format", "json"], "tools-show-gamma.json"),
    ];
    for (args, file) in cases {
        let out = env.run(args);
        assert_eq!(out.code, 0, "{args:?}: {}", out.stderr);
        let want = std::fs::read_to_string(golden.join(file)).unwrap();
        let got = out.stdout.replace(&cfg, "<CFG>");
        assert_eq!(got, want, "{args:?} differs from {file}");
    }
}

// -- setup and the state root -------------------------------------------------

#[test]
fn setup_seeds_config_and_state_and_never_clobbers() {
    let env = Env::new();
    let out = env.run(&["setup", "--no-skills", "--no-completions"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(std::fs::read_to_string(env.cfg.join("tools.toml")).unwrap(), repo_file("tools.toml"));
    assert_eq!(std::fs::read_to_string(env.cfg.join("toolsets.toml")).unwrap(), repo_file("toolsets.toml"));
    assert!(env.cfg.join("state.json").is_file());

    // A user edit survives a re-run, and setup says so.
    let edited = repo_file("tools.toml") + "\n# mine\n";
    env.write("tools.toml", &edited);
    let again = env.run(&["setup", "--no-skills", "--no-completions"]);
    assert_eq!(std::fs::read_to_string(env.cfg.join("tools.toml")).unwrap(), edited);
    assert!(again.stdout.contains("differs from the shipped copy"), "{}", again.stdout);
    assert!(again.stdout.contains("(kept)"));
}

#[test]
fn setup_dry_run_writes_nothing() {
    let env = Env::new();
    std::fs::remove_dir_all(&env.cfg).unwrap();
    let out = env.run(&["setup", "--dry-run"]);
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("would seed"));
    assert!(!env.cfg.exists());
    assert!(!env.home.join(".zfunc").exists());
}

#[test]
fn setup_installs_completions_where_each_shell_looks() {
    let env = Env::new();
    let out = env.run(&["setup", "--no-skills"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    for (shell, rel) in [
        ("bash", ".local/share/bash-completion/completions/reactor"),
        ("zsh", ".zfunc/_reactor"),
        ("fish", ".config/fish/completions/reactor.fish"),
    ] {
        let written = std::fs::read_to_string(env.home.join(rel)).unwrap_or_else(|e| panic!("{shell}: {e}"));
        assert_eq!(written, env.run(&["completion", shell]).stdout);
    }
    assert!(out.stdout.contains("fpath+="));
}

#[test]
fn a_bare_binary_runs_doctor_before_setup_from_the_compiled_in_catalogue() {
    // No config dir at all: the shipped copy is read, and doctor says to run setup.
    let env = Env::new();
    std::fs::remove_dir_all(&env.cfg).unwrap();
    let out = env.json(&["doctor", "--cached"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let p = out.json();
    assert_eq!(p["config"]["shipped_fallback"], true);
    assert!(p["problems"].as_array().unwrap().iter().any(|x| x["kind"] == "not-installed"));
    assert!(!env.cfg.exists(), "reading the fallback must not create the config dir");
}

#[test]
fn the_legacy_state_root_is_moved_once() {
    let env = Env::new();
    let old = env.home.join(".pi/reactor");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("tools.toml"), repo_file("tools.toml")).unwrap();
    std::fs::write(old.join("state.json"), r#"{"version":1,"toolsets":["static"],"tools":{"enabled":[],"disabled":[]}}"#).unwrap();

    // No REACTOR_CONFIG_DIR: the default location is in play.
    let mut cmd = env.command(&["state", "--format", "json"]);
    cmd.env_remove("REACTOR_CONFIG_DIR");
    let o = cmd.output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let p: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(p["state"]["toolsets"], serde_json::json!(["static"]));
    assert!(env.home.join(".reactor/tools.toml").is_file());
    assert!(!old.exists(), "the old root should have moved, not been copied");
}

#[test]
fn the_legacy_root_is_left_alone_when_the_new_one_exists() {
    let env = Env::new();
    let old = env.home.join(".pi/reactor");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("state.json"), "{}").unwrap();
    std::fs::create_dir_all(env.home.join(".reactor")).unwrap();

    let mut cmd = env.command(&["state", "--format", "json"]);
    cmd.env_remove("REACTOR_CONFIG_DIR");
    cmd.output().unwrap();
    assert!(old.join("state.json").is_file());
}

#[test]
fn an_explicit_config_dir_never_triggers_the_move() {
    let env = Env::new();
    let old = env.home.join(".pi/reactor");
    std::fs::create_dir_all(&old).unwrap();
    env.run(&["state"]);
    assert!(old.is_dir());
    assert!(!env.home.join(".reactor").exists());
}

// -- diff-config / overwrite-config ------------------------------------------

#[test]
fn diff_config_reports_drift_and_exits_1() {
    let env = Env::shipped();
    let same = env.json(&["diff-config"]);
    assert_eq!(same.code, 0);
    assert_eq!(same.json()["differ"], false);

    env.write("tools.toml", &(repo_file("tools.toml") + "\n# mine\n"));
    let out = env.json(&["diff-config", "--file", "tools"]);
    assert_eq!(out.code, 1);
    let p = out.json();
    assert_eq!(p["differ"], true);
    assert!(p["diff"].as_str().unwrap().contains("--- shipped/tools.toml"));
    assert!(p["diff"].as_str().unwrap().contains("+# mine"));
    assert_eq!(p["files"][0]["state"], "differs");
}

#[test]
fn overwrite_config_backs_up_and_needs_yes() {
    let env = Env::shipped();
    let edited = repo_file("tools.toml") + "\n# mine\n";
    env.write("tools.toml", &edited);

    let refused = env.json(&["overwrite-config"]);
    assert_eq!(refused.code, 1);
    assert_eq!(std::fs::read_to_string(env.cfg.join("tools.toml")).unwrap(), edited);

    let done = env.json(&["overwrite-config", "--yes"]);
    assert_eq!(done.code, 0);
    let p = done.json();
    assert_eq!(p["replaced"], serde_json::json!(["tools.toml"]));
    let backup = PathBuf::from(p["backups"][0].as_str().unwrap());
    assert_eq!(std::fs::read_to_string(backup).unwrap(), edited);
    assert_eq!(std::fs::read_to_string(env.cfg.join("tools.toml")).unwrap(), repo_file("tools.toml"));
}

// -- skills -------------------------------------------------------------------

#[test]
fn skills_show_prints_a_fetched_skill_and_refuses_an_unfetched_one() {
    let env = Env::shipped();
    let missing = env.json(&["skills", "show", "bn"]);
    assert_eq!(missing.code, 1);
    assert!(missing.json()["error"].as_str().unwrap().contains("no fetched skill"));

    let dir = env.cfg.join("skills/bn");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "---\nname: bn\n---\nbody\n").unwrap();
    let ok = env.run(&["skills", "show", "bn"]);
    assert_eq!(ok.stdout, "---\nname: bn\n---\nbody\n\n");
}

#[test]
fn skills_list_reports_fetch_state_and_metadata() {
    let env = Env::shipped();
    let dir = env.cfg.join("skills/bn");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "---\n---\n").unwrap();
    std::fs::write(dir.join(".reactor-skill.json"), r#"{"commit":"abcdef0123456789","fetched_at":1700000000}"#).unwrap();
    let p = env.json(&["skills", "list"]).json();
    let bn = p["skills"].as_array().unwrap().iter().find(|s| s["tool"] == "bn").unwrap();
    assert_eq!(bn["fetched"], true);
    assert_eq!(bn["commit"], "abcdef0123456789");
    assert_eq!(bn["fetched_at"], 1700000000);
}

#[test]
fn the_skill_paths_in_the_registry_are_sorted_and_only_for_present_tools() {
    // A fetched skill for a tool that is not here must not be advertised.
    let env = Env::shipped();
    let dir = env.cfg.join("skills/bn");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "---\n---\n").unwrap();
    let p = env.json(&["registry", "--cached"]).json();
    assert_eq!(p["skillPaths"], serde_json::json!([]));
}

// -- argparse compatibility ---------------------------------------------------

#[test]
fn a_repeated_flag_is_accepted_and_the_last_one_wins() {
    // The GUI's client used to send `--format json --format json`; argparse
    // allowed it, so callers depend on it.
    let env = Env::shipped();
    let out = env.run(&["services", "--format", "text", "--format", "json", "--cached", "--cached"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.json()["schema"], 1);
}

#[test]
fn repeated_tags_still_accumulate() {
    let env = Env::shipped();
    let one = env.json(&["tools", "list", "--cached", "--tag", "static"]).json();
    let two = env.json(&["tools", "list", "--cached", "--tag", "static", "--tag", "native"]).json();
    let n = |v: &Value| v["tools"].as_array().unwrap().len();
    assert!(n(&two) < n(&one), "a second --tag must narrow, not replace: {} vs {}", n(&two), n(&one));
}
