//! `reactor install`: the pseudo-target `decompile-python[all]` (TestDecompilePythonAll)
//! and the plan/confirm/run flow, driven through a fake [`Host`] so nothing
//! here shells out to a real package manager.

mod common;

use std::cell::RefCell;
use std::collections::BTreeMap;

use common::*;
use reactor_core::catalogue::{Catalogue, load_catalogue};
use reactor_core::host::{Capture, Exec, ExecOutcome, Host};
use reactor_core::install::{DECOMPILE_PYTHON_ALL_ID, InstallOpts, InstallOutcome, install};

struct FakeHost {
    managers: BTreeMap<String, String>,
    discovered: BTreeMap<String, Vec<String>>,
    json: bool,
    yes: bool,
    /// Every argv `exec` was asked to run.
    ran: RefCell<Vec<Vec<String>>>,
    exit_code: i32,
    captured: String,
    /// Text-mode lines the install printed.
    printed: RefCell<Vec<String>>,
}

impl FakeHost {
    fn new(managers: &[&str]) -> Self {
        FakeHost {
            managers: managers.iter().map(|m| (m.to_string(), format!("/usr/bin/{m}"))).collect(),
            discovered: BTreeMap::new(),
            json: true,
            yes: false,
            ran: RefCell::new(vec![]),
            exit_code: 0,
            captured: "ok\n".into(),
            printed: RefCell::new(vec![]),
        }
    }
    fn discovering(mut self, manager: &str, pkgs: &[&str]) -> Self {
        self.discovered.insert(manager.into(), ids(pkgs));
        self
    }
    fn yes(mut self) -> Self {
        self.yes = true;
        self
    }
}

impl Host for FakeHost {
    fn confirm(&self, _prompt: &str) -> bool {
        // json has no tty to confirm on, so only --yes says yes.
        self.yes
    }
    fn print(&self, line: &str) {
        self.printed.borrow_mut().push(line.to_string());
    }
    fn eprint(&self, _text: &str) {}
    fn exec(&self, req: &Exec) -> ExecOutcome {
        self.ran.borrow_mut().push(req.argv.clone());
        ExecOutcome { code: self.exit_code, captured: if req.capture == Capture::Inherit { String::new() } else { self.captured.clone() } }
    }
    fn managers(&self, _cat: &Catalogue) -> BTreeMap<String, String> {
        self.managers.clone()
    }
    fn discover_python(&self, manager: &str) -> Vec<String> {
        self.discovered.get(manager).cloned().unwrap_or_default()
    }
    fn json(&self) -> bool {
        self.json
    }
}

fn opts() -> InstallOpts {
    InstallOpts { ids: ids(&[DECOMPILE_PYTHON_ALL_ID]), ..Default::default() }
}

fn python_all(fx: &Fx, o: InstallOpts, host: &FakeHost) -> reactor_core::Result<(i32, reactor_core::install::PythonAllReport)> {
    let done = install(&fx.paths, &o, host)?;
    match done.report {
        InstallOutcome::PythonAll(r) => Ok((done.code, r)),
        InstallOutcome::Tools(_) => panic!("expected the pseudo-target's report"),
    }
}

fn sudo_prefix() -> Vec<String> {
    if reactor_core::util::is_root() { vec![] } else { ids(&["sudo"]) }
}

// -- TestDecompilePythonAll -------------------------------------------------

#[test]
fn not_a_catalogue_id_and_never_swept_into_install_all() {
    let fx = Fx::new();
    let cat = load_catalogue(&fx.paths).unwrap();
    assert!(!cat.contains(DECOMPILE_PYTHON_ALL_ID));
    assert!(!cat.order().contains(&DECOMPILE_PYTHON_ALL_ID.to_string()));
}

#[test]
fn method_flag_is_rejected() {
    let fx = Fx::new();
    let host = FakeHost::new(&["pacman"]);
    assert!(python_all(&fx, InstallOpts { method: Some("pip".into()), ..opts() }, &host).is_err());
}

#[test]
fn the_manual_flags_are_rejected_too() {
    let fx = Fx::new();
    let host = FakeHost::new(&["pacman"]);
    assert!(python_all(&fx, InstallOpts { auto_manual: true, ..opts() }, &host).is_err());
    assert!(python_all(&fx, InstallOpts { force_manual: true, ..opts() }, &host).is_err());
}

#[test]
fn no_supported_manager_is_an_error() {
    // uv is a real, present manager -- just never a python-version source.
    let fx = Fx::new();
    let (code, r) = python_all(&fx, opts(), &FakeHost::new(&["uv"])).unwrap();
    assert_eq!(code, 1);
    assert!(r.error.is_some());
}

