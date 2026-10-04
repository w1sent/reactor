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
    /// Rename in place, keeping the position. Nothing happens if `from` is not there.
    pub fn rename(&mut self, from: &str, to: String) {
        if let Some(slot) = self.0.iter_mut().find(|(k, _)| k == from) {
            slot.0 = to;
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

// -- managing the user's saved identities (the settings window's editor) -----------------------

/// Words `/identity` itself takes first, which a saved identity cannot be called (it could
/// never be selected by name), besides `custom` and the built-ins.
pub const RESERVED: [&str; 9] = [
    "custom", "off", "show", "write", "save", "delete", "new", "edit", "manage",
];

/// Why `name` cannot be the name of a saved identity, if it cannot: empty, with a space in it
/// (`/identity <name>` could not select it), a word `/identity` uses, or a built-in's.
pub fn name_problem(name: &str) -> Option<String> {
    let name = js_trim(name);
    if name.is_empty() {
        return Some("an identity needs a name".into());
    }
    if name.chars().any(char::is_whitespace) {
        return Some("the name cannot have a space in it: `/identity <name>` selects by it".into());
    }
    if builtin(name).is_some() {
        return Some(format!(
            "\"{name}\" is a built-in identity: pick another name"
        ));
    }
    if RESERVED.contains(&name) {
        return Some(format!(
            "\"{name}\" is a word `/identity` uses: pick another name"
        ));
    }
    None
}

/// Save a user identity: `name` with `text`, replacing `old` when it is a rename (so that it
/// keeps its place) and refusing to overwrite another identity. A default that pointed at the
/// old name follows it.
pub fn save_user(
    cfg: &mut IdentitySettings,
    old: Option<&str>,
    name: &str,
    text: &str,
) -> Result<(), String> {
    let name = js_trim(name);
    if let Some(problem) = name_problem(name) {
        return Err(problem);
    }
    if js_trim(text).is_empty() {
        return Err("an identity needs text: what the model is told it is".into());
    }
    let renaming = old.filter(|o| *o != name);
    if renaming.is_none() && old.is_none() && cfg.user.get(name).is_some() {
        return Err(format!(
            "there is already an identity called \"{name}\": edit it, or pick another name"
        ));
    }
    if renaming.is_some() && cfg.user.get(name).is_some() {
        return Err(format!("there is already an identity called \"{name}\""));
    }
    match renaming {
        // A rename keeps its place in the list.
        Some(o) if cfg.user.get(o).is_some() => {
            cfg.user.rename(o, name.to_string());
            cfg.user.set(name.to_string(), text.to_string());
            if cfg.default == o {
                cfg.default = name.to_string();
            }
        }
        _ => cfg.user.set(name.to_string(), text.to_string()),
    }
    Ok(())
}

/// Delete a user identity. A default that was it is cleared.
pub fn delete_user(cfg: &mut IdentitySettings, name: &str) -> Result<(), String> {
    if builtin(name).is_some() {
        return Err(format!("\"{name}\" is built in: it cannot be deleted"));
    }
    if !cfg.user.remove(name) {
        return Err(format!("there is no saved identity \"{name}\""));
    }
    if cfg.default == name {
        cfg.default.clear();
    }
    Ok(())
}

/// Make `name` the identity a session without its own choice starts with (`""` for none).
pub fn set_default(cfg: &mut IdentitySettings, name: &str) -> Result<(), String> {
    let name = js_trim(name);
    if !name.is_empty() && builtin(name).is_none() && cfg.user.get(name).is_none() {
        return Err(format!("there is no identity \"{name}\""));
    }
    cfg.default = name.to_string();
    Ok(())
}

#[cfg(test)]
mod managing_tests {
    use super::*;

    fn cfg() -> IdentitySettings {
        let mut c = IdentitySettings::default();
        c.user.set("triage".into(), "You triage alerts.".into());
        c.user.set("writer".into(), "You write reports.".into());
        c
    }

    #[test]
    fn a_name_cannot_be_empty_have_a_space_or_be_a_word_the_command_uses() {
        assert!(name_problem("triage").is_none());
        assert!(name_problem("  ").unwrap().contains("needs a name"));
        assert!(name_problem("two words").unwrap().contains("space"));
        assert!(name_problem("forensics").unwrap().contains("built-in"));
        for word in RESERVED {
            assert!(name_problem(word).is_some(), "{word}");
        }
    }

    #[test]
    fn saving_adds_replaces_in_place_and_refuses_to_overwrite_another() {
        let mut c = cfg();
        save_user(&mut c, None, "analyst", "You analyse.").unwrap();
        assert_eq!(
            c.user.names().collect::<Vec<_>>(),
            ["triage", "writer", "analyst"]
        );
        // A new one cannot take a name that is in use.
        assert!(
            save_user(&mut c, None, "triage", "x")
                .unwrap_err()
                .contains("already")
        );
        // Editing keeps the name and the place.
        save_user(&mut c, Some("triage"), "triage", "You triage well.").unwrap();
        assert_eq!(c.user.get("triage"), Some("You triage well."));
        assert_eq!(c.user.names().next(), Some("triage"));
        // A rename keeps the place, and a default that pointed at it follows.
        c.default = "writer".into();
        save_user(&mut c, Some("writer"), "author", "You write.").unwrap();
        assert_eq!(
            c.user.names().collect::<Vec<_>>(),
            ["triage", "author", "analyst"]
        );
        assert_eq!(c.default, "author");
        assert!(c.user.get("writer").is_none());
        // Not onto another's name, and not without text.
        assert!(save_user(&mut c, Some("author"), "triage", "x").is_err());
        assert!(
            save_user(&mut c, None, "empty", "   ")
                .unwrap_err()
                .contains("needs text")
        );
        assert!(save_user(&mut c, None, "off", "x").is_err());
    }

    #[test]
    fn deleting_removes_a_saved_identity_and_clears_a_default_that_was_it() {
        let mut c = cfg();
        c.default = "triage".into();
        delete_user(&mut c, "triage").unwrap();
        assert!(c.user.get("triage").is_none());
        assert_eq!(c.default, "");
        assert!(
            delete_user(&mut c, "triage")
                .unwrap_err()
                .contains("no saved")
        );
        assert!(
            delete_user(&mut c, "forensics")
                .unwrap_err()
                .contains("built in")
        );
    }

    #[test]
    fn the_default_is_a_known_identity_or_none() {
        let mut c = cfg();
        set_default(&mut c, "writer").unwrap();
        assert_eq!(c.default, "writer");
        set_default(&mut c, "forensics").unwrap();
        assert!(set_default(&mut c, "nobody").is_err());
        set_default(&mut c, "").unwrap();
        assert_eq!(c.default, "");
    }
}
