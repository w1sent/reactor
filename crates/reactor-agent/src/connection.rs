//! How a provider is reached: the endpoint, the key, where each came from, and the models the
//! endpoint offers.
//!
//! rig reads a provider's endpoint and key from the process environment
//! ([`crate::provider::AnyLlm::from_spec`] calls `Client::from_env`), so that is where REactor
//! keeps them too. [`EnvFile`] is only how a value outlives the process: `~/.reactor/env`,
//! owner-only, `NAME=value` lines, exported at start for the names below and never over a
//! variable the environment already has.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// What a provider needs to be reached, as rig reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSpec {
    pub name: &'static str,
    /// Where the provider is when nothing says otherwise.
    pub default_endpoint: &'static str,
    /// The variable that moves the endpoint; `None` where the endpoint is fixed.
    pub endpoint_var: Option<&'static str>,
    pub key_var: &'static str,
    /// Without it the provider cannot be used (Ollama's is only for a proxy in front of it).
    pub key_required: bool,
}

/// The providers, in the order they are shown. Matches [`crate::provider::PROVIDERS`] and
/// what `rig-core`'s `from_env` constructors read.
pub const SPECS: [ProviderSpec; 5] = [
    ProviderSpec {
        name: "anthropic",
        default_endpoint: "https://api.anthropic.com",
        endpoint_var: Some("ANTHROPIC_BASE_URL"),
        key_var: "ANTHROPIC_API_KEY",
        key_required: true,
    },
    ProviderSpec {
        name: "openai",
        default_endpoint: "https://api.openai.com/v1",
        endpoint_var: Some("OPENAI_BASE_URL"),
        key_var: "OPENAI_API_KEY",
        key_required: true,
    },
    ProviderSpec {
        name: "gemini",
        default_endpoint: "https://generativelanguage.googleapis.com",
        endpoint_var: None,
        key_var: "GEMINI_API_KEY",
        key_required: true,
    },
    ProviderSpec {
        name: "openrouter",
        default_endpoint: "https://openrouter.ai/api/v1",
        endpoint_var: None,
        key_var: "OPENROUTER_API_KEY",
        key_required: true,
    },
    ProviderSpec {
        name: "ollama",
        default_endpoint: "http://localhost:11434",
        endpoint_var: Some("OLLAMA_API_BASE_URL"),
        key_var: "OLLAMA_API_KEY",
        key_required: false,
    },
];

pub fn spec(provider: &str) -> Option<&'static ProviderSpec> {
    SPECS.iter().find(|s| s.name == provider)
}

/// Whether `name` is one of the variables this module manages. The env file is never allowed
/// to set anything else.
pub fn is_managed(name: &str) -> bool {
    SPECS
        .iter()
        .any(|s| s.key_var == name || s.endpoint_var == Some(name))
}

/// Where a value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Not set anywhere: the provider's own default (an endpoint) or nothing (a key).
    Default,
    /// The environment REactor was started in.
    Environment,
    /// `~/.reactor/env`.
    Saved,
    /// Typed in the window and not kept: gone when REactor closes.
    Session,
}

/// One variable as it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    /// What is in effect: the variable's value, else (an endpoint) the default, else `None`.
    pub value: Option<String>,
    pub source: Source,
    /// A saved value the environment overrides.
    pub shadowed: Option<String>,
}

/// Names set from the window this run and not written to the file.
static SESSION_ONLY: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The pure part of [`value_of`]: what a variable resolves to given what the environment, the
/// file and this run's unsaved edits say.
pub fn resolve(
    var: &str,
    default: Option<&str>,
    env: Option<&str>,
    saved: Option<&str>,
    session_only: &HashSet<String>,
) -> Value {
    match env.filter(|v| !v.is_empty()) {
        None => Value {
            value: default.map(str::to_string),
            source: Source::Default,
            shadowed: None,
        },
        Some(v) => {
            let (source, shadowed) = if session_only.contains(var) {
                (Source::Session, saved.filter(|s| *s != v))
            } else if saved == Some(v) {
                (Source::Saved, None)
            } else {
                (Source::Environment, saved.filter(|s| *s != v))
            };
            Value {
                value: Some(v.to_string()),
                source,
                shadowed: shadowed.map(str::to_string),
            }
        }
    }
}

/// `var` as it stands in this process.
pub fn value_of(var: &str, default: Option<&str>, file: &EnvFile) -> Value {
    let session: HashSet<String> = SESSION_ONLY.lock().unwrap().iter().cloned().collect();
    resolve(
        var,
        default,
        std::env::var(var).ok().as_deref(),
        file.get(var),
        &session,
    )
}

