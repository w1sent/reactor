//! Startup (SPEC.md §3): resolve the launch flags, choose the workdir when nothing
//! is passed, show the native session picker for `-r`, then open the session window.
//!
//! One session per launch: the GUI never switches sessions at runtime. A session is a
//! directory under `~/.reactor/sessions/` (ADR-0036) — there is no shared store with
//! any other program to be polite to, so there are no pi flags to forward.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The launch flags.
#[derive(Debug, Clone, Default)]
pub struct LaunchArgs {
    /// The project directory; `None` shows the workdir chooser.
    pub cwd: Option<PathBuf>,
    /// `-c` / `--continue`: reopen the newest session for this directory.
    pub continue_recent: bool,
    /// `-r` / `--resume`: show the native session picker.
    pub resume: bool,
    /// `--session <dir>`: reopen this session directory.
    pub session: Option<PathBuf>,
    /// `--model provider/name`.
    pub model: Option<String>,
}

/// A fully resolved launch: the cwd is known.
#[derive(Debug, Clone)]
pub struct ResolvedLaunch {
    pub cwd: PathBuf,
    pub launch_args: LaunchArgs,
}

impl ResolvedLaunch {
    /// The session directory to reopen, if the flags name one.
    pub fn resume_dir(&self, paths: &reactor_core::Paths) -> Option<PathBuf> {
        if let Some(dir) = &self.launch_args.session {
            return Some(dir.clone());
        }
        if self.launch_args.continue_recent {
            return crate::backend::list_sessions(paths, Some(&self.cwd))
                .into_iter()
                .next()
                .map(|s| s.dir);
        }
        None
    }
}

/// Parse the GUI's arguments — a positional workdir plus the session flags.
pub fn parse_args(args: &[String]) -> LaunchArgs {
    let mut launch = LaunchArgs::default();
    let mut positional: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-c" | "--continue" => launch.continue_recent = true,
            "-r" | "--resume" => launch.resume = true,
            "--session" => {
                i += 1;
                launch.session = args.get(i).map(PathBuf::from);
            }
            "--model" | "-m" => {
                i += 1;
                launch.model = args.get(i).cloned();
            }
            other => {
                if !other.starts_with('-') && positional.is_none() {
                    positional = Some(PathBuf::from(other));
                }
            }
        }
        i += 1;
    }
    launch.cwd = positional;
    launch
}

/// The GUI's own config (SPEC.md §8): recent workdirs, and nothing else. The state
/// root is `~/.reactor/`; the old `~/.pi/reactor-gui.json` is read once if the new file
/// does not exist yet.
pub fn config_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".reactor")
        .join("gui.json")
}

fn legacy_config_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".pi")
        .join("reactor-gui.json")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GuiConfig {
    #[serde(default)]
    pub recent_workdirs: Vec<String>,
    /// How often each command has been run from the palette or the composer, for ranking.
    #[serde(default)]
    pub command_usage: std::collections::BTreeMap<String, u32>,
    /// Fonts and behaviour (`crate::settings`).
    #[serde(default)]
    pub ui: crate::settings::UiSettings,
}

impl GuiConfig {
    pub fn load() -> Self {
        [config_path(), legacy_config_path()]
            .iter()
            .find_map(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str::<GuiConfig>(&text).ok())
            .unwrap_or_default()
    }

    pub fn push_workdir(&mut self, cwd: &Path) {
        let path = cwd.display().to_string();
        self.recent_workdirs.retain(|entry| entry != &path);
        self.recent_workdirs.insert(0, path);
        self.recent_workdirs.truncate(8);
        self.save();
    }

    /// Count one run of a command. Loads first, so a second window's counts are not lost.
    pub fn record_command(key: &str) {
        let mut config = GuiConfig::load();
        *config.command_usage.entry(key.to_string()).or_insert(0) += 1;
        config.save();
    }

    /// The UI settings, normalized.
    pub fn load_ui() -> crate::settings::UiSettings {
        let mut ui = GuiConfig::load().ui;
        ui.normalize();
        ui
    }

    /// Store the UI settings. Loads first, so the recents and counts are kept.
    pub fn save_ui(ui: &crate::settings::UiSettings) {
        let mut config = GuiConfig::load();
        config.ui = ui.clone();
        config.save();
    }

    fn save(&self) {
        let file = config_path();
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(file, json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_positional_directory_and_the_session_flags_parse() {
        let l = parse_args(&args(&["/work/sample", "-c", "--model", "ollama/qwen"]));
        assert_eq!(l.cwd.as_deref(), Some(Path::new("/work/sample")));
        assert!(l.continue_recent && !l.resume);
        assert_eq!(l.model.as_deref(), Some("ollama/qwen"));

        let l = parse_args(&args(&["-r", "--session", "/s/1"]));
        assert!(l.resume && l.cwd.is_none());
        assert_eq!(l.session.as_deref(), Some(Path::new("/s/1")));
    }

    #[test]
    fn an_unknown_flag_is_ignored_rather_than_taken_for_a_directory() {
        let l = parse_args(&args(&["--no-such-flag", "/work"]));
        assert_eq!(l.cwd.as_deref(), Some(Path::new("/work")));
    }

    #[test]
    fn continue_reopens_the_newest_session_for_the_directory() {
        use reactor_agent::entry::Kind;
        use reactor_agent::store::Store;
        let dir = tempfile::tempdir().unwrap();
        let paths = reactor_core::Paths::new(dir.path(), reactor_core::paths::Shipped::Embedded);
        let make = |id: &str, cwd: &str, prompt: &str| {
            let mut s =
                Store::create(dir.path().join("sessions").join(id), id, Path::new(cwd)).unwrap();
            s.append(Kind::User {
                text: prompt.into(),
            })
            .unwrap();
        };
        make("a", "/work", "the older one");
        std::thread::sleep(std::time::Duration::from_millis(5));
        make("b", "/work", "the newer one");
        make("c", "/elsewhere", "a different project");

        let launch = ResolvedLaunch {
            cwd: "/work".into(),
            launch_args: LaunchArgs {
                continue_recent: true,
                ..Default::default()
            },
        };
        assert!(launch.resume_dir(&paths).unwrap().ends_with("sessions/b"));
        let listed = crate::backend::list_sessions(&paths, Some(Path::new("/work")));
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].first_prompt.as_deref(), Some("the newer one"));

        let named = ResolvedLaunch {
            cwd: "/work".into(),
            launch_args: LaunchArgs {
                session: Some("/x/y".into()),
                ..Default::default()
            },
        };
        assert_eq!(named.resume_dir(&paths).as_deref(), Some(Path::new("/x/y")));
        assert!(
            ResolvedLaunch {
                cwd: "/work".into(),
                launch_args: LaunchArgs::default()
            }
            .resume_dir(&paths)
            .is_none()
        );
    }
}
