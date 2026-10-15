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

/// How much of one tool result the model sees before it is cut to its head and tail
/// (`reactor-agent`'s `truncate`). A call to a built-in tool may ask for another limit;
/// this is the one that applies when it does not, and always to MCP tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutputSettings {
    pub max_inline_bytes: usize,
}

impl ToolOutputSettings {
    /// Below this the marker would be most of what is shown.
    pub const MIN_BYTES: usize = 1024;
    /// Above this, paging with `history_read` is the better way to read.
    pub const MAX_BYTES: usize = 1024 * 1024;

    pub fn is_default(&self) -> bool {
        *self == ToolOutputSettings::default()
    }
}

impl Default for ToolOutputSettings {
    fn default() -> Self {
        Self {
            max_inline_bytes: 32 * 1024,
        }
    }
}

/// When tools are loaded on demand instead of advertised
/// ([ADR-0055](../../../docs/adr/0055-large-tool-groups-are-loaded-on-demand.md)): a group of
/// tools (the case graph's, or one MCP server's) whose definitions together cost more than
/// `defer_over_tokens` is left out of the request, and the agent loads what it needs with
/// `tool_search`. `0` defers every group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSearchSettings {
    pub defer_over_tokens: usize,
}

impl ToolSearchSettings {
    pub const MAX_TOKENS: usize = 100_000;

    pub fn is_default(&self) -> bool {
        *self == ToolSearchSettings::default()
    }
}

impl Default for ToolSearchSettings {
    fn default() -> Self {
        Self {
            defer_over_tokens: 1_000,
        }
    }
}

/// What a session may draw on from the rest of its case
/// ([ADR-0047](../../../docs/adr/0047-the-analysis-graph-is-the-case-s-model.md)), as one
/// *layer* like [`ContextSettings`]: an absent field means "the layer below decides", and
/// with no layer deciding, all are on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySettings {
    /// Whether the session may search and read the case's other sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_history: Option<bool>,
    /// Whether the session is attached to the case's graph. Off detaches it: no reading,
    /// no writing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case_graph: Option<bool>,
    /// Whether the session may search the case's reference documents. Off withdraws
    /// `docs_search`; the person can still search them in the window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documents: Option<bool>,
}

impl MemorySettings {
    /// Read a stored layer, keeping only what has the right shape.
    pub fn normalize(raw: &Value) -> MemorySettings {
        let Some(r) = raw.as_object() else {
            return MemorySettings::default();
        };
        MemorySettings {
            session_history: r.get("sessionHistory").and_then(Value::as_bool),
            case_graph: r.get("caseGraph").and_then(Value::as_bool),
            documents: r.get("documents").and_then(Value::as_bool),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == MemorySettings::default()
    }

    /// This layer over `below` -- [`resolve`]'s rule, per field.
    pub fn over(&self, below: &MemorySettings) -> MemorySettings {
        MemorySettings {
            session_history: self.session_history.or(below.session_history),
            case_graph: self.case_graph.or(below.case_graph),
            documents: self.documents.or(below.documents),
        }
    }

    pub fn session_history(&self) -> bool {
        self.session_history.unwrap_or(true)
    }

    pub fn case_graph(&self) -> bool {
        self.case_graph.unwrap_or(true)
    }

    pub fn documents(&self) -> bool {
        self.documents.unwrap_or(true)
    }
}

/// The model that makes the vectors the context documents are searched by
/// ([ADR-0046](../../../docs/adr/0046-the-analysis-database-is-sqlite-inside-the-case.md)).
/// Global only: a case records which model made its vectors, and a new session simply uses
/// what is chosen. There is no default — until one is chosen, search is by keyword.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingSettings {
    /// `provider/name`, in the same form as the chat model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// How close in meaning a passage must be to a question to count as a match, as a cosine
    /// similarity from 0 (no threshold) to 1 (identical). What is a good value depends on
    /// the model; unset means none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_similarity: Option<f32>,
}

