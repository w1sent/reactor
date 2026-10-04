//! Tools: what the agent can do.
//!
//! A [`Tool`] is a spec the model reads and a function that runs. Availability follows
//! *state*, not a gate that refuses: the manifest's `update_steps` is advertised only
//! while a goal is set and the switch is on, the scenario's `reactor_phase_complete`
//! only while a scenario runs — and a stale call still gets an explanation rather than
//! a block ([ADR-0030](../../../docs/adr/0030-gated-tools-advertise-by-state.md)). The
//! graph tools follow the same rule: there while the session is attached to the case's
//! graph, gone while it is detached.
//!
//! There is deliberately **no permission or approval layer**
//! ([ADR-0033](../../../docs/adr/0033-reactor-is-a-rust-project-on-rig.md)): an agent
//! with `bash` can do anything its user can, so REactor is meant to be run inside a
//! VM or container, and nothing here pretends otherwise.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use reactor_core::Paths;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::context::SessionState;
use crate::llm::ToolSpec;
use crate::store::Store;

pub mod bash;
pub mod docs;
pub mod files;
pub mod graph;
pub mod session;

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a tool produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolOutput {
    /// Model-visible text. May be longer than the inline limit (the loop cuts it) or
    /// already a head-and-tail composite (see [`crate::truncate::CUT`]).
    pub text: String,
    pub is_error: bool,
    /// The whole output, on disk, when it was too big to hold: the loop keeps it as the
    /// entry's blob.
    pub full: Option<PathBuf>,
}

impl ToolOutput {
    pub fn ok(text: impl Into<String>) -> Self {
        ToolOutput {
            text: text.into(),
            ..Default::default()
        }
    }
    pub fn err(text: impl Into<String>) -> Self {
        ToolOutput {
            text: text.into(),
            is_error: true,
            full: None,
        }
    }
}

/// Everything a running tool can reach.
#[derive(Clone)]
pub struct ToolCtx {
    pub cwd: PathBuf,
    pub cancel: CancellationToken,
    /// Live output, as it arrives (a `bash` chunk). For a frontend; never for the model.
    pub emit: Arc<dyn Fn(&str) + Send + Sync>,
    pub store: Arc<Mutex<Store>>,
    pub paths: Paths,
    /// Where scenario directories live.
    pub scenarios_dir: PathBuf,
    /// The model on this turn (`provider/name`): the author of what it writes to the case.
    pub model: String,
    /// Makes the vectors a query is compared with, when the user chose a model.
    pub embedder: Option<Arc<dyn crate::embed::Embedder>>,
}

impl ToolCtx {
    pub fn state(&self) -> SessionState {
        SessionState::load(&self.store.lock().unwrap())
    }
}

pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    /// What to advertise for this state, where the description or the parameters depend
    /// on it (an option that cannot work is not offered).
    fn spec_for(&self, _state: &SessionState) -> ToolSpec {
        self.spec()
    }
    /// Whether to advertise it right now.
    fn available(&self, _state: &SessionState) -> bool {
        true
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput>;
}

#[derive(Default, Clone)]
pub struct Tools(Vec<Arc<dyn Tool>>);

impl Tools {
    pub fn new() -> Self {
        Tools(Vec::new())
    }

    pub fn with(mut self, tool: impl Tool + 'static) -> Self {
        self.0.push(Arc::new(tool));
        self
    }

    /// The tools REactor gives every session.
    pub fn standard(cwd: PathBuf) -> Self {
        Tools::new()
            .with(bash::Bash::new(cwd))
            .with(files::Read)
            .with(files::Write)
            .with(files::Edit)
            .with(session::HistoryIndex)
            .with(session::HistoryRead)
            .with(session::HistorySearch)
            .with(session::UpdateSteps)
            .with(session::PhaseComplete)
            .with(graph::GraphAdd)
            .with(graph::GraphAnnotate)
            .with(graph::GraphQuery)
            .with(graph::GraphPull)
            .with(graph::ArtifactRegister)
            .with(docs::DocsSearch)
    }

    /// What to advertise for this session state.
    pub fn specs(&self, state: &SessionState) -> Vec<ToolSpec> {
        self.0
            .iter()
            .filter(|t| t.available(state))
            .map(|t| t.spec_for(state))
            .collect()
    }

    pub fn find(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.0.iter().find(|t| t.spec().name == name)
    }
}

pub(crate) fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

pub(crate) fn usize_arg(args: &Value, key: &str) -> Option<usize> {
    args.get(key).and_then(Value::as_u64).map(|n| n as usize)
}
