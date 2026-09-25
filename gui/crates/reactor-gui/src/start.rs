//! Startup (gui/SPEC.md §3): resolve the launch flags, choose the workdir
//! when nothing is passed, show the native session picker for `-r`, then
//! open the session window.
//!
//! The GUI never switches sessions at runtime — pi's own model: one session
//! per launch, chosen by the same flags pi itself takes, passed through
//! one-to-one to `pi --mode rpc`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The launch flags the GUI accepts — pi's own session model, mirrored
/// (gui/SPEC.md §3).
#[derive(Debug, Clone, Default)]
pub struct LaunchArgs {
    /// The project directory; `None` shows the workdir chooser.
    pub cwd: Option<PathBuf>,
    /// `-c` / `--continue`.
    pub continue_recent: bool,
    /// `-r` / `--resume` — the GUI shows its native session picker, then
    /// spawns pi with `--session <path>`.
    pub resume: bool,
    pub session: Option<String>,
    pub session_dir: Option<PathBuf>,
    pub no_session: bool,
    pub name: Option<String>,
    pub fork: Option<String>,
}

impl LaunchArgs {
    /// Whether this launch attaches to a session that may already hold
    /// entries — `-c`/`--continue`, `--session`, `--fork` all reopen
    /// something that already exists, unlike a bare `reactor-gui <dir>` or
    /// `--no-session`, which always start with zero entries. The GUI backfills
    /// history (`get_entries`) only for the former: doing it unconditionally
    /// raced the live event stream on a brand-new session (see the call site
    /// in `ReactorApp::open`).
    pub fn resumes_existing_session(&self) -> bool {
        self.continue_recent || self.session.is_some() || self.fork.is_some()
    }

    /// The `pi --mode rpc …` command line, built one-to-one from the flags.
    pub fn pi_args(&self) -> Vec<String> {
        let mut args = vec!["--mode".to_owned(), "rpc".to_owned()];
        if self.continue_recent {
            args.push("--continue".into());
        }
        if let Some(session) = &self.session {
            args.push("--session".into());
            args.push(session.clone());
        }
        if let Some(dir) = &self.session_dir {
            args.push("--session-dir".into());
            args.push(dir.display().to_string());
        }
        if self.no_session {
            args.push("--no-session".into());
        }
        if let Some(name) = &self.name {
            args.push("--name".into());
            args.push(name.clone());
        }
        if let Some(fork) = &self.fork {
            args.push("--fork".into());
            args.push(fork.clone());
        }
        args
    }
}

/// A fully resolved launch: the cwd is known, the pi flags are final.
#[derive(Debug, Clone)]
pub struct ResolvedLaunch {
    pub cwd: PathBuf,
    pub launch_args: LaunchArgs,
}

/// Parse the GUI's arguments — a positional workdir plus pi's session flags
/// (`-c`, `-r`, `--session`, `--session-dir`, `--no-session`, `--name`,
/// `--fork`), forwarded untouched (gui/SPEC.md §3).
pub fn parse_args(args: &[String]) -> LaunchArgs {
    let mut launch = LaunchArgs::default();
    let mut positional: Option<PathBuf> = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "-c" | "--continue" => launch.continue_recent = true,
            "-r" | "--resume" => launch.resume = true,
            "--session" => {
                index += 1;
                launch.session = args.get(index).cloned();
            }
            "--session-dir" => {
                index += 1;
                launch.session_dir = args.get(index).map(PathBuf::from);
            }
            "--no-session" => launch.no_session = true,
            "-n" | "--name" => {
                index += 1;
                launch.name = args.get(index).cloned();
            }
            "--fork" => {
                index += 1;
                launch.fork = args.get(index).cloned();
            }
            other => {
                if !other.starts_with('-') && positional.is_none() {
                    positional = Some(PathBuf::from(other));
                }
            }
        }
        index += 1;
    }
    if let Some(dir) = positional {
        launch.cwd = Some(dir);
    }
    launch
}

/// The GUI's own config (gui/SPEC.md §8): recent workdirs, keyed by nothing
/// else — not the CLI's `~/.pi/reactor/`, not pi's `~/.pi/agent/`.
pub fn config_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".pi")
        .join("reactor-gui.json")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GuiConfig {
    #[serde(default)]
    pub recent_workdirs: Vec<String>,
}

impl GuiConfig {
    pub fn load() -> Self {
        std::fs::read_to_string(config_path())
            .ok()
            .and_then(|text| serde_json::from_str::<GuiConfig>(&text).ok())
            .unwrap_or_default()
    }

    pub fn push_workdir(&mut self, cwd: &Path) {
        let path = cwd.display().to_string();
        self.recent_workdirs.retain(|entry| entry != &path);
        self.recent_workdirs.insert(0, path);
        self.recent_workdirs.truncate(8);
        let dir = config_path();
        if let Some(parent) = dir.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(config_path(), json);
        }
    }
}

/// The pi-parallel encoding of a cwd into the session store's directory name,
/// read out of the shipped `dist/core/session-manager.js` (see "Facts for
/// reactor-gui" in `docs/pi-api-notes.md`): `--{cwd sans leading /, with
/// slashes/colons as -}--`.
pub fn sessions_dir_for(cwd: &Path, agent_dir: &Path) -> PathBuf {
    let cwd = cwd
        .display()
        .to_string()
        .trim_start_matches(['/', '\\'])
        .replace(['/', '\\', ':'], "-");
    agent_dir.join("sessions").join(format!("--{cwd}--"))
}

/// List one project's sessions the way pi's `-r` picker would: the session
/// files under the cwd's store, headers read for id, name and date.
pub fn list_sessions(cwd: &Path) -> Vec<SessionSummary> {
    let agent_dir = std::env::var("HOME")
        .map(|home| PathBuf::from(home).join(".pi").join("agent"))
        .unwrap_or_default();
    let dir = sessions_dir_for(cwd, &agent_dir);
    let mut sessions = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return sessions;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        // The header is the first line: `{"type":"session","version":…,…}` —
        // the only line the picker reads, never the whole file.
        if let Ok(first) = read_first_line(&path) {
            if let Ok(header) = serde_json::from_str::<SessionHeader>(&first) {
                sessions.push(SessionSummary {
                    path: path.display().to_string(),
                    id: header.id,
                    name: header.name,
                    timestamp: header.timestamp,
                });
            }
        }
    }
    sessions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    sessions
}

/// The first line of a file, as bytes — the session header.
fn read_first_line(path: &Path) -> std::io::Result<String> {
    use std::io::{BufRead, Read};
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let mut line = String::new();
    reader.take(16 * 1024).read_line(&mut line)?;
    Ok(line)
}

#[derive(Debug, Clone, Deserialize)]
struct SessionHeader {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
}

/// One row of the session picker.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    pub path: String,
    pub id: String,
    pub name: Option<String>,
    pub timestamp: Option<String>,
}
