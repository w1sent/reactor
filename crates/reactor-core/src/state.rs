//! Activation state: which toolsets are on, and per-tool overrides (ADR-0003,
//! ADR-0007, ADR-0011). Activation is a *hint* about what to advertise; nothing
//! here ever blocks a tool.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::catalogue::{Catalogue, Toolset};
use crate::err;
use crate::error::Result;
use crate::json::{io_reason, write_json_atomic};
use crate::paths::Paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// This session's own override (ADR-0038).
    Session,
    Machine,
    Project,
    Default,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Session => "session",
            Scope::Machine => "machine",
            Scope::Project => "project",
            Scope::Default => "default",
        }
    }
}

#[derive(Debug, Clone)]
pub struct State {
    pub path: PathBuf,
    pub scope: Scope,
    pub toolsets: Vec<String>,
    pub enabled: Vec<String>,
    pub disabled: Vec<String>,
}

/// `state.json`'s shape as `reactor state` emits it (insertion order).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateDoc {
    pub version: u32,
    pub toolsets: Vec<String>,
    pub tools: StateTools,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateTools {
    pub enabled: Vec<String>,
    pub disabled: Vec<String>,
}

fn sorted_unique(v: &[String]) -> Vec<String> {
    v.iter().cloned().collect::<BTreeSet<_>>().into_iter().collect()
}

impl State {
    pub fn empty(path: PathBuf, scope: Scope) -> Self {
        Self { path, scope, toolsets: vec![], enabled: vec![], disabled: vec![] }
    }

    pub fn as_doc(&self) -> StateDoc {
        StateDoc {
            version: 1,
            toolsets: sorted_unique(&self.toolsets),
            tools: StateTools {
                enabled: sorted_unique(&self.enabled),
                disabled: sorted_unique(&self.disabled),
            },
        }
    }

    pub fn save(&self) -> Result<()> {
        write_json_atomic(&self.path, &self.as_doc())
    }

    /// What state.json says about one tool on its own: "on", "off", or nothing.
    pub fn override_of(&self, tid: &str) -> Option<&'static str> {
        if self.disabled.iter().any(|t| t == tid) {
            Some("off")
        } else if self.enabled.iter().any(|t| t == tid) {
            Some("on")
        } else {
            None
        }
    }
}

/// Nearest `./.reactor/state.json` at or above `start` (ADR-0003).
pub fn project_state_path(start: &Path) -> Option<PathBuf> {
    let here = start.canonicalize().ok()?;
    here.ancestors()
        .map(|d| d.join(".reactor").join("state.json"))
        .find(|c| c.is_file())
}

pub fn load_state(paths: &Paths) -> Result<State> {
    // A session override, where one has been written, wins over everything.
    let session = paths.session_state.clone().filter(|p| p.is_file());
    let project = paths.cwd.as_deref().and_then(project_state_path);
    let (path, scope) = match (session, project) {
        (Some(s), _) => (s, Scope::Session),
        (None, Some(p)) => (p, Scope::Project),
        (None, None) => (paths.state_file(), Scope::Machine),
    };
    if !path.is_file() {
        return Ok(State::empty(paths.state_file(), Scope::Default));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| err!("{}: {}", path.display(), io_reason(&e)))?;
    let doc: Value = serde_json::from_str(&text).map_err(|e| err!("{}: {}", path.display(), e))?;
    let list = |v: Option<&Value>, what: &str| -> Result<Vec<String>> {
        match v {
            None | Some(Value::Null) => Ok(vec![]),
            Some(Value::Array(items)) => items
                .iter()
                .map(|i| i.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| err!("{}: {what}: expected a list of strings", path.display())),
            Some(_) => Err(err!("{}: {what}: expected a list of strings", path.display())),
        }
    };
    let tools = doc.get("tools");
    Ok(State {
        toolsets: list(doc.get("toolsets"), "toolsets")?,
        enabled: list(tools.and_then(|t| t.get("enabled")), "tools.enabled")?,
        disabled: list(tools.and_then(|t| t.get("disabled")), "tools.disabled")?,
        path,
        scope,
    })
}

/// Explicit `tools`, plus everything carrying *every* tag in `tags`.
///
/// Tags intersect (ADR-0013): the axes they name are independent, so listing
/// two of them is how you say "static *and* native". Union across groups is
/// already what activating two toolsets does; intersection has nowhere else to
/// live. An empty `tags` selects nothing -- vacuously true is the correct
/// reading and a useless one.
pub fn toolset_members(ts: &Toolset, cat: &Catalogue) -> Vec<String> {
    if ts.everything {
        return cat.order();
    }
    let mut members: BTreeSet<String> =
        ts.tools.iter().filter(|t| cat.contains(t)).cloned().collect();
    if !ts.tags.is_empty() {
        for t in &cat.tools {
            if ts.tags.iter().all(|w| t.tags.contains(w)) {
                members.insert(t.id.clone());
            }
        }
    }
    members.into_iter().collect()
}

/// What the active toolsets alone say, before any per-tool override.
pub fn base_ids(cat: &Catalogue, toolsets: &[Toolset], state: &State) -> BTreeSet<String> {
    if state.toolsets.is_empty() {
        return cat.ids().into_iter().collect();
    }
    let mut base = BTreeSet::new();
    for sid in &state.toolsets {
        if let Some(ts) = toolsets.iter().find(|t| &t.id == sid) {
            base.extend(toolset_members(ts, cat));
        }
    }
    base
}

/// Which tools the agent is told about. Never blocks anything (ADR-0007).
pub fn active_ids(cat: &Catalogue, toolsets: &[Toolset], state: &State) -> BTreeSet<String> {
    let mut base = base_ids(cat, toolsets, state);
    base.extend(state.enabled.iter().filter(|t| cat.contains(t)).cloned());
    for t in &state.disabled {
        base.remove(t);
    }
    base
}

/// Activation edits. `on = None` means "reset": drop the override, assert nothing.
///
/// Per-tool edits are *minimal* — an override is stored only where the toolsets
/// do not already produce the requested answer, so toggling a tool off and back
/// on leaves state.json exactly as it was (ADR-0011).
pub fn toggle(
    cat: &Catalogue,
    toolsets: &[Toolset],
    state: &mut State,
    ids: &[String],
    tools: bool,
    on: Option<bool>,
) -> Result<()> {
    let unknown: Vec<&str> = ids
        .iter()
        .filter(|i| {
            if tools {
                !cat.contains(i)
            } else {
                !toolsets.iter().any(|t| &t.id == *i)
            }
        })
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        return Err(err!(
            "unknown {}(s): {}",
            if tools { "tool" } else { "toolset" },
            unknown.join(", ")
        ));
    }

    if tools {
        let base = base_ids(cat, toolsets, state);
        for i in ids {
            state.enabled.retain(|x| x != i);
            state.disabled.retain(|x| x != i);
            match on {
                Some(true) if !base.contains(i) => state.enabled.push(i.clone()),
                Some(false) if base.contains(i) => state.disabled.push(i.clone()),
                _ => {}
            }
        }
    } else {
        for i in ids {
            state.toolsets.retain(|x| x != i);
            if on == Some(true) {
                state.toolsets.push(i.clone());
            }
        }
    }
    Ok(())
}
