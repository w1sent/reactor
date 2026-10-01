//! Shared fixtures. Every test points a `Paths` at a throwaway config dir, so
//! nothing here ever reads or writes the real `~/.reactor/`.
#![allow(dead_code)]

use std::collections::HashMap;

use reactor_core::catalogue::DetectKind;
use reactor_core::model::{DetectSpec, ToolEntry};
use reactor_core::paths::{Paths, Shipped};
use reactor_core::probe::{ServiceInfo, ServiceState, Status};
use reactor_core::state::{Scope, State};
use tempfile::TempDir;

pub const FIXTURE_TOOLS: &str = r#"
version = 1

[platform]
prefer = ["pacman", "uv", "pip"]

[platform.manager]
pacman = { binary = "pacman", os = "linux", sudo = true }
brew   = { binary = "brew", os = "darwin" }
uv     = { binary = "uv" }
pip    = { binary = "pip3" }

[probe]
timeout = 2.0

[tool.alpha]
name   = "Alpha"
desc   = "does alpha things"
invoke = "alpha"
detect = { binary = "alpha" }
tags   = ["static", "python"]

[tool.alpha.install]
pacman = "pacman -S alpha"
uv     = "uv tool install alpha"
manual = "https://example.invalid/alpha"

[tool.beta]
name   = "Beta"
desc   = "does beta things"
invoke = "python3 -c 'import beta'"
detect = { python_module = "beta" }
tags   = ["dynamic", "python"]

[tool.gamma]
name    = "Gamma"
desc    = "does gamma things"
invoke  = "gamma"
detect  = { binary = "gamma" }
tags    = ["odd"]
service = { probe = ["gamma", "status"], label = "gamma", count = { pattern = 'ready$', noun = "worker" } }
"#;

pub const FIXTURE_TOOLSETS: &str = r#"
version = 1

[toolset.all]
desc = "everything"
all  = true

[toolset.static]
desc = "static only"
tags = ["static"]

[toolset.pair]
desc  = "explicit"
tools = ["alpha", "gamma"]

# Both tags are carried by something, so a union would widen these and an
# intersection narrows them -- which is what makes them worth asserting.
[toolset.narrow]
desc = "two tags, one tool"
tags = ["static", "python"]

[toolset.miss]
desc = "nothing carries both"
tags = ["static", "dynamic"]

[toolset.plus]
desc  = "a tag, and one more by name"
tags  = ["dynamic", "python"]
tools = ["gamma"]
"#;

pub struct Fx {
    pub dir: TempDir,
    pub paths: Paths,
}

impl Fx {
    pub fn new() -> Self {
        Self::with(FIXTURE_TOOLS, FIXTURE_TOOLSETS)
    }

    pub fn with(tools: &str, toolsets: &str) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("reactor-test-")
            .tempdir()
            .unwrap();
        std::fs::write(dir.path().join("tools.toml"), tools).unwrap();
        std::fs::write(dir.path().join("toolsets.toml"), toolsets).unwrap();
        // Like the Python fixture: the shipped copy *is* the fixture.
        let paths = Paths::new(dir.path(), Shipped::Dir(dir.path().to_path_buf()));
        Fx { dir, paths }
    }

    pub fn write_tools(&self, text: &str) {
        std::fs::write(self.dir.path().join("tools.toml"), text).unwrap();
    }

    pub fn state(&self, toolsets: &[&str], enabled: &[&str], disabled: &[&str]) -> State {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        State {
            path: self.dir.path().join("state.json"),
            scope: Scope::Machine,
            toolsets: v(toolsets),
            enabled: v(enabled),
            disabled: v(disabled),
        }
    }

    pub fn read_state_file(&self) -> Option<serde_json::Value> {
        let text = std::fs::read_to_string(self.dir.path().join("state.json")).ok()?;
        serde_json::from_str(&text).ok()
    }
}

/// A registry entry, present and active, doing nothing in particular.
pub fn entry(tid: &str) -> ToolEntry {
    ToolEntry {
        id: tid.into(),
        name: tid.into(),
        desc: format!("does {tid}"),
        source: None,
        invoke: tid.into(),
        tags: vec![],
        detect: DetectSpec {
            kind: DetectKind::Binary,
            value: tid.into(),
        },
        status: Status::Present,
        path: Some(format!("/bin/{tid}")),
        version: None,
        active: true,
        override_: None,
        service: None,
        skill: None,
        install: None,
    }
}

pub fn python_module(mut e: ToolEntry) -> ToolEntry {
    e.invoke = format!("python3 -c 'import {}'", e.id);
    e.detect = DetectSpec {
        kind: DetectKind::PythonModule,
        value: e.id.clone(),
    };
    e
}

pub fn service(label: &str, state: ServiceState, detail: Option<&str>) -> Option<ServiceInfo> {
    Some(ServiceInfo {
        label: Some(label.into()),
        state,
        detail: detail.map(str::to_string),
    })
}

pub fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

pub fn map<K: Into<String>, V>(items: impl IntoIterator<Item = (K, V)>) -> HashMap<String, V> {
    items.into_iter().map(|(k, v)| (k.into(), v)).collect()
}
