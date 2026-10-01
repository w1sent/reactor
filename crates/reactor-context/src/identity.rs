//! The agent's working persona, selected per session and injected into the
//! system prompt. Port of `extensions/identity/` (ADR-0026).
//!
//! Off by default: with no selection nothing is injected and the system prompt
//! stays byte-identical, so switching identity mid-session costs exactly one
//! cache invalidation. The built-in texts are code (compiled in from
//! `identities/*.md`): nothing for an installer to seed, and they cannot drift
//! from the renderer that uses them.

use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::notice::Notice;
use crate::settings::IdentitySettings;
use crate::text::js_trim;

pub const ENTRY_TYPE: &str = "pi-identity";

/// The built-ins, in the order the TS declared them (which is the order
/// `/identity` lists them in).
pub const BUILTINS: [(&str, &str); 6] = [
    (
        "reverse-engineer",
        include_str!("../identities/reverse-engineer.md"),
    ),
    (
        "cyber-forensics",
        include_str!("../identities/cyber-forensics.md"),
    ),
    ("forensics", include_str!("../identities/forensics.md")),
    (
        "software-engineer",
        include_str!("../identities/software-engineer.md"),
    ),
    (
        "infrastructure",
        include_str!("../identities/infrastructure.md"),
    ),
    ("publisher", include_str!("../identities/publisher.md")),
];

/// One line per built-in, for a completion dropdown.
pub fn builtin_description(name: &str) -> Option<&'static str> {
    Some(match name {
        "reverse-engineer" => "code-level artifact analysis",
        "cyber-forensics" => "malware incident reconstruction",
        "forensics" => "general and user-activity forensics",
        "software-engineer" => "tooling for the analysis team",
        "infrastructure" => "analysis infrastructure and isolation",
        "publisher" => "defensible deliverables",
        _ => return None,
    })
}

pub fn builtin(name: &str) -> Option<&'static str> {
    BUILTINS.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
}

/// The user's saved identities, in file order, serialized as a JSON object.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserIdentities(Vec<(String, String)>);

impl UserIdentities {
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut me = UserIdentities::default();
        for (k, v) in pairs {
            me.set(k, v);
        }
        me
    }
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    /// Overwrites in place, so a saved name keeps its position.
    pub fn set(&mut self, name: String, text: String) {
        match self.0.iter_mut().find(|(k, _)| *k == name) {
            Some(slot) => slot.1 = text,
            None => self.0.push((name, text)),
        }
    }
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.0.len();
        self.0.retain(|(k, _)| k != name);
        self.0.len() != before
    }
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(k, _)| k.as_str())
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for UserIdentities {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_map(self.0.iter().map(|(k, v)| (k, v)))
    }
}

/// Per-session identity state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct State {
    /// The active identity: a built-in, a saved one, or `"custom"`. Absent = the
    /// global default applies. Empty string = explicitly off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    /// The adhoc custom identity's text, for `active == "custom"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

impl State {
    pub fn normalize(raw: &Value) -> State {
        let Some(r) = raw.as_object() else {
            return State::default();
        };
        State {
            active: r.get("active").and_then(Value::as_str).map(str::to_string),
            custom: r.get("custom").and_then(Value::as_str).map(str::to_string),
        }
    }

    pub fn active_name<'a>(&'a self, cfg: &'a IdentitySettings) -> Option<&'a str> {
        let name = match &self.active {
            None => cfg.default.as_str(),
            Some(a) => a.as_str(), // "" is an explicit off
        };
        (!name.is_empty()).then_some(name)
    }

    pub fn resolve_text<'a>(
        &'a self,
        cfg: &'a IdentitySettings,
        name: Option<&str>,
    ) -> Option<&'a str> {
        let name = name.filter(|n| !n.is_empty())?;
        if name == "custom" {
            let t = self.custom.as_deref().map(js_trim).unwrap_or("");
            return (!t.is_empty()).then_some(t);
        }
        builtin(name).or_else(|| cfg.user.get(name))
    }
}

/// The block for the system prompt, when an identity is active and has text.
pub fn block(state: &State, cfg: &IdentitySettings) -> Option<String> {
    let text = state.resolve_text(cfg, state.active_name(cfg))?;
    (!text.is_empty()).then(|| format!("## Identity\n\n{text}"))
}

pub fn available_names(cfg: &IdentitySettings) -> String {
    let mut names: Vec<&str> = BUILTINS.iter().map(|(n, _)| *n).collect();
    names.push("custom");
    names.extend(cfg.user.names());
    names.join(", ")
}

/// Names that can be selected right now: built-ins, the custom one when it has
/// text, and the user's saved ones.
pub fn selectable_names(state: &State, cfg: &IdentitySettings) -> Vec<String> {
    let mut v: Vec<String> = BUILTINS.iter().map(|(n, _)| n.to_string()).collect();
    if state
        .custom
        .as_deref()
        .is_some_and(|c| !js_trim(c).is_empty())
    {
        v.push("custom".into());
    }
    v.extend(cfg.user.names().map(str::to_string));
    v
}

/// What a handler did to the world.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effects {
    pub notices: Vec<Notice>,
    /// The session state changed and must be stored.
    pub persist: bool,
    /// The user's saved identities or the default changed and the global
    /// settings must be written.
    pub save_settings: bool,
}

impl Effects {
    fn note(n: Notice) -> Self {
        Effects {
            notices: vec![n],
            ..Default::default()
        }
    }
    fn changed(n: Notice) -> Self {
        Effects {
            notices: vec![n],
            persist: true,
            save_settings: false,
        }
    }
}

