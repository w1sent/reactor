//! The installed config against the shipped one (ADR-0004), and first-time setup.
//!
//! Config is the user's once seeded: nothing here ever overwrites it except
//! `overwrite_config`, which backs up first and asks.

use std::path::PathBuf;

use serde::Serialize;

use crate::catalogue::load_catalogue;
use crate::completion;
use crate::err;
use crate::error::Result;
use crate::host::Host;
use crate::json::{io_reason, write_json_atomic};
use crate::paths::{CONFIG_FILES, Paths, Shipped};
use crate::report::{Done, Report};
use crate::skills::{fetch_skill, tempdir};
use crate::state::StateDoc;
use crate::util::{home_dir, on_path, run, which};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigDrift {
    Same,
    Differs,
    Absent,
}

pub fn config_drift(paths: &Paths, name: &str) -> ConfigDrift {
    let live = paths.live(name);
    if !live.is_file() {
        return ConfigDrift::Absent;
    }
    if !paths.shipped_exists(name) {
        return ConfigDrift::Same;
    }
    match (paths.shipped_bytes(name), std::fs::read(&live)) {
        (Ok(a), Ok(b)) if a == b => ConfigDrift::Same,
        _ => ConfigDrift::Differs,
    }
}

fn names_for(file: Option<&str>) -> Vec<String> {
    match file {
        Some(f) => vec![format!("{f}.toml")],
        None => CONFIG_FILES.iter().map(|s| s.to_string()).collect(),
    }
}

// ---------------------------------------------------------------------------
// diff-config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct DiffFile {
    pub file: String,
    pub shipped: String,
    pub installed: String,
    pub state: ConfigDrift,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffReport {
    pub files: Vec<DiffFile>,
    pub differ: bool,
    pub diff: String,
}

impl Report for DiffReport {
    fn human(&self) -> String {
        if self.diff.is_empty() { "no differences".into() } else { self.diff.clone() }
    }
}

pub fn diff_config(paths: &Paths, file: Option<&str>) -> Result<Done<DiffReport>> {
    if which("diff").is_none() {
        return Err(err!("diff(1) is not on PATH"));
    }
    let (mut files, mut chunks, mut differ) = (Vec::new(), Vec::new(), false);
    for name in names_for(file) {
        let live = paths.live(&name);
        let state = config_drift(paths, &name);
        files.push(DiffFile {
            file: name.clone(),
            shipped: paths.shipped_display(&name),
            installed: live.display().to_string(),
            state,
        });
        if state == ConfigDrift::Absent {
            chunks.push(format!("--- {name}: not installed; run `reactor setup`"));
            continue;
        }
        // diff(1) wants a file, and a compiled-in copy is not one.
        let (shipped_path, scratch) = match &paths.shipped {
            Shipped::Dir(root) => (root.join(&name), None),
            Shipped::Embedded => {
                let dir = tempdir("reactor-diff-")?;
                let p = dir.join(&name);
                std::fs::write(&p, paths.shipped_bytes(&name)?).map_err(|e| err!("{}: {}", p.display(), io_reason(&e)))?;
                (p, Some(dir))
            }
        };
        let r = run(
            &[
                "diff".into(),
                "-u".into(),
                "--label".into(),
                format!("shipped/{name}"),
                "--label".into(),
                format!("installed/{name}"),
                shipped_path.display().to_string(),
                live.display().to_string(),
            ],
            std::time::Duration::from_secs(30),
        );
        if let Some(d) = scratch {
            let _ = std::fs::remove_dir_all(d);
        }
        if r.code != Some(0) {
            differ = true;
            chunks.push(r.output.trim_end().to_string());
        }
    }
    let diff = chunks.join("\n");
    Ok(Done::code(DiffReport { files, differ, diff }, if differ { 1 } else { 0 }))
}

// ---------------------------------------------------------------------------
// overwrite-config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct OverwriteReport {
    pub replaced: Vec<String>,
    pub backups: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip)]
    human: String,
}

impl Report for OverwriteReport {
    fn human(&self) -> String {
        self.human.clone()
    }
}

