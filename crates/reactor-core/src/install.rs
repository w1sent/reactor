//! `reactor install` — opt-in, never implicit (ADR-0010, ADR-0027).
//!
//! A command runs only when its package manager was verified present. Free-text
//! install keys (`manual`, a URL) are shown and never executed. The one
//! exception is `manual-install-oneliner`, and only under an explicit flag.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::catalogue::{Catalogue, Tool, load_catalogue, load_toolsets};
use crate::commands::select;
use crate::err;
use crate::error::Result;
use crate::host::{Capture, Exec, Host, discovery_run};
use crate::model::{ToolEntry, describe_using};
use crate::paths::Paths;
use crate::probe::{Cache, ProbeOpts, Status, probe};
use crate::recipes::{Recipe, shlex_join, shlex_split};
use crate::report::{Done, Report};
use crate::skills::fetch_skill;
use crate::state::load_state;
use crate::util::{home_dir, is_root, ljust, on_path, which};

/// pip's own wording for PEP 668 (a distro-managed Python refusing a
/// system-wide install). Matched against captured output so `install` can tell
/// this one well-known, actionable failure apart from "something else went
/// wrong".
pub const EXTERNALLY_MANAGED_MARKER: &str = "externally-managed-environment";

pub const DECOMPILE_PYTHON_ALL_ID: &str = "decompile-python[all]";

