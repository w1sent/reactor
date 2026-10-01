//! Settings resolve global default, then session override
//! ([ADR-0038](../../docs/adr/0038-settings-resolve-global-then-session.md)).
//!
//! This is the *global half*: `~/.reactor/settings.json`, which the pi flavor
//! spreads over `reactor.json`, `pi-goal-setting.json`, `pi-identity.json` and
//! `pi-reactor-reporting.json`. The session half arrives with the session store
//! (phase 4); until then the per-session state each module already owns
//! (`manifest::State::enabled`, `identity::State::active`, …) *is* the override,
//! and [`resolve`] is the one place the rule is written down.
//!
//! Loading is forgiving on purpose — an absent file, unreadable JSON, or a field
//! of the wrong shape all mean "use the default". This is a preference a person
//! sets, not a contract to fail loudly over. Saving is not: it goes through the
//! atomic, sorted-key writer every other JSON file in REactor uses.

use std::path::{Path, PathBuf};

use reactor_core::json::write_json_atomic;
use reactor_core::{Paths, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::identity::UserIdentities;

/// Where a resolved value came from — what the GUI shows beside a setting so
/// the cascade is never the confusing kind of magic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The global default (`settings.json`, or the built-in default under it).
    Default,
    /// This session's own override.
    Session,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved<T> {
    pub value: T,
    pub origin: Origin,
}

/// A session override wins where present; absent means inherit. A pure function
/// of its two inputs, and tested as one — no setting gets a bespoke lookup.
pub fn resolve<T: Clone>(global: &T, session: Option<&T>) -> Resolved<T> {
    match session {
        Some(v) => Resolved {
            value: v.clone(),
            origin: Origin::Session,
        },
        None => Resolved {
            value: global.clone(),
            origin: Origin::Default,
        },
    }
}

// -- sections -----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSettings {
    pub soft_step_limit: usize,
    pub max_description: usize,
    pub status_words: usize,
    /// Char budget for the session tail handed to the derive call.
    pub derive_context_chars: usize,
}