fn select(state: &mut State, cfg: &IdentitySettings, name: &str) -> Effects {
    let known = name == "custom"
        || builtin(name).is_some()
        || cfg.user.get(name).is_some_and(|t| !t.is_empty());
    if !known {
        return Effects::note(Notice::warning(format!(
            "identity: unknown identity \"{name}\" -- available: {}",
            available_names(cfg)
        )));
    }
    if name == "custom"
        && state
            .custom
            .as_deref()
            .map(js_trim)
            .unwrap_or("")
            .is_empty()
    {
        return Effects::note(Notice::warning(
            "identity: the custom identity is empty -- write one with /identity write <text> or /identity editor",
        ));
    }
    state.active = Some(name.to_string());
    Effects::changed(Notice::info(format!("identity: {name}")))
}

fn set_custom(state: &mut State, text: &str) -> Effects {
    state.active = Some("custom".into());
    state.custom = Some(text.to_string());
    Effects::changed(Notice::info(
        "identity: custom set -- save it with /identity save <name> if it proves useful",
    ))
}

/// `/identity`. `cfg` is the *global* identity settings and is edited in place
/// by `save`/`delete` (the effects say so); `editor` is not handled here — it
/// needs a terminal. Feed its outcome to [`apply_editor`].
pub fn command(state: &mut State, cfg: &mut IdentitySettings, args: &str) -> Effects {
    let arg = js_trim(args);
    if arg.is_empty() {
        let name = state.active_name(cfg);
        let text = state.resolve_text(cfg, name);
        let mut lines = vec![format!("identity: {}", name.unwrap_or("(none)"))];
        if name == Some("custom") && text.is_none() {
            lines.push(
                "  (the custom identity is empty -- write one with /identity write <text>)".into(),
            );
        }
        lines.push(format!("available: {}", available_names(cfg)));
        return Effects::note(Notice::info(lines.join("\n")));
    }
    if arg == "off" {
        state.active = Some(String::new());
        return Effects::changed(Notice::info("identity: off"));
    }
    if arg == "show" {
        let name = state.active_name(cfg);
        return Effects::note(Notice::info(match state.resolve_text(cfg, name) {
            Some(text) if !text.is_empty() => format!("[{}]\n{text}", name.unwrap_or("")),
            _ => "identity: none active".into(),
        }));
    }
    if let Some(rest) = arg.strip_prefix("show ") {
        let name = js_trim(rest);
        return Effects::note(Notice::info(match state.resolve_text(cfg, Some(name)) {
            Some(text) if !text.is_empty() => format!("[{name}]\n{text}"),
            _ => format!("identity: no such identity \"{name}\""),
        }));
    }
    if let Some(rest) = arg.strip_prefix("write ") {
        let text = js_trim(rest);
        if text.is_empty() {
            return Effects::note(Notice::warning("usage: /identity write <text>"));
        }
        return set_custom(state, text);
    }
    if arg == "save" || arg.starts_with("save ") {
        let name = if arg == "save" {
            ""
        } else {
            js_trim(&arg["save ".len()..])
        };
        if name.is_empty() {
            return Effects::note(Notice::warning("usage: /identity save <name>"));
        }
        if builtin(name).is_some() {
            return Effects::note(Notice::warning(format!(
                "identity: \"{name}\" is built in -- pick another name"
            )));
        }
        let text = if state.active.as_deref() == Some("custom") {
            state
                .custom
                .as_deref()
                .map(js_trim)
                .filter(|t| !t.is_empty())
        } else {
            None
        };
        let Some(text) = text else {
            return Effects::note(Notice::warning(
                "identity: only an adhoc custom identity can be saved -- write one with /identity write <text>",
            ));
        };
        let existed = cfg.user.get(name).is_some_and(|t| !t.is_empty());
        cfg.user.set(name.to_string(), text.to_string());
        return Effects {
            notices: vec![Notice::info(format!(
                "identity: saved \"{name}\"{}",
                if existed {
                    " (overwrote an existing one)"
                } else {
                    ""
                }
            ))],
            persist: false,
            save_settings: true,
        };
    }
    if let Some(rest) = arg.strip_prefix("delete ") {
        let name = js_trim(rest);
        if builtin(name).is_some() {
            return Effects::note(Notice::warning(format!(
                "identity: \"{name}\" is built in -- it cannot be deleted"
            )));
        }
        if !cfg.user.get(name).is_some_and(|t| !t.is_empty()) {
            return Effects::note(Notice::warning(format!(
                "identity: no saved identity \"{name}\""
            )));
        }
        cfg.user.remove(name);
        return Effects {
            notices: vec![Notice::info(format!("identity: deleted \"{name}\""))],
            persist: false,
            save_settings: true,
        };
    }
    select(state, cfg, arg)
}

/// Whether `/identity <args>` is the `editor` subcommand, which needs a terminal
/// and so is the caller's to run (then [`apply_editor`]).
pub fn wants_editor(args: &str) -> bool {
    js_trim(args) == "editor"
}

/// What an external editor session came back with.
pub enum EditorResult {
    Complete(String),
    Cancelled,
}

/// The tail of `/identity editor`, once the editor has run.
pub fn apply_editor(state: &mut State, result: EditorResult) -> Effects {
    match result {
        EditorResult::Cancelled => Effects::note(Notice::info(
            "identity: editor exited without saving -- cancelled",
        )),
        EditorResult::Complete(content) => {
            let text = js_trim(&content);
            if text.is_empty() {
                Effects::note(Notice::warning(
                    "identity: editor content was empty -- nothing set",
                ))
            } else {
                set_custom(state, text)
            }
        }
    }
}

/// The editor needs a terminal.
pub fn editor_needs_terminal() -> Effects {
    Effects::note(Notice::warning(
        "identity: the editor needs a terminal; use /identity write <text> here",
    ))
}