fn timestamp() -> String {
    // SAFETY: localtime_r/strftime write only into the buffers handed to them.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let mut buf = [0u8; 32];
        let n = libc::strftime(buf.as_mut_ptr() as *mut libc::c_char, buf.len(), c"%Y%m%d-%H%M%S".as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}

pub fn overwrite_config(paths: &Paths, file: Option<&str>, host: &dyn Host) -> Result<Done<OverwriteReport>> {
    let plan: Vec<String> = names_for(file).into_iter().filter(|n| config_drift(paths, n) != ConfigDrift::Same).collect();
    if plan.is_empty() {
        return Ok(Done::ok(OverwriteReport {
            replaced: vec![],
            backups: vec![],
            message: None,
            human: "already identical to the shipped copies".into(),
        }));
    }
    let prompt = format!("replace {} in {} with the shipped copies?", plan.join(", "), paths.config_dir.display());
    if !host.confirm(&prompt) {
        return Ok(Done::code(
            OverwriteReport {
                replaced: vec![],
                backups: vec![],
                message: Some("not confirmed".into()),
                human: "aborted -- re-run with --yes".into(),
            },
            1,
        ));
    }
    let (mut replaced, mut backups) = (Vec::new(), Vec::new());
    for name in &plan {
        let live = paths.live(name);
        if live.is_file() {
            let backup = live.with_file_name(format!("{name}.{}.bak", timestamp()));
            std::fs::copy(&live, &backup).map_err(|e| err!("{}: {}", backup.display(), io_reason(&e)))?;
            backups.push(backup.display().to_string());
        }
        std::fs::create_dir_all(&paths.config_dir).map_err(|e| err!("{}: {}", paths.config_dir.display(), io_reason(&e)))?;
        std::fs::write(&live, paths.shipped_bytes(name)?).map_err(|e| err!("{}: {}", live.display(), io_reason(&e)))?;
        replaced.push(name.clone());
    }
    let human = format!(
        "replaced: {}{}",
        replaced.join(", "),
        if backups.is_empty() { String::new() } else { format!("\nbackups: {}", backups.join(", ")) }
    );
    Ok(Done::ok(OverwriteReport { replaced, backups, message: None, human }))
}

// ---------------------------------------------------------------------------
// setup
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct SetupOpts {
    pub skills: bool,
    pub completions: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetupReport {
    pub actions: Vec<String>,
    pub warnings: Vec<String>,
}

impl Report for SetupReport {
    fn human(&self) -> String {
        let mut out: Vec<String> = self.actions.clone();
        out.extend(self.warnings.iter().map(|w| format!("  ! {w}")));
        out.push(String::new());
        if !self.warnings.is_empty() {
            out.push(format!("{} warning(s). REactor still works -- see above.", self.warnings.len()));
        }
        out.push("Next: `reactor doctor` for what is present and what is missing.".into());
        out.join("\n")
    }
}

/// Where each shell's completion file goes, in the conventional per-user
/// location for that shell. bash and fish need no shell config change to pick
/// these up; zsh needs `~/.zfunc` on `fpath` before `compinit`, which setup
/// reminds about rather than editing `.zshrc`.
fn completion_target(shell: &str) -> PathBuf {
    let home = home_dir();
    let xdg = |var: &str, fallback: PathBuf| {
        std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or(fallback)
    };
    match shell {
        "bash" => xdg("XDG_DATA_HOME", home.join(".local/share")).join("bash-completion/completions/reactor"),
        "zsh" => home.join(".zfunc/_reactor"),
        _ => xdg("XDG_CONFIG_HOME", home.join(".config")).join("fish/completions/reactor.fish"),
    }
}

/// Seed `~/.reactor/` from the shipped copies, fetch upstream skills, install
/// shell completions. Idempotent; re-running is the supported way to update.
/// Seeding never clobbers an existing config file: if yours differs from the
/// shipped copy it says so and points at `reactor diff-config` (ADR-0004).
pub fn setup(paths: &Paths, opts: SetupOpts) -> Result<Done<SetupReport>> {
    let (mut actions, mut warnings) = (Vec::new(), Vec::new());
    let would = if opts.dry_run { "would " } else { "" };

    actions.push("config".to_string());
    for name in CONFIG_FILES {
        let live = paths.live(name);
        match config_drift(paths, name) {
            ConfigDrift::Absent => {
                actions.push(format!("{would}seed {}", live.display()));
                if !opts.dry_run {
                    let bytes = paths.shipped_bytes(name)?;
                    std::fs::create_dir_all(&paths.config_dir)
                        .and_then(|_| std::fs::write(&live, bytes))
                        .map_err(|e| err!("{}: {}", live.display(), io_reason(&e)))?;
                }
            }
            // Never clobber: this is the user's file now (ADR-0003/0004).
            ConfigDrift::Differs => warnings.push(format!("{name} differs from the shipped copy -- `reactor diff-config`")),
            ConfigDrift::Same => actions.push(format!("  {} (up to date)", live.display())),
        }
    }
    let state = paths.state_file();
    if !state.exists() {
        actions.push(format!("{would}create {}", state.display()));
        if !opts.dry_run {
            let empty = StateDoc {
                version: 1,
                toolsets: vec![],
                tools: crate::state::StateTools { enabled: vec![], disabled: vec![] },
            };
            write_json_atomic(&state, &empty)?;
        }
    } else {
        actions.push(format!("  {} (kept)", state.display()));
    }

    actions.push("skills".to_string());
    if !opts.skills {
        actions.push("  skipped (--no-skills)".into());
    } else if opts.dry_run {
        actions.push("  skipped (--dry-run)".into());
    } else {
        // Only a *configured* skill that could not be fetched is a warning. A
        // tool with no configured skill is the normal case and says nothing
        // (ADR-0008).
        let cat = load_catalogue(paths)?;
        for tool in cat.tools.iter().filter(|t| t.skill.is_some()) {
            let r = fetch_skill(paths, tool);
            if r.ok {
                actions.push(format!("  {} skill → {}", r.tool, r.message));
            } else {
                warnings.push(format!("{} skill not fetched: {}", r.tool, r.message));
            }
        }
    }

    actions.push("completions".to_string());
    if !opts.completions {
        actions.push("  skipped (--no-completions)".into());
    } else {
        for shell in completion::SHELLS {
            let dest = completion_target(shell);
            let script = completion::script(shell).expect("shipped shell");
            if std::fs::read_to_string(&dest).is_ok_and(|s| s == script) {
                actions.push(format!("  {} (up to date)", dest.display()));
                continue;
            }
            actions.push(format!("{would}write {}", dest.display()));
            if !opts.dry_run {
                let wrote = dest
                    .parent()
                    .map(std::fs::create_dir_all)
                    .transpose()
                    .and_then(|_| std::fs::write(&dest, script));
                if let Err(e) = wrote {
                    warnings.push(format!("{shell} completion: {}", io_reason(&e)));
                }
            }
        }
        let zfunc = completion_target("zsh");
        actions.push(format!(
            "  zsh: add `fpath+=({})` before `compinit` in .zshrc if not already there",
            zfunc.parent().unwrap().display()
        ));
    }

    if let Some(exe) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()))
        && !on_path(&exe)
    {
        warnings.push(format!("{} is not on your PATH", exe.display()));
    }
    Ok(Done::ok(SetupReport { actions, warnings }))
}