#[test]
fn a_manager_with_nothing_discovered_is_an_error() {
    let fx = Fx::new();
    let (code, r) = python_all(&fx, opts(), &FakeHost::new(&["pacman"]).discovering("pacman", &[])).unwrap();
    assert_eq!(code, 1);
    assert!(r.packages.is_empty());
}

#[test]
fn aur_helpers_are_never_candidates_even_when_present() {
    // yay ranks nowhere near last in a real [platform].prefer, but it is not a
    // python-version source at all, which is what actually excludes it -- not a
    // ranking loss.
    let fx = Fx::new();
    let host = FakeHost::new(&["pacman", "yay"]).discovering("pacman", &["python"]);
    let (code, r) = python_all(&fx, InstallOpts { dry_run: true, ..opts() }, &host).unwrap();
    assert_eq!(code, 0);
    assert_eq!(r.manager.as_deref(), Some("pacman"));
}

#[test]
fn dry_run_reports_the_plan_and_runs_nothing() {
    let fx = Fx::new();
    let host = FakeHost::new(&["pacman"]).discovering("pacman", &["python"]);
    let (code, r) = python_all(&fx, InstallOpts { dry_run: true, ..opts() }, &host).unwrap();
    assert_eq!(code, 0);
    assert_eq!(r.packages, ["python"]);
    assert!(r.ran.is_empty());
    assert!(r.argv.unwrap().contains(&"pacman".to_string()));
    assert!(host.ran.borrow().is_empty(), "--dry-run must not execute anything");
}

#[test]
fn confirmed_run_installs_every_discovered_package_at_once() {
    let fx = Fx::new();
    let host = FakeHost::new(&["pacman"]).discovering("pacman", &["python", "python-extra"]).yes();
    let (code, r) = python_all(&fx, opts(), &host).unwrap();
    assert_eq!(code, 0);
    let mut expect = sudo_prefix();
    expect.extend(ids(&["pacman", "-S", "python", "python-extra"]));
    assert_eq!(host.ran.borrow().as_slice(), [expect]);
    assert_eq!(r.ran[0].returncode, 0);
}

#[test]
fn unconfirmed_run_installs_nothing() {
    let fx = Fx::new();
    let host = FakeHost::new(&["pacman"]).discovering("pacman", &["python"]);
    let (code, r) = python_all(&fx, opts(), &host).unwrap();
    assert_eq!(code, 1);
    assert!(r.ran.is_empty());
    assert!(host.ran.borrow().is_empty(), "an unconfirmed run must not execute anything");
}

#[test]
fn a_failing_install_exits_nonzero() {
    let fx = Fx::new();
    let mut host = FakeHost::new(&["pacman"]).discovering("pacman", &["python"]).yes();
    host.exit_code = 3;
    let (code, r) = python_all(&fx, opts(), &host).unwrap();
    assert_eq!(code, 1);
    assert_eq!(r.ran[0].returncode, 3);
}

// -- the ordinary flow --------------------------------------------------------

fn tools(fx: &Fx, o: InstallOpts, host: &FakeHost) -> reactor_core::Result<(i32, reactor_core::install::InstallReport)> {
    let done = install(&fx.paths, &o, host)?;
    match done.report {
        InstallOutcome::Tools(r) => Ok((done.code, r)),
        InstallOutcome::PythonAll(_) => panic!("expected an ordinary report"),
    }
}

fn o(id: &[&str]) -> InstallOpts {
    InstallOpts { ids: ids(id), ..Default::default() }
}

/// alpha has pacman+uv recipes and a `manual` note; nothing here is installed.
fn fx_absent() -> Fx {
    Fx::new()
}

#[test]
fn a_plan_is_built_from_the_ranked_recipe_and_nothing_runs_on_dry_run() {
    let fx = fx_absent();
    let host = FakeHost::new(&["pacman", "uv"]);
    let (code, r) = tools(&fx, InstallOpts { dry_run: true, ..o(&["alpha"]) }, &host).unwrap();
    assert_eq!(code, 0);
    assert_eq!(r.plan.len(), 1);
    assert_eq!(r.plan[0].method, "pacman");
    assert_eq!(r.plan[0].command, "pacman -S alpha");
    assert!(host.ran.borrow().is_empty());
}

#[test]
fn install_never_runs_without_confirmation() {
    // json mode has no tty to confirm on, so it must refuse rather than assume
    // yes. Every catalogued tool is a plausible `sudo pacman -S`.
    let fx = fx_absent();
    let host = FakeHost::new(&["pacman"]);
    let (code, r) = tools(&fx, o(&["alpha"]), &host).unwrap();
    assert_eq!(code, 1);
    assert!(r.ran.is_empty());
    assert!(r.message.is_some());
    assert!(host.ran.borrow().is_empty());
}