/// The endpoint the provider is reached at. Always has a value.
pub fn endpoint(provider: &str, file: &EnvFile) -> Option<Value> {
    let spec = spec(provider)?;
    Some(match spec.endpoint_var {
        Some(var) => value_of(var, Some(spec.default_endpoint), file),
        None => Value {
            value: Some(spec.default_endpoint.to_string()),
            source: Source::Default,
            shadowed: None,
        },
    })
}

/// The key's state; its `value` is the secret itself — show [`mask`] of it, never it.
pub fn key(provider: &str, file: &EnvFile) -> Option<Value> {
    Some(value_of(spec(provider)?.key_var, None, file))
}

/// `••••a91f`: enough to tell two keys apart.
pub fn mask(secret: &str) -> String {
    let tail: String = secret
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if secret.chars().count() <= 8 {
        "••••".to_string()
    } else {
        format!("••••{tail}")
    }
}

/// An endpoint worth saving: an http(s) URL with a host.
pub fn valid_endpoint(text: &str) -> bool {
    let rest = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"));
    match rest {
        Some(rest) => {
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            !host.is_empty() && !text.contains(char::is_whitespace)
        }
        None => false,
    }
}

// -- the file ----------------------------------------------------------------------------------

/// `~/.reactor/env`: the variables that outlive the process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvFile {
    path: PathBuf,
    vars: BTreeMap<String, String>,
}

impl EnvFile {
    pub fn path_in(config_dir: &Path) -> PathBuf {
        config_dir.join("env")
    }

    /// Read the file; a missing or unreadable one is empty.
    pub fn load(config_dir: &Path) -> EnvFile {
        let path = Self::path_in(config_dir);
        let vars = std::fs::read_to_string(&path)
            .map(|text| parse(&text))
            .unwrap_or_default();
        EnvFile { path, vars }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn set(&mut self, name: &str, value: &str) {
        if is_managed(name) {
            self.vars.insert(name.to_string(), value.to_string());
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.vars.remove(name);
    }

    /// Write the file, owner-only. Nothing is left behind when there is nothing to keep.
    pub fn save(&self) -> std::io::Result<()> {
        if self.vars.is_empty() {
            return match std::fs::remove_file(&self.path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut text = String::from(
            "# REactor: provider endpoints and keys, set from Settings > Model.\n\
             # Exported at start; a variable already in the environment wins.\n",
        );
        for (k, v) in &self.vars {
            text.push_str(&format!("{k}={v}\n"));
        }
        let temp = self.path.with_extension("tmp");
        write_private(&temp, &text)?;
        std::fs::rename(&temp, &self.path)
    }

    /// Export what the file holds into this process, for the variables the environment does
    /// not already have. Call once at start, before any thread reads the environment.
    pub fn export_missing(&self) {
        for (name, value) in &self.vars {
            if is_managed(name) && std::env::var_os(name).is_none() {
                export(name, value);
            }
        }
    }
}

#[cfg(unix)]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(text.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    std::fs::write(path, text)
}

fn parse(text: &str) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim().trim_start_matches("export ").trim();
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value);
        if is_managed(name) && !value.is_empty() {
            vars.insert(name.to_string(), value.to_string());
        }
    }
    vars
}

// -- the process environment -------------------------------------------------------------------

/// Put a managed variable in the process environment, for `from_env` to find.
///
/// Environment mutation is unsound against a concurrent reader on Unix, so this is for the
/// window's main thread, at the moment a person edits a field; the readers are the agent's
/// `from_env` calls, which run when a session starts or a model is switched.
pub fn export(name: &str, value: &str) {
    debug_assert!(is_managed(name));
    // SAFETY: see above — one writer, on the UI thread, at a person's keystroke rate.
    unsafe { std::env::set_var(name, value) };
}

/// Remove a managed variable from the process environment.
pub fn unexport(name: &str) {
    debug_assert!(is_managed(name));
    // SAFETY: as in [`export`].
    unsafe { std::env::remove_var(name) };
}

/// Set or clear a variable from the window. `keep` writes it to `file` (and saves it);
/// otherwise it lives until the process ends. An empty `value` clears it everywhere.
pub fn set_variable(
    file: &mut EnvFile,
    name: &str,
    value: &str,
    keep: bool,
) -> std::io::Result<()> {
    if !is_managed(name) {
        return Ok(());
    }
    let value = value.trim();
    let mut session = SESSION_ONLY.lock().unwrap();
    session.retain(|n| n != name);
    if value.is_empty() {
        unexport(name);
        file.remove(name);
        return file.save();
    }
    export(name, value);
    if keep {
        file.set(name, value);
        file.save()
    } else {
        // Not kept: whatever was saved before no longer matches, and must not come back.
        session.push(name.to_string());
        file.remove(name);
        file.save()
    }
}

