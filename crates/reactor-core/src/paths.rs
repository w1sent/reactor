//! Where REactor's files live.
//!
//! Two roots. The *config dir* (`~/.reactor/` by default, `REACTOR_CONFIG_DIR`
//! to override) is the user's: their `tools.toml`, `state.json`, the probe cache,
//! fetched skills. The *shipped* copies of `tools.toml`/`toolsets.toml` are what
//! this build was released with — compiled in, so a bare binary can seed a
//! config and `doctor` can run before anything is installed. `REACTOR_PACKAGE_ROOT`
//! points at a checkout instead, for development and for tests.

use std::path::{Path, PathBuf};

use crate::error::{ReactorError, Result};
use crate::json::io_reason;
use crate::util::home_dir;

pub const CONFIG_FILES: [&str; 2] = ["tools.toml", "toolsets.toml"];

const SHIPPED_TOOLS: &str = include_str!("../../../tools.toml");
const SHIPPED_TOOLSETS: &str = include_str!("../../../toolsets.toml");

#[derive(Debug, Clone)]
pub enum Shipped {
    /// The copies compiled into this binary.
    Embedded,
    /// A checkout on disk.
    Dir(PathBuf),
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub shipped: Shipped,
    /// Where to look for a project-scoped `.reactor/state.json` (ADR-0003).
    /// `None` means no project scope — the library reads no ambient process
    /// state, so tests and embedders decide.
    pub cwd: Option<PathBuf>,
    /// A per-session activation file (ADR-0038: the GUI writes session scope by
    /// default). When the file exists it wins over the project and machine state and
    /// is reported as scope `session`; activation edits go *there*, so one session's
    /// toggles never touch another's. `None` means no session scope.
    pub session_state: Option<PathBuf>,
}

impl Paths {
    pub fn new(config_dir: impl Into<PathBuf>, shipped: Shipped) -> Self {
        Self { config_dir: config_dir.into(), shipped, cwd: None, session_state: None }
    }

    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn with_session_state(mut self, file: impl Into<PathBuf>) -> Self {
        self.session_state = Some(file.into());
        self
    }

    /// The process environment's answer. Performs the one-time
    /// `~/.pi/reactor` → `~/.reactor` move when the default location is in
    /// play (an explicit `REACTOR_CONFIG_DIR` is always taken as written).
    pub fn from_env() -> Self {
        let config_dir = match std::env::var_os("REACTOR_CONFIG_DIR").filter(|v| !v.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            None => {
                let home = home_dir();
                let dir = home.join(".reactor");
                migrate_legacy_root(&home.join(".pi").join("reactor"), &dir);
                dir
            }
        };
        let shipped = match std::env::var_os("REACTOR_PACKAGE_ROOT").filter(|v| !v.is_empty()) {
            Some(root) => Shipped::Dir(PathBuf::from(root)),
            None => Shipped::Embedded,
        };
        Self { config_dir, shipped, cwd: std::env::current_dir().ok(), session_state: None }
    }

    pub fn live(&self, name: &str) -> PathBuf {
        self.config_dir.join(name)
    }

    pub fn skills_dir(&self) -> PathBuf {
        self.config_dir.join("skills")
    }

    pub fn state_file(&self) -> PathBuf {
        self.config_dir.join("state.json")
    }

    pub fn cache_file(&self) -> PathBuf {
        self.config_dir.join("cache.json")
    }

    /// The shipped copy of `name` as bytes.
    pub fn shipped_bytes(&self, name: &str) -> Result<Vec<u8>> {
        match &self.shipped {
            Shipped::Embedded => match name {
                "tools.toml" => Ok(SHIPPED_TOOLS.as_bytes().to_vec()),
                "toolsets.toml" => Ok(SHIPPED_TOOLSETS.as_bytes().to_vec()),
                other => Err(ReactorError::new(format!("no shipped file named {other}"))),
            },
            Shipped::Dir(root) => {
                let p = root.join(name);
                std::fs::read(&p).map_err(|e| ReactorError::new(format!("{}: {}", p.display(), io_reason(&e))))
            }
        }
    }

    /// Where the shipped copy is, for messages. Compiled-in copies have no path.
    pub fn shipped_display(&self, name: &str) -> String {
        match &self.shipped {
            Shipped::Embedded => format!("<built-in>/{name}"),
            Shipped::Dir(root) => root.join(name).display().to_string(),
        }
    }

    pub fn shipped_exists(&self, name: &str) -> bool {
        match &self.shipped {
            Shipped::Embedded => CONFIG_FILES.contains(&name),
            Shipped::Dir(root) => root.join(name).is_file(),
        }
    }
}

/// Move the pre-Rust state root to its new home, once. Never overwrites: if the
/// new root exists, the old one is left alone for the user to sort out.
fn migrate_legacy_root(old: &Path, new: &Path) {
    if new.exists() || !old.is_dir() {
        return;
    }
    if let Some(parent) = new.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::rename(old, new).is_ok() {
        eprintln!("reactor: moved {} → {}", old.display(), new.display());
    }
}