impl Default for ManifestSettings {
    fn default() -> Self {
        Self {
            soft_step_limit: 20,
            max_description: 80,
            status_words: 3,
            derive_context_chars: 24_000,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct IdentitySettings {
    /// A name applied when the session has not chosen one. Empty = off.
    pub default: String,
    /// The user's own saved identities, in the order the file lists them.
    pub user: UserIdentities,
}

/// How hard reporting pushes: 0 the prompt block only, 1 adds a nag, 2 reverts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Enforcement(u8);

impl Enforcement {
    pub const OFF: Enforcement = Enforcement(0);
    pub const NAG: Enforcement = Enforcement(1);
    pub const STRICT: Enforcement = Enforcement(2);

    pub fn new(n: u8) -> Option<Enforcement> {
        (n <= 2).then_some(Enforcement(n))
    }
    pub fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for Enforcement {
    type Error = String;
    fn try_from(n: u8) -> std::result::Result<Self, String> {
        Enforcement::new(n).ok_or_else(|| format!("reporting level must be 0, 1 or 2, not {n}"))
    }
}

impl From<Enforcement> for u8 {
    fn from(l: Enforcement) -> u8 {
        l.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportingSettings {
    pub level: Enforcement,
    pub folder: String,
    pub step_threshold: usize,
    pub max_reverts: usize,
    pub template_path: Option<String>,
}

impl Default for ReportingSettings {
    fn default() -> Self {
        Self {
            level: Enforcement::OFF,
            folder: "report".into(),
            step_threshold: 8,
            max_reverts: 3,
            template_path: None,
        }
    }
}

/// How the context budget behaves (ADR-0037), as one *layer*: every field is optional,
/// and a field that is absent means "the layer below decides". The layers are the
/// built-in defaults (`reactor-agent`'s `BudgetConfig::new`), then the global
/// `settings.json`, then a session's own override (ADR-0038) — the same type at both of
/// the two stored layers, combined with [`ContextSettings::over`].
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSettings {
    /// `auto`, `fade` or `compact`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Reduce once the context passes this fraction of `window - reserve`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pct: Option<f64>,
    /// Reduce down to this fraction of it: the keep window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep: Option<f64>,
    /// Headroom for the reply, in tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reserve: Option<u64>,
    /// The context window in tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<u64>,
    /// The model that writes summaries (`provider/name`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summarizer: Option<String>,
}

pub const CONTEXT_MODES: [&str; 3] = ["auto", "fade", "compact"];

impl ContextSettings {
    /// Read a stored layer, keeping only what has the right shape.
    pub fn normalize(raw: &Value) -> ContextSettings {
        let Some(r) = raw.as_object() else {
            return ContextSettings::default();
        };
        ContextSettings {
            mode: r
                .get("mode")
                .and_then(Value::as_str)
                .filter(|m| CONTEXT_MODES.contains(m))
                .map(str::to_string),
            pct: number(r.get("pct")).filter(|p| *p > 0.0 && *p <= 1.0),
            keep: number(r.get("keep")).filter(|k| *k > 0.0 && *k < 1.0),
            reserve: number(r.get("reserve"))
                .filter(|n| *n >= 0.0 && n.fract() == 0.0)
                .map(|n| n as u64),
            window: number(r.get("window"))
                .filter(|n| *n > 0.0 && n.fract() == 0.0)
                .map(|n| n as u64),
            summarizer: r
                .get("summarizer")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == ContextSettings::default()
    }

    /// This layer over `below`: each field is this layer's where it has one, else
    /// `below`'s — [`resolve`]'s rule, per field.
    pub fn over(&self, below: &ContextSettings) -> ContextSettings {
        ContextSettings {
            mode: self.mode.clone().or_else(|| below.mode.clone()),
            pct: self.pct.or(below.pct),
            keep: self.keep.or(below.keep),
            reserve: self.reserve.or(below.reserve),
            window: self.window.or(below.window),
            summarizer: self.summarizer.clone().or_else(|| below.summarizer.clone()),
        }
    }

    /// Where each field of `self.over(global)` comes from — what a UI shows beside a
    /// setting so the cascade is never the confusing kind of magic.
    pub fn origins(&self) -> [(&'static str, Origin); 6] {
        let o = |set: bool| {
            if set {
                Origin::Session
            } else {
                Origin::Default
            }
        };
        [
            ("mode", o(self.mode.is_some())),
            ("pct", o(self.pct.is_some())),
            ("keep", o(self.keep.is_some())),
            ("reserve", o(self.reserve.is_some())),
            ("window", o(self.window.is_some())),
            ("summarizer", o(self.summarizer.is_some())),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// `false` withdraws the toolbox (registry block, selector) from a session.
    pub toolbox: bool,
    /// Catalogue ids omitted from service listings.
    pub hidden_services: Vec<String>,
    pub manifest: ManifestSettings,
    pub identity: IdentitySettings,
    pub reporting: ReportingSettings,
    pub context: ContextSettings,
    /// Models to offer in a picker, as `provider/name`.
    pub models: Vec<String>,
    /// The model a new session starts on.
    pub default_model: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            toolbox: true,
            hidden_services: Vec::new(),
            manifest: ManifestSettings::default(),
            identity: IdentitySettings::default(),
            reporting: ReportingSettings::default(),
            context: ContextSettings::default(),
            models: Vec::new(),
            default_model: None,
        }
    }
}

// -- loading ------------------------------------------------------------------

fn number(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
}

impl Settings {
    /// Parse leniently: every field that is present *and* the right shape is
    /// taken, everything else keeps its default.
    pub fn from_json(raw: &Value) -> Settings {
        let mut s = Settings::default();
        let Some(root) = raw.as_object() else {
            return s;
        };

        if let Some(b) = root.get("toolbox").and_then(Value::as_bool) {
            s.toolbox = b;
        }
        if let Some(Value::Array(ids)) = root.get("hiddenServices") {
            s.hidden_services = ids
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
        }

        if let Some(m) = root.get("manifest").and_then(Value::as_object) {
            let take = |key: &str, into: &mut usize| {
                // JS took any defined value; a count that is not a non-negative
                // integer cannot mean anything here.
                if let Some(n) = number(m.get(key)).filter(|n| *n >= 0.0 && n.fract() == 0.0) {
                    *into = n as usize;
                }
            };
            take("softStepLimit", &mut s.manifest.soft_step_limit);
            take("maxDescription", &mut s.manifest.max_description);
            take("statusWords", &mut s.manifest.status_words);
            take("deriveContextChars", &mut s.manifest.derive_context_chars);
        }

        if let Some(i) = root.get("identity").and_then(Value::as_object) {
            if let Some(d) = i.get("default").and_then(Value::as_str) {
                s.identity.default = d.to_string();
            }
            if let Some(Value::Object(user)) = i.get("user") {
                s.identity.user = UserIdentities::from_pairs(
                    user.iter()
                        .filter_map(|(k, v)| v.as_str().map(|t| (k.clone(), t.to_string()))),
                );
            }
        }

        if let Some(Value::Array(ms)) = root.get("models") {
            s.models = ms
                .iter()
                .filter_map(|v| v.as_str().filter(|m| m.contains('/')).map(str::to_string))
                .collect();
        }
        s.default_model = root
            .get("defaultModel")
            .and_then(Value::as_str)
            .filter(|m| m.contains('/'))
            .map(str::to_string);
        if let Some(c) = root.get("context") {
            s.context = ContextSettings::normalize(c);
        }

        if let Some(r) = root.get("reporting").and_then(Value::as_object) {
            if let Some(level) = number(r.get("level"))
                .filter(|n| n.fract() == 0.0)
                .and_then(|n| u8::try_from(n as i64).ok())
                .and_then(Enforcement::new)
            {
                s.reporting.level = level;
            }
            if let Some(f) = r
                .get("folder")
                .and_then(Value::as_str)
                .filter(|f| !crate::text::js_trim(f).is_empty())
            {
                s.reporting.folder = f.to_string();
            }
            if let Some(n) = number(r.get("stepThreshold")).filter(|n| *n > 0.0) {
                s.reporting.step_threshold = n as usize;
            }
            if let Some(n) = number(r.get("maxReverts")).filter(|n| *n >= 0.0) {
                s.reporting.max_reverts = n as usize;
            }
            if let Some(t) = r
                .get("templatePath")
                .and_then(Value::as_str)
                .filter(|t| !crate::text::js_trim(t).is_empty())
            {
                s.reporting.template_path = Some(t.to_string());
            }
        }
        s
    }

    pub fn path(paths: &Paths) -> PathBuf {
        paths.config_dir.join("settings.json")
    }

    /// The global settings, defaults where the file is absent or unreadable.
    pub fn load(paths: &Paths) -> Settings {
        Settings::load_from(&Settings::path(paths))
    }

    pub fn load_from(path: &Path) -> Settings {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map(|v| Settings::from_json(&v))
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        write_json_atomic(&Settings::path(paths), self)
    }

    /// Merge `patch` (a partial settings object) over what is on disk, so a
    /// caller changing one key does not clobber the rest.
    pub fn patch(paths: &Paths, patch: Map<String, Value>) -> Result<Settings> {
        let mut doc = serde_json::to_value(Settings::load(paths)).expect("settings serialize");
        if let Some(obj) = doc.as_object_mut() {
            for (k, v) in patch {
                obj.insert(k, v);
            }
        }
        let next = Settings::from_json(&doc);
        next.save(paths)?;
        Ok(next)
    }
}