#[derive(Debug, Clone, Default)]
pub struct InstallOpts {
    pub ids: Vec<String>,
    pub method: Option<String>,
    pub dry_run: bool,
    pub create_venv: bool,
    pub auto_manual: bool,
    pub force_manual: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanItem {
    pub tool: String,
    pub method: String,
    pub command: String,
    pub sudo: bool,
    pub argv: Vec<String>,
}

impl PlanItem {
    fn text(&self) -> String {
        Recipe { method: self.method.clone(), command: self.command.clone(), sudo: self.sudo }.text()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub tool: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Ran {
    pub tool: String,
    pub command: String,
    pub returncode: i32,
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallReport {
    pub plan: Vec<PlanItem>,
    pub skipped: Vec<Skipped>,
    pub ran: Vec<Ran>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub venv: Option<String>,
    #[serde(skip)]
    human: String,
}

impl Report for InstallReport {
    fn human(&self) -> String {
        self.human.clone()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PythonAllReport {
    pub manager: Option<String>,
    pub packages: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    pub ran: Vec<Ran>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip)]
    human: String,
}

impl Report for PythonAllReport {
    fn human(&self) -> String {
        self.human.clone()
    }
}

/// What `install` answers with: the ordinary plan, or the pseudo-target's.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum InstallOutcome {
    Tools(InstallReport),
    PythonAll(PythonAllReport),
}

impl Report for InstallOutcome {
    fn human(&self) -> String {
        match self {
            InstallOutcome::Tools(r) => r.human(),
            InstallOutcome::PythonAll(r) => r.human(),
        }
    }
}

fn venv_exe(venv: &Path, name: &str) -> PathBuf {
    venv.join("bin").join(name)
}

fn tail20(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}

pub fn install(paths: &Paths, opts: &InstallOpts, host: &dyn Host) -> Result<Done<InstallOutcome>> {
    if opts.ids == [DECOMPILE_PYTHON_ALL_ID] {
        let d = install_python_all(paths, opts, host)?;
        return Ok(Done::code(InstallOutcome::PythonAll(d.report), d.code));
    }
    let d = install_tools(paths, opts, host)?;
    Ok(Done::code(InstallOutcome::Tools(d.report), d.code))
}

fn recipe_argv(recipe: &Recipe) -> Vec<String> {
    let mut argv = shlex_split(&recipe.command);
    if recipe.sudo && !is_root() {
        argv.insert(0, "sudo".into());
    }
    argv
}

fn install_tools(paths: &Paths, opts: &InstallOpts, host: &dyn Host) -> Result<Done<InstallReport>> {
    if opts.auto_manual && opts.force_manual {
        return Err(err!(
            "--auto-install-manual and --force-install-manual are mutually exclusive: \
             one makes the oneliner a fallback, the other an override"
        ));
    }
    if (opts.auto_manual || opts.force_manual) && opts.method.is_some() {
        return Err(err!(
            "--method picks among manager recipes; the manual flags pick the \
             manual-install-oneliner instead (ADR-0027)"
        ));
    }

    let cat = load_catalogue(paths)?;
    let toolsets = load_toolsets(paths)?;
    let state = load_state(paths)?;
    // `all` is the one reserved id: every catalogue entry, mirroring the `all`
    // toolset (toolsets.toml) so the same word means the same thing in both
    // places. It cannot be mixed with real ids -- there is nothing left for a
    // second id to narrow, so `install all jq` is rejected as an unknown tool
    // rather than silently doing what `install all` alone already does.
    let ids = if opts.ids == ["all"] { cat.order() } else { select(&cat, &opts.ids)? };
    let results = probe(paths, &cat, &ids, ProbeOpts { refresh: true, cached_only: false, services: false });
    let managers = host.managers(&cat);
    let entries = describe_using(paths, &cat, &toolsets, &state, &ids, &results, Some(&managers));

    // Chosen up front so the plan (and --dry-run's preview of it) can already
    // show pip recipes redirected here, even though the directory itself is
    // not created until the run is confirmed.
    let venv_dir = opts.create_venv.then(|| paths.config_dir.join("venv"));

    let (plan, skipped) = build_plan(&entries, opts, venv_dir.as_deref());

    let venv_str = venv_dir.as_ref().map(|d| d.display().to_string());
    if opts.dry_run || plan.is_empty() {
        let mut lines: Vec<String> = plan
            .iter()
            .map(|p| format!("would run  {} {}", ljust(&p.tool, 12), p.text()))
            .chain(skipped.iter().map(|s| format!("skip       {} {}", ljust(&s.tool, 12), s.reason)))
            .collect();
        if let Some(v) = &venv_str {
            lines.push(format!("venv       {v} (not created -- --dry-run)"));
        }
        let human = if lines.is_empty() { "nothing to do".to_string() } else { lines.join("\n") };
        return Ok(Done::ok(InstallReport { plan, skipped, ran: vec![], message: None, venv: venv_str, human }));
    }

    if !host.json() {
        // Never to stdout in json mode: that stream is the caller's input.
        host.print(&plan.iter().map(|p| format!("  {} {}", ljust(&p.tool, 12), p.text())).collect::<Vec<_>>().join("\n"));
    }
    if !host.confirm(&format!("run {} install command(s)?", plan.len())) {
        return Ok(Done::code(
            InstallReport {
                plan,
                skipped,
                ran: vec![],
                message: Some("not confirmed; re-run with --yes or --dry-run".into()),
                venv: None,
                human: "aborted -- re-run with --yes to execute, or --dry-run to see the plan".into(),
            },
            1,
        ));
    }

    if let Some(dir) = &venv_dir {
        let note = ensure_venv(&cat, dir, host).map_err(|why| err!("--create-venv: could not create {}: {why}", dir.display()))?;
        if !host.json() {
            host.print(&format!("venv  {} ({note})", dir.display()));
        }
    }

    let mut ran = Vec::new();
    for p in &plan {
        let tool = cat.get(&p.tool).expect("planned tools come from the catalogue");
        let mut cwd = None;
        let mut run_dir = PathBuf::new();
        if p.method == "manual" {
            // The oneliner's scratch dir doubles as its install dir (ADR-0027):
            // recreated per run so `git clone && ...` chains stay re-runnable,
            // and kept afterwards because the PATH promotion below symlinks
            // into it. Scripts like mac_apt.py need their clone's siblings, so
            // promotion symlinks rather than copies.
            run_dir = paths.config_dir.join("manual").join(&p.tool);
            let _ = std::fs::remove_dir_all(&run_dir);
            std::fs::create_dir_all(&run_dir).map_err(|e| err!("{}: {e}", run_dir.display()))?;
            cwd = Some(run_dir.clone());
        }
        // An installer writing to the inherited stdout would corrupt the
        // payload, so json captures it and hands back the tail. pip is the one
        // recipe kind with a well-known, worth-detecting failure (PEP 668), so
        // its stderr is captured rather than streamed live -- and, since that
        // means the user does not see it as it happens, echoed once the run is
        // over.
        let capture = if host.json() {
            Capture::Both
        } else if p.method == "pip" {
            Capture::Stderr
        } else {
            Capture::Inherit
        };
        let outcome = host.exec(&Exec { argv: p.argv.clone(), cwd, capture });
        let mut entry = Ran {
            tool: p.tool.clone(),
            command: p.text(),
            returncode: outcome.code,
            output: host.json().then(|| tail20(&outcome.captured)),
            path: None,
            hint: None,
        };
        if outcome.code != 0 && !host.json() && p.method == "pip" && !outcome.captured.is_empty() {
            let mut text = outcome.captured.clone();
            if !text.ends_with('\n') {
                text.push('\n');
            }
            host.eprint(&text);
        }
        if outcome.code == 0 && p.method == "manual" {
            let (link, hint) = promote_binary(tool, &run_dir);
            entry.path = link;
            entry.hint = hint;
        }
        if outcome.code != 0 && p.method == "pip" && outcome.captured.contains(EXTERNALLY_MANAGED_MARKER) {
            entry.hint = Some(format!(
                "this Python refuses system-wide pip installs (PEP 668). Re-run with \
                 `reactor install --create-venv {}` to install into a venv instead.",
                p.tool
            ));
        }
        ran.push(entry);
        if outcome.code == 0 && tool.skill.is_some() {
            let r = fetch_skill(paths, tool);
            if !host.json() && r.ok {
                host.print(&format!("fetched {} skill → {}", r.tool, r.message));
            }
        }
    }
    let mut cache = Cache::load(paths, &cat);
    cache.clear();
    cache.save();

    let failed = ran.iter().any(|r| r.returncode != 0);
    let mut lines: Vec<String> = ran
        .iter()
        .map(|r| {
            let mut s = format!("{}{} {}", if r.returncode == 0 { "ok   " } else { "FAIL " }, ljust(&r.tool, 12), r.command);
            if let Some(p) = &r.path {
                s.push_str(&format!("\n       path: {p}"));
            }
            if let Some(h) = &r.hint {
                s.push_str(&format!("\n       hint: {h}"));
            }
            s
        })
        .collect();
    if let Some(v) = &venv_str {
        lines.push(format!("venv       {v}"));
    }
    Ok(Done::code(
        InstallReport { plan, skipped, ran, message: None, venv: venv_str, human: lines.join("\n") },
        if failed { 1 } else { 0 },
    ))
}

fn build_plan(entries: &[ToolEntry], opts: &InstallOpts, venv_dir: Option<&Path>) -> (Vec<PlanItem>, Vec<Skipped>) {
    let (mut plan, mut skipped) = (Vec::new(), Vec::new());
    for e in entries {
        if e.status == Status::Present {
            skipped.push(Skipped { tool: e.id.clone(), reason: "already present".into() });
            continue;
        }
        let inst = e.install.as_ref().expect("described with install");
        let oneliner = inst.notes.get("manual-install-oneliner");
        let manual = |cmd: &String| Recipe {
            method: "manual".into(),
            command: cmd.clone(),
            // Never sudo-prefixed (ADR-0027): an upstream build that needs
            // root is what the `manual` note is for; the oneliner targets the
            // user's own prefix and asks for nothing it cannot do as itself.
            sudo: false,
        };
        let recipe: Option<Recipe> = if opts.force_manual {
            match oneliner {
                Some(cmd) => Some(manual(cmd)),
                None => {
                    skipped.push(Skipped { tool: e.id.clone(), reason: "no manual-install-oneliner".into() });
                    continue;
                }
            }
        } else if let Some(method) = &opts.method {
            match inst.candidates.iter().find(|c| &c.method == method) {
                Some(c) => Some(c.clone()),
                None => {
                    skipped.push(Skipped { tool: e.id.clone(), reason: format!("no runnable {method} recipe") });
                    continue;
                }
            }
        } else {
            match (&inst.recommended, opts.auto_manual, oneliner) {
                (Some(r), _, _) => Some(r.clone()),
                (None, true, Some(cmd)) => Some(manual(cmd)),
                _ => None,
            }
        };
        let Some(recipe) = recipe else {
            let mut reason = match inst.notes.iter().next() {
                Some((k, v)) => format!("no recipe for this machine; ({k}) {v}"),
                None => "no recipe for this machine".to_string(),
            };
            if oneliner.is_some() {
                reason.push_str(" -- a manual-install-oneliner exists; re-run with --auto-install-manual to use it");
            }
            skipped.push(Skipped { tool: e.id.clone(), reason });
            continue;
        };
        let argv = if recipe.method == "manual" {
            vec!["sh".to_string(), "-c".to_string(), recipe.command.clone()]
        } else {
            let mut argv = recipe_argv(&recipe);
            if let (Some(venv), "pip") = (venv_dir, recipe.method.as_str()) {
                // The one recipe kind PEP 668 can refuse. Redirecting to the
                // venv's own pip sidesteps that without needing sudo, which is
                // also why this never touches a `uv`/`pipx` recipe -- both
                // already manage their own isolated environment.
                argv[0] = venv_exe(venv, "pip").display().to_string();
            }
            argv
        };
        plan.push(PlanItem { tool: e.id.clone(), method: recipe.method, command: recipe.command, sudo: recipe.sudo, argv });
    }
    (plan, skipped)
}

/// Create `venv_dir` with the machine's Python unless it already looks like one.
/// → Ok(what happened) | Err(why). This is REactor asking the *tool ecosystem's*
/// interpreter to make an environment for pip-installed tools; REactor itself
/// does not run on Python.
fn ensure_venv(cat: &Catalogue, venv_dir: &Path, host: &dyn Host) -> std::result::Result<String, String> {
    if venv_exe(venv_dir, "pip").is_file() {
        return Ok("already exists".into());
    }
    let argv = vec![cat.python.clone(), "-m".into(), "venv".into(), venv_dir.display().to_string()];
    let out = host.exec(&Exec { argv: argv.clone(), cwd: None, capture: Capture::Both });
    if out.code != 0 || !venv_exe(venv_dir, "pip").is_file() {
        let tail = out.captured.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
        return Err(if tail.is_empty() { format!("`{} -m venv` exited {}", cat.python, out.code) } else { tail });
    }
    Ok("created".into())
}

/// After a manual oneliner ran, make the tool's binary reachable (ADR-0027).
///
/// Returns (symlink path, hint). One-liners build or unpack into their scratch
/// dir, which no probe will ever look at, so when the tool's detect binary is
/// still absent from PATH after the run, the scratch dir is searched and the
/// binary symlinked into ~/.local/bin -- the user-local PATH directory, created
/// on demand. A oneliner that installs onto PATH itself (the influx one does)
/// makes this a no-op.
pub fn promote_binary(tool: &Tool, run_dir: &Path) -> (Option<String>, Option<String>) {
    use crate::catalogue::DetectKind;
    if tool.detect_kind != DetectKind::Binary {
        return (None, None);
    }
    let name = &tool.detect_value;
    if which(name).is_some() {
        return (None, None);
    }
    let Some(found) = find_bounded(run_dir, name, 0) else {
        return (
            None,
            Some(format!(
                "{name} is not on PATH and was not found under {}; check where the one-liner put it and add that to PATH",
                run_dir.display()
            )),
        );
    };
    {
        // Scripts cloned from upstream are often not +x yet; a symlink to a
        // non-executable file is useless to `which`.
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&found) {
            let mut perm = meta.permissions();
            perm.set_mode(perm.mode() | 0o111);
            let _ = std::fs::set_permissions(&found, perm);
        }
    }
    let bin_dir = home_dir().join(".local").join("bin");
    let _ = std::fs::create_dir_all(&bin_dir);
    let link = bin_dir.join(name);
    if link.is_symlink() || link.exists() {
        let _ = std::fs::remove_file(&link);
    }
    let target = found.canonicalize().unwrap_or(found);
    if let Err(e) = std::os::unix::fs::symlink(&target, &link) {
        return (None, Some(format!("could not link {}: {e}", link.display())));
    }
    let hint = (!on_path(&bin_dir)).then(|| format!("{} is not on PATH; add it to pick up {name}", bin_dir.display()));
    (Some(link.display().to_string()), hint)
}

/// A bounded walk: a scratch dir can hold a whole build tree.
fn find_bounded(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    let mut subdirs = Vec::new();
    for e in &entries {
        let Ok(ty) = e.file_type() else { continue };
        if ty.is_dir() {
            subdirs.push(e.path());
        } else if e.file_name() == name {
            return Some(e.path());
        }
    }
    if depth >= 4 {
        return None;
    }
    subdirs.into_iter().find_map(|d| find_bounded(&d, name, depth + 1))
}

// ---------------------------------------------------------------------------
// `decompile-python[all]` -- every python this platform's own repos ship
// ---------------------------------------------------------------------------
//
// Not a catalogue id: `decompile-python` (tools.toml) is one tool with one
// normal install table. This is a single special-cased pseudo-target recognised
// before catalogue lookup, because what it does has no shape a
// `[tool.*.install]` table can express -- "however many packages a live repo
// query turns up", not one fixed command. It exists because the
// decompile-python skill has to run a .pyc against the *matching* interpreter
// version, and the more versions are on PATH, the more .pyc files that works for.
//
// Deliberately excluded from `reactor install all`: it is a lot of installed
// weight (several full interpreters) for one narrow skill, so it stays opt-in
// and separately named. The bracket is real syntax, borrowed from `pip install
// pkg[extra]`, and like pip's it needs quoting in most shells.

const PYTHON_ALL_MANAGERS: [&str; 4] = ["pacman", "apt", "dnf", "brew"];

/// The `python3.x` packages `manager`'s own repos offer right now. A manager not
/// in [`PYTHON_ALL_MANAGERS`] is never a candidate, which is what keeps AUR
/// helpers (paru, yay) out even when `reactor install` would otherwise prefer
/// them for other tools.
pub fn discover_python_packages(manager: &str) -> Vec<String> {
    let re = |p: &str| regex::Regex::new(p).unwrap();
    match manager {
        // Arch is rolling and its official repos carry exactly one blessed
        // python3 -- there is no "all versions" to enumerate here. Every
        // pythonNN package on Arch lives in the AUR, which this excludes.
        "pacman" => {
            if discovery_run(&["pacman", "-Si", "python"]).ok() { vec!["python".into()] } else { vec![] }
        }
        "apt" => {
            let r = discovery_run(&["apt-cache", "search", "--names-only", r"^python3\.[0-9]+$"]);
            if !r.ok() {
                return vec![];
            }
            let rx = re(r"^python3\.\d+$");
            let mut names: Vec<String> = Vec::new();
            for line in r.output.lines() {
                let name = line.split(' ').next().unwrap_or("").trim();
                if rx.is_match(name) && !names.iter().any(|n| n == name) {
                    names.push(name.to_string());
                }
            }
            names.sort();
            names
        }
        "dnf" => {
            let r = discovery_run(&["dnf", "--quiet", "list", "--available", "python3.*"]);
            if !r.ok() {
                return vec![];
            }
            let rx = re(r"^(python3\.\d+)\.");
            let mut names: Vec<String> = Vec::new();
            for line in r.output.lines() {
                // Strips the trailing .<arch>.
                let first = line.split_whitespace().next().unwrap_or("");
                if let Some(c) = rx.captures(first) {
                    let n = c[1].to_string();
                    if !names.contains(&n) {
                        names.push(n);
                    }
                }
            }
            names.sort();
            names
        }
        // A regex search, not a fixed list: Homebrew's versioned formulae
        // (python@3.9, python@3.10, ...) change as old ones are retired and new
        // ones land.
        "brew" => {
            let r = discovery_run(&["brew", "search", r"/^python@3\.[0-9]+$/"]);
            if !r.ok() {
                return vec![];
            }
            let rx = re(r"^python@3\.\d+$");
            let mut names: Vec<String> =
                r.output.lines().map(str::trim).filter(|l| rx.is_match(l)).map(str::to_string).collect();
            names.sort();
            names.dedup();
            names
        }
        _ => vec![],
    }
}

fn install_python_all(paths: &Paths, opts: &InstallOpts, host: &dyn Host) -> Result<Done<PythonAllReport>> {
    if opts.method.is_some() {
        return Err(err!(
            "--method does not apply to '{DECOMPILE_PYTHON_ALL_ID}'; it always uses \
             whichever supported package manager `[platform].prefer` ranks first"
        ));
    }
    if opts.auto_manual || opts.force_manual {
        return Err(err!(
            "--auto-install-manual/--force-install-manual do not apply to \
             '{DECOMPILE_PYTHON_ALL_ID}'; it has no manual path -- its manual note \
             describes the platform's own repos, which the default path already uses"
        ));
    }

    let cat = load_catalogue(paths)?;
    let present = host.managers(&cat);
    let rank = |m: &str| cat.prefer.iter().position(|p| p == m).unwrap_or(cat.prefer.len());
    let mut candidates: Vec<&String> = present.keys().filter(|m| PYTHON_ALL_MANAGERS.contains(&m.as_str())).collect();
    candidates.sort_by_key(|m| rank(m));
    let fail = |manager: Option<String>, packages: Vec<String>, message: String| {
        Done::code(
            PythonAllReport {
                manager,
                packages,
                argv: None,
                ran: vec![],
                error: Some(message.clone()),
                message: None,
                human: message,
            },
            1,
        )
    };
    let Some(manager) = candidates.first().map(|m| m.to_string()) else {
        return Ok(fail(
            None,
            vec![],
            format!(
                "no supported package manager on this machine can discover python versions ({})",
                { let mut v = PYTHON_ALL_MANAGERS.to_vec(); v.sort(); v.join(", ") }
            ),
        ));
    };
    let packages = host.discover_python(&manager);
    if packages.is_empty() {
        return Ok(fail(
            Some(manager.clone()),
            vec![],
            format!("{manager}: found no python3.x packages in the platform's own repos"),
        ));
    }

    let m = &cat.managers[&manager];
    let verb = match manager.as_str() {
        "pacman" => "-S",
        _ => "install",
    };
    let mut argv = vec![manager.clone(), verb.to_string()];
    argv.extend(packages.iter().cloned());
    if m.sudo && !is_root() {
        argv.insert(0, "sudo".into());
    }
    let command_text = shlex_join(&argv);

    let report = |argv: Option<Vec<String>>, ran: Vec<Ran>, message: Option<String>, human: String| PythonAllReport {
        manager: Some(manager.clone()),
        packages: packages.clone(),
        argv,
        ran,
        error: None,
        message,
        human,
    };

    if opts.dry_run {
        return Ok(Done::ok(report(Some(argv), vec![], None, format!("would run  {command_text}"))));
    }
    if !host.json() {
        host.print(&format!("  {command_text}"));
    }
    if !host.confirm(&format!("install {} python version(s) via {manager}?", packages.len())) {
        return Ok(Done::code(
            report(
                Some(argv),
                vec![],
                Some("not confirmed; re-run with --yes or --dry-run".into()),
                "aborted -- re-run with --yes to execute, or --dry-run to see the plan".into(),
            ),
            1,
        ));
    }
    let outcome = host.exec(&Exec {
        argv: argv.clone(),
        cwd: None,
        capture: if host.json() { Capture::Both } else { Capture::Inherit },
    });
    let mut cache = Cache::load(paths, &cat);
    cache.clear();
    cache.save();
    let ran = Ran {
        tool: DECOMPILE_PYTHON_ALL_ID.into(),
        command: command_text.clone(),
        returncode: outcome.code,
        output: host.json().then(|| tail20(&outcome.captured)),
        path: None,
        hint: None,
    };
    let human = format!("{}{command_text}", if outcome.code == 0 { "ok   " } else { "FAIL " });
    Ok(Done::code(report(Some(argv), vec![ran], None, human), if outcome.code != 0 { 1 } else { 0 }))
}
