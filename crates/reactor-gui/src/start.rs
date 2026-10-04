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
    /// `--session <id>`: reopen this session of the directory's case.
    pub session: Option<String>,
    /// `--model provider/name`.
    pub model: Option<String>,
    /// `--save-case`: save the directory's unsaved state as its case before opening. What
    /// *Save as Case* relaunches with — the case can only be moved while no window has it.
    pub save_case: bool,
}

/// A fully resolved launch: the cwd is known.
#[derive(Debug, Clone)]
pub struct ResolvedLaunch {
    pub cwd: PathBuf,
    pub launch_args: LaunchArgs,
}

impl ResolvedLaunch {
    /// The session to reopen, if the flags name one.
    pub fn resume_session(&self, paths: &reactor_core::Paths) -> Option<String> {
        if let Some(id) = &self.launch_args.session {
            return Some(id.clone());
        }
        if self.launch_args.continue_recent {
            return crate::backend::list_sessions(paths, &self.cwd)
                .into_iter()
                .next()
                .map(|s| s.id);
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
            "--save-case" => launch.save_case = true,
            "--session" => {
                i += 1;
                // `3`, `#3` and `<case id>#3` all name the case's third session.
                launch.session = args.get(i).map(|s| crate::session_list::parse_reference(s));
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

        let l = parse_args(&args(&["-r", "--session", "1759500000-42"]));
        assert!(l.resume && l.cwd.is_none());
        assert_eq!(l.session.as_deref(), Some("1759500000-42"));
    }

    #[test]
    fn an_unknown_flag_is_ignored_rather_than_taken_for_a_directory() {
        let l = parse_args(&args(&["--no-such-flag", "/work"]));
        assert_eq!(l.cwd.as_deref(), Some(Path::new("/work")));
    }

    #[test]
    fn continue_reopens_the_newest_session_of_the_directorys_case() {
        use reactor_agent::entry::Kind;
        use reactor_agent::store::Store;
        use reactor_db::Case;
        let (config, work, elsewhere, plain) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let paths = reactor_core::Paths::new(config.path(), reactor_core::paths::Shipped::Embedded);
        let make = |case: &Case, id: &str, prompt: &str| {
            let mut s = Store::create(case, id).unwrap();
            s.append(Kind::User {
                text: prompt.into(),
            })
            .unwrap();
        };
        // A saved case with two sessions, and another folder's unsaved state with one.
        let case = Case::create(work.path(), None).unwrap();
        make(&case, "a", "the older one");
        std::thread::sleep(std::time::Duration::from_millis(5));
        make(&case, "b", "the newer one");
        drop(case);
        let other = Case::open_any(elsewhere.path(), config.path()).unwrap();
        make(&other, "c", "a different folder");
        drop(other);

        let launch = |cwd: &Path, args: LaunchArgs| ResolvedLaunch {
            cwd: cwd.to_path_buf(),
            launch_args: args,
        };
        let cont = LaunchArgs {
            continue_recent: true,
            ..Default::default()
        };
        assert_eq!(
            launch(work.path(), cont.clone())
                .resume_session(&paths)
                .as_deref(),
            Some("b")
        );
        let listed = crate::backend::list_sessions(&paths, work.path());
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].first_prompt.as_deref(), Some("the newer one"));
        // An unsaved state has sessions too.
        assert_eq!(
            launch(elsewhere.path(), cont.clone())
                .resume_session(&paths)
                .as_deref(),
            Some("c")
        );

        let named = LaunchArgs {
            session: Some("a".into()),
            ..Default::default()
        };
        assert_eq!(
            launch(work.path(), named).resume_session(&paths).as_deref(),
            Some("a")
        );
        assert!(
            launch(work.path(), LaunchArgs::default())
                .resume_session(&paths)
                .is_none()
        );

        // Asking about a folder that is neither does not make it anything.
        assert!(crate::backend::list_sessions(&paths, plain.path()).is_empty());
        assert!(launch(plain.path(), cont).resume_session(&paths).is_none());
        assert!(!Case::has_unsaved(plain.path(), config.path()));
        assert_eq!(std::fs::read_dir(plain.path()).unwrap().count(), 0);
    }
}