impl EmbeddingSettings {
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.min_similarity.is_none()
    }

    /// The threshold in effect: 0 when none is set.
    pub fn threshold(&self) -> f32 {
        self.min_similarity.unwrap_or(0.0)
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
    #[serde(skip_serializing_if = "ToolOutputSettings::is_default")]
    pub tool_output: ToolOutputSettings,
    #[serde(skip_serializing_if = "ToolSearchSettings::is_default")]
    pub tool_search: ToolSearchSettings,
    #[serde(skip_serializing_if = "MemorySettings::is_empty")]
    pub memory: MemorySettings,
    #[serde(skip_serializing_if = "EmbeddingSettings::is_empty")]
    pub embedding: EmbeddingSettings,
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
            tool_output: ToolOutputSettings::default(),
            tool_search: ToolSearchSettings::default(),
            memory: MemorySettings::default(),
            embedding: EmbeddingSettings::default(),
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
        if let Some(n) = number(root.get("toolOutput").and_then(|t| t.get("maxInlineBytes")))
            .filter(|n| *n > 0.0 && n.fract() == 0.0)
        {
            s.tool_output.max_inline_bytes =
                (n as usize).clamp(ToolOutputSettings::MIN_BYTES, ToolOutputSettings::MAX_BYTES);
        }
        if let Some(n) = number(
            root.get("toolSearch")
                .and_then(|t| t.get("deferOverTokens")),
        )
        .filter(|n| *n >= 0.0 && n.fract() == 0.0)
        {
            s.tool_search.defer_over_tokens = (n as usize).min(ToolSearchSettings::MAX_TOKENS);
        }
        if let Some(m) = root.get("memory") {
            s.memory = MemorySettings::normalize(m);
        }
        s.embedding.model = root
            .get("embedding")
            .and_then(|e| e.get("model"))
            .and_then(Value::as_str)
            .filter(|m| m.contains('/'))
            .map(str::to_string);
        s.embedding.min_similarity = root
            .get("embedding")
            .and_then(|e| e.get("minSimilarity"))
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
            .map(|v| v.clamp(0.0, 1.0) as f32)
            .filter(|v| *v > 0.0);

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

#[cfg(test)]
mod threshold_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_deferral_threshold_is_kept_bounded_and_absent_means_a_thousand_tokens() {
        let at = |v: Value| Settings::from_json(&v).tool_search.defer_over_tokens;
        assert_eq!(at(json!({})), 1_000);
        assert_eq!(at(json!({ "toolSearch": { "deferOverTokens": 0 } })), 0);
        assert_eq!(
            at(json!({ "toolSearch": { "deferOverTokens": 4000 } })),
            4000
        );
        assert_eq!(
            at(json!({ "toolSearch": { "deferOverTokens": 1e9 } })),
            ToolSearchSettings::MAX_TOKENS
        );
        assert_eq!(
            at(json!({ "toolSearch": { "deferOverTokens": -5 } })),
            1_000
        );
        assert_eq!(
            at(json!({ "toolSearch": { "deferOverTokens": "lots" } })),
            1_000
        );
        let s = Settings::from_json(&json!({ "toolSearch": { "deferOverTokens": 0 } }));
        let back = Settings::from_json(&serde_json::to_value(&s).unwrap());
        assert_eq!(back.tool_search, s.tool_search);
    }

    #[test]
    fn the_tool_output_limit_is_kept_clamped_and_absent_means_32_kib() {
        let at = |v: Value| Settings::from_json(&v).tool_output.max_inline_bytes;
        assert_eq!(at(json!({})), 32 * 1024);
        assert_eq!(
            at(json!({ "toolOutput": { "maxInlineBytes": 65536 } })),
            65536
        );
        assert_eq!(at(json!({ "toolOutput": { "maxInlineBytes": 10 } })), 1024);
        assert_eq!(
            at(json!({ "toolOutput": { "maxInlineBytes": 1e9 } })),
            1024 * 1024
        );
        assert_eq!(
            at(json!({ "toolOutput": { "maxInlineBytes": "big" } })),
            32 * 1024
        );
        let s = Settings::from_json(&json!({ "toolOutput": { "maxInlineBytes": 4096 } }));
        let back = Settings::from_json(&serde_json::to_value(&s).unwrap());
        assert_eq!(back.tool_output, s.tool_output);
    }

    #[test]
    fn the_meaning_threshold_is_kept_clamped_and_absent_means_none() {
        let s = Settings::from_json(
            &json!({ "embedding": { "model": "ollama/x", "minSimilarity": 0.45 } }),
        );
        assert_eq!(s.embedding.threshold(), 0.45);
        assert_eq!(s.embedding.model.as_deref(), Some("ollama/x"));
        assert_eq!(
            Settings::from_json(&json!({ "embedding": { "minSimilarity": 7 } }))
                .embedding
                .threshold(),
            1.0
        );
        assert_eq!(
            Settings::from_json(&json!({ "embedding": { "minSimilarity": -1 } }))
                .embedding
                .threshold(),
            0.0
        );
        assert_eq!(Settings::from_json(&json!({})).embedding.threshold(), 0.0);
        // It survives being written out and read back, and is left out when it is not set.
        let back = Settings::from_json(&serde_json::to_value(&s).unwrap());
        assert_eq!(back.embedding, s.embedding);
        let none = serde_json::to_value(Settings::default()).unwrap();
        assert!(none.get("embedding").is_none());
    }
}
