//! What a session is made of: entries in an append-only log.
//!
//! The format is REactor's own ([ADR-0036](../../../docs/adr/0036-reactor-owns-its-session-store-format.md)),
//! not pi's and not rig's — rig's message types are converted to and from these at
//! the model seam, so a rig release never changes what is on disk.
//!
//! An entry has an `id` unique within its session, a `parent` pointer (branches are
//! parent pointers, so a fork is just an append whose parent is not the tip), a
//! timestamp, and a kind. Bulk is never stored inline in the log line unbounded: a
//! tool result carries the model-visible text and, when that was cut, the address
//! of the blob holding every byte.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type EntryId = u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: EntryId,
    pub parent: Option<EntryId>,
    /// Unix milliseconds.
    pub ts: u64,
    #[serde(flatten)]
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Kind {
    /// The first line of every log: what this file is.
    Session {
        version: u32,
        session: String,
        cwd: String,
    },
    User {
        text: String,
    },
    Assistant {
        blocks: Vec<Block>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop: Option<String>,
    },
    ToolResult {
        call_id: String,
        name: String,
        /// What the model sees (possibly cut by the truncation policy).
        content: String,
        #[serde(default)]
        is_error: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blob: Option<Blob>,
    },
    /// Session state owned by another module: `manifest`, `identity`, `reporting`,
    /// `scenario`, `settings`. The latest one on the branch wins.
    Custom {
        key: String,
        data: Value,
    },
    /// A context reduction: the original entries stay in the log and this says what
    /// stands in for them ([`Reduction`]).
    Reduction(Reduction),
    /// Undo a reduction: it no longer hides anything.
    Restore {
        reduction: EntryId,
    },
    /// A human-readable marker (a branch's name, a note).
    Label {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    Thinking {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
}

/// Where the whole of a truncated tool output lives, and how big it was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blob {
    /// File name under the session's `blobs/` directory.
    pub file: String,
    /// The full size, in bytes.
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Drop mechanical and conceptual entries alike; leave stubs.
    Fade,
    /// Summarize everything.
    Compact,
    /// Drop mechanical entries (stubs), summarize conceptual ones.
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// The budget manager, at a tool-result boundary or before a request.
    Budget,
    /// A person asked.
    Manual,
}

/// One entry that was dropped rather than summarized, and how to get it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stub {
    pub entry: EntryId,
    /// `tool result`, `assistant`, `user`…
    pub what: String,
    /// The tool name and a digest of its arguments, or the first words of a message.
    pub detail: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reduction {
    pub mode: Mode,
    pub trigger: Trigger,
    /// Every entry this reduction hides — including earlier reductions it
    /// supersedes — in log order.
    pub covers: Vec<EntryId>,
    /// The summary that stands in for the conceptual entries, if any were summarized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// What was dropped, by address.
    #[serde(default)]
    pub stubs: Vec<Stub>,
    pub before_tokens: u64,
    pub after_tokens: u64,
}

impl Kind {
    /// A word for listings.
    pub fn label(&self) -> &'static str {
        match self {
            Kind::Session { .. } => "session",
            Kind::User { .. } => "user",
            Kind::Assistant { .. } => "assistant",
            Kind::ToolResult { .. } => "tool_result",
            Kind::Custom { .. } => "custom",
            Kind::Reduction(_) => "reduction",
            Kind::Restore { .. } => "restore",
            Kind::Label { .. } => "label",
        }
    }
}