#[test]
fn a_confirmed_install_runs_the_argv_and_reports_it() {
    let fx = fx_absent();
    let host = FakeHost::new(&["pacman"]).yes();
    let (code, r) = tools(&fx, o(&["alpha"]), &host).unwrap();
    assert_eq!(code, 0);
    let mut expect = sudo_prefix();
    expect.extend(ids(&["pacman", "-S", "alpha"]));
    assert_eq!(host.ran.borrow().as_slice(), [expect]);
    assert_eq!(r.ran[0].returncode, 0);
    assert_eq!(r.ran[0].output.as_deref(), Some("ok"));
}

#[test]
fn free_text_keys_are_never_executed() {
    // With no manager present, alpha's only candidates are notes.
    let fx = fx_absent();
    let host = FakeHost::new(&[]).yes();
    let (_, r) = tools(&fx, o(&["alpha"]), &host).unwrap();
    assert!(r.plan.is_empty());
    assert!(r.skipped[0].reason.contains("no recipe for this machine"));
    assert!(host.ran.borrow().is_empty());
}

#[test]
fn all_covers_the_whole_catalogue_and_cannot_be_mixed() {
    let fx = fx_absent();
    let host = FakeHost::new(&["pacman"]);
    let (_, r) = tools(&fx, InstallOpts { dry_run: true, ..o(&["all"]) }, &host).unwrap();
    let covered: std::collections::BTreeSet<_> =
        r.plan.iter().map(|p| p.tool.clone()).chain(r.skipped.iter().map(|s| s.tool.clone())).collect();
    assert_eq!(covered.into_iter().collect::<Vec<_>>(), ["alpha", "beta", "gamma"]);

    let e = tools(&fx, InstallOpts { dry_run: true, ..o(&["all", "alpha"]) }, &host).unwrap_err();
    assert!(e.to_string().contains("all"), "{e}");
}

#[test]
fn method_picks_among_runnable_recipes() {
    let fx = fx_absent();
    let host = FakeHost::new(&["pacman", "uv"]);
    let (_, r) = tools(&fx, InstallOpts { dry_run: true, method: Some("uv".into()), ..o(&["alpha"]) }, &host).unwrap();
    assert_eq!(r.plan[0].method, "uv");
    let (_, r) = tools(&fx, InstallOpts { dry_run: true, method: Some("pip".into()), ..o(&["alpha"]) }, &host).unwrap();
    assert!(r.plan.is_empty());
    assert_eq!(r.skipped[0].reason, "no runnable pip recipe");
}

#[test]
fn an_unknown_tool_is_an_error_before_anything_runs() {
    let fx = fx_absent();
    let host = FakeHost::new(&["pacman"]).yes();
    let e = tools(&fx, o(&["ghost"]), &host).unwrap_err();
    assert!(e.to_string().contains("unknown tool(s): ghost"), "{e}");
    assert!(host.ran.borrow().is_empty());
}

#[test]
fn a_failed_install_exits_nonzero_and_is_reported() {
    let fx = fx_absent();
    let mut host = FakeHost::new(&["pacman"]).yes();
    host.exit_code = 1;
    let (code, r) = tools(&fx, o(&["alpha"]), &host).unwrap();
    assert_eq!(code, 1);
    assert_eq!(r.ran[0].returncode, 1);
}

const PIP_TOOL: &str = r#"
version = 1
[platform]
prefer = ["pip"]
[platform.manager]
pip = { binary = "pip3" }
[tool.p]
name = "P"
desc = "a pip tool"
invoke = "p"
detect = { binary = "reactor-absent-p" }
[tool.p.install]
pip = "pip3 install p"
"#;

#[test]
fn create_venv_redirects_pip_recipes_and_says_where_the_venv_will_be() {
    let fx = Fx::with(PIP_TOOL, FIXTURE_TOOLSETS);
    let host = FakeHost::new(&["pip"]);
    let (_, r) = tools(&fx, InstallOpts { dry_run: true, create_venv: true, ..o(&["p"]) }, &host).unwrap();
    let venv = fx.dir.path().join("venv");
    assert_eq!(r.venv.as_deref(), Some(venv.to_str().unwrap()));
    assert_eq!(r.plan[0].argv[0], venv.join("bin/pip").to_str().unwrap());
    assert!(!venv.exists(), "--dry-run must not create the venv");
}

#[test]
fn a_pep668_refusal_gets_an_actionable_hint() {
    let fx = Fx::with(PIP_TOOL, FIXTURE_TOOLSETS);
    let mut host = FakeHost::new(&["pip"]).yes();
    host.exit_code = 1;
    host.captured = "error: externally-managed-environment\n".into();
    let (code, r) = tools(&fx, o(&["p"]), &host).unwrap();
    assert_eq!(code, 1);
    assert!(r.ran[0].hint.as_deref().unwrap().contains("--create-venv p"));
}