// -- models --------------------------------------------------------------------------------------

/// The models the provider's endpoint reports, by id, with what is not for chatting left out.
///
/// Uses the environment as it is now, so an edit made in the window is seen without a restart.
/// Everything the endpoint lists. The listing does not say what a model is for, so telling chat
/// models from embedding ones is left to the caller ([`is_embedding_model`]).
pub async fn list_models(provider: &str) -> Result<Vec<String>, String> {
    use rig_core::client::{ModelListingClient, ProviderClient};
    use rig_core::providers::{anthropic, gemini, ollama, openai, openrouter};

    let listing = async {
        let list = match provider {
            "anthropic" => anthropic::Client::from_env()
                .map_err(|e| e.to_string())?
                .list_models()
                .await,
            "openai" => openai::Client::from_env()
                .map_err(|e| e.to_string())?
                .list_models()
                .await,
            "gemini" => gemini::Client::from_env()
                .map_err(|e| e.to_string())?
                .list_models()
                .await,
            "openrouter" => openrouter::Client::from_env()
                .map_err(|e| e.to_string())?
                .list_models()
                .await,
            "ollama" => ollama::Client::from_env()
                .map_err(|e| e.to_string())?
                .list_models()
                .await,
            other => return Err(format!("unknown provider `{other}`")),
        };
        list.map_err(|e| e.to_string())
    };
    let list = tokio::time::timeout(Duration::from_secs(12), listing)
        .await
        .map_err(|_| "no answer within 12 seconds".to_string())??;

    let mut ids: Vec<String> = list
        .iter()
        .map(|m| m.id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// The thinking levels an Ollama model takes, from the server's own answer for it
/// (`POST /api/show`).
///
/// NOTE: rig (0.42) has no call for this, so it is made here with `reqwest` directly. If rig
/// gains one (`/api/show` or a model-capabilities query), switch to it and drop this request
/// and the direct `reqwest` dependency. `None` when the server cannot be asked or does not say; an empty list
/// is a model that does not think.
pub async fn ollama_thinking_levels(model: &str) -> Option<Vec<String>> {
    let base = std::env::var("OLLAMA_API_BASE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "http://localhost:11434".to_string());
    let url = format!("{}/api/show", base.trim_end_matches('/'));
    let mut request = reqwest::Client::new()
        .post(url)
        .json(&serde_json::json!({ "model": model }))
        .timeout(Duration::from_secs(5));
    if let Ok(key) = std::env::var("OLLAMA_API_KEY")
        && !key.is_empty()
    {
        request = request.bearer_auth(key);
    }
    let shown: serde_json::Value = request.send().await.ok()?.json().await.ok()?;
    thinking_levels_from_show(&shown)
}

/// What `/api/show` says a model takes: the `thinking.values` list when there is one (named
/// levels, or booleans for a model that only switches on and off), else the `thinking`
/// capability.
pub fn thinking_levels_from_show(shown: &serde_json::Value) -> Option<Vec<String>> {
    if let Some(values) = shown
        .get("thinking")
        .and_then(|t| t.get("values"))
        .and_then(|v| v.as_array())
    {
        let named: Vec<String> = values
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        if !named.is_empty() {
            return Some(named);
        }
        // `[false]` is a model that does not think; a `true` means it switches on and off.
        return Some(if values.iter().any(|v| v.as_bool() == Some(true)) {
            vec!["off".into(), "on".into()]
        } else {
            Vec::new()
        });
    }
    let caps = shown.get("capabilities")?.as_array()?;
    Some(if caps.iter().any(|c| c.as_str() == Some("thinking")) {
        vec!["off".into(), "on".into()]
    } else {
        Vec::new()
    })
}

/// A model id that names an embedding model: by the word, or by the well-known families whose
/// names do not carry it. A guess; callers that need to be sure offer every model as well.
pub fn is_embedding_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.contains("embed")
        || id.contains("minilm")
        || id.contains("paraphrase-")
        || ["bge-", "gte-", "e5-", "jina-"]
            .iter()
            .any(|p| id.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_says_which_thinking_levels_a_model_takes() {
        use serde_json::json;
        let levels = |v| thinking_levels_from_show(&v);
        assert_eq!(
            levels(json!({"thinking": {"values": ["low", "medium", "high"], "default": "medium"}})),
            Some(vec!["low".into(), "medium".into(), "high".into()])
        );
        assert_eq!(
            levels(json!({"thinking": {"values": [false, true]}})),
            Some(vec!["off".into(), "on".into()])
        );
        assert_eq!(levels(json!({"thinking": {"values": [false]}})), Some(vec![]));
        assert_eq!(
            levels(json!({"capabilities": ["completion", "thinking"]})),
            Some(vec!["off".into(), "on".into()])
        );
        assert_eq!(levels(json!({"capabilities": ["completion"]})), Some(vec![]));
        assert_eq!(levels(json!({})), None);
    }

    #[test]
    fn the_specs_match_the_providers_the_agent_builds() {
        let names: Vec<&str> = SPECS.iter().map(|s| s.name).collect();
        assert_eq!(names, crate::provider::PROVIDERS);
    }

    #[test]
    fn only_three_providers_can_move_their_endpoint() {
        let movable: Vec<&str> = SPECS
            .iter()
            .filter(|s| s.endpoint_var.is_some())
            .map(|s| s.name)
            .collect();
        assert_eq!(movable, ["anthropic", "openai", "ollama"]);
        assert!(!spec("ollama").unwrap().key_required);
    }

    #[test]
    fn a_value_says_where_it_came_from() {
        let none = HashSet::new();
        let unset = resolve("OLLAMA_API_BASE_URL", Some("http://localhost:11434"), None, None, &none);
        assert_eq!(unset.source, Source::Default);
        assert_eq!(unset.value.as_deref(), Some("http://localhost:11434"));

        let env = resolve("X", None, Some("a"), None, &none);
        assert_eq!(env.source, Source::Environment);

        let saved = resolve("X", None, Some("a"), Some("a"), &none);
        assert_eq!(saved.source, Source::Saved);

        // The environment overrides the file, and the file's value is reported.
        let shadow = resolve("X", None, Some("a"), Some("b"), &none);
        assert_eq!(shadow.source, Source::Environment);
        assert_eq!(shadow.shadowed.as_deref(), Some("b"));

        let session: HashSet<String> = ["X".to_string()].into();
        assert_eq!(resolve("X", None, Some("a"), None, &session).source, Source::Session);

        // An empty variable is not a value.
        assert_eq!(resolve("X", None, Some(""), None, &none).source, Source::Default);
    }

    #[test]
    fn a_key_is_shown_by_its_tail_only() {
        assert_eq!(mask("sk-ant-0123456789a91f"), "••••a91f");
        assert_eq!(mask("short"), "••••");
    }

    #[test]
    fn endpoints_are_urls_with_a_host() {
        assert!(valid_endpoint("http://gpu-box.lan:11434"));
        assert!(valid_endpoint("https://llm.corp.example/v1"));
        assert!(!valid_endpoint("gpu-box.lan:11434"));
        assert!(!valid_endpoint("http://"));
        assert!(!valid_endpoint("http://a b"));
    }

    #[test]
    fn the_file_round_trips_and_ignores_what_is_not_ours() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = EnvFile::load(dir.path());
        file.set("OPENAI_API_KEY", "sk-test");
        file.set("PATH", "/evil"); // not a managed name: refused
        file.save().unwrap();

        let text = std::fs::read_to_string(EnvFile::path_in(dir.path())).unwrap();
        assert!(text.contains("OPENAI_API_KEY=sk-test"));
        assert!(!text.contains("PATH"));

        // A hand-edited file: comments, `export`, quotes, a foreign name.
        std::fs::write(
            EnvFile::path_in(dir.path()),
            "# note\nexport OLLAMA_API_BASE_URL=\"http://gpu-box.lan:11434\"\nHOME=/x\n",
        )
        .unwrap();
        let again = EnvFile::load(dir.path());
        assert_eq!(again.get("OLLAMA_API_BASE_URL"), Some("http://gpu-box.lan:11434"));
        assert_eq!(again.get("HOME"), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let mut file = EnvFile::load(dir.path());
        file.set("GEMINI_API_KEY", "k");
        file.save().unwrap();
        let mode = std::fs::metadata(file.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn an_empty_file_is_not_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = EnvFile::load(dir.path());
        file.set("GEMINI_API_KEY", "k");
        file.save().unwrap();
        file.remove("GEMINI_API_KEY");
        file.save().unwrap();
        assert!(!file.path().exists());
    }

    #[test]
    fn embedding_models_are_not_for_the_picker() {
        assert!(is_embedding_model("nomic-embed-text:latest"));
        assert!(is_embedding_model("text-embedding-3-small"));
        assert!(!is_embedding_model("qwen2.5-coder:32b"));
        assert!(is_embedding_model("bge-m3:latest"));
        assert!(is_embedding_model("all-minilm"));
        assert!(!is_embedding_model("gemini-2.5-pro"));
    }
}
