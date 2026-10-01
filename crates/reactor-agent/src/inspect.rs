//! What the model would be sent, piece by piece, and where each piece comes from.
//!
//! The same request the loop builds ([`crate::agent::Agent::context_preview`]), taken apart:
//! the system prompt's parts, the tool definitions, then the messages — each labelled with its
//! source, so a person can see *why* a string is in the context and what to change to remove it.

use crate::entry::EntryId;

/// Where a piece of the request comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    Base,
    Identity,
    Registry,
    Skills,
    Manifest,
    Reporting,
    Scenario,
    ToolDefinition,
    User,
    Assistant,
    ToolResult,
    Reduction,
    Reminder,
}

impl Origin {
    pub const ALL: [Origin; 13] = [
        Origin::Base,
        Origin::Identity,
        Origin::Registry,
        Origin::Skills,
        Origin::Manifest,
        Origin::Reporting,
        Origin::Scenario,
        Origin::ToolDefinition,
        Origin::User,
        Origin::Assistant,
        Origin::ToolResult,
        Origin::Reduction,
        Origin::Reminder,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Origin::Base => "Base prompt",
            Origin::Identity => "Identity",
            Origin::Registry => "Tool registry",
            Origin::Skills => "Skills",
            Origin::Manifest => "Manifest",
            Origin::Reporting => "Reporting",
            Origin::Scenario => "Scenario",
            Origin::ToolDefinition => "Tool definitions",
            Origin::User => "You",
            Origin::Assistant => "Assistant",
            Origin::ToolResult => "Tool results",
            Origin::Reduction => "Reductions",
            Origin::Reminder => "Reminder",
        }
    }

    /// Where it comes from, and how to change it.
    pub fn source(self) -> &'static str {
        match self {
            Origin::Base => "The built-in base prompt of reactor-agent. The same in every session.",
            Origin::Identity => {
                "The working persona: `/identity`, stored in the session; saved identities are in settings.json."
            }
            Origin::Registry => {
                "The tool catalogue (tools.toml), probed on this machine: present and active tools only. Change it with the Tools panel or `/tool`."
            }
            Origin::Skills => {
                "Skills of active tools: REactor's own, and upstream skills fetched by `reactor setup`."
            }
            Origin::Manifest => {
                "The session manifest: `/goal`, `/guidelines` and the agent's own steps. `/manifest off` leaves it out."
            }
            Origin::Reporting => "The reporting discipline: `/report on`, level and folder.",
            Origin::Scenario => "The current scenario phase's briefing: `/reactor-scenario`.",
            Origin::ToolDefinition => {
                "The tools the model may call, with their argument schemas. Sent with every request."
            }
            Origin::User => "A prompt you sent.",
            Origin::Assistant => {
                "The model's own earlier reply: its text, thinking and tool calls."
            }
            Origin::ToolResult => {
                "What a tool returned. Long output was cut; the session log keeps all of it (`history_read`)."
            }
            Origin::Reduction => {
                "A stand-in for a range of older entries that a context reduction took out. `/undo` brings them back."
            }
            Origin::Reminder => {
                "A reporting reminder added to this request only; it is never stored."
            }
        }
    }
}

/// Which part of the request a segment belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    System,
    Tools,
    Messages,
}

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::System => "System prompt",
            Section::Tools => "Tools",
            Section::Messages => "Messages",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub section: Section,
    pub origin: Origin,
    /// A short name: `Identity`, `bash`, `#14 tool result: bash`.
    pub label: String,
    /// The exact text sent (tool definitions and assistant messages are rendered readably).
    pub text: String,
    /// The estimate the budget uses.
    pub tokens: u64,
    /// The session entry it came from, for messages.
    pub entry: Option<EntryId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextPreview {
    pub segments: Vec<Segment>,
    pub total_tokens: u64,
    /// Real tokens per estimated token, as the provider reported.
    pub calibration: f64,
}

impl ContextPreview {
    /// Estimated tokens per origin, largest first.
    pub fn by_origin(&self) -> Vec<(Origin, u64)> {
        let mut out: Vec<(Origin, u64)> = Origin::ALL
            .iter()
            .map(|o| {
                (
                    *o,
                    self.segments
                        .iter()
                        .filter(|s| s.origin == *o)
                        .map(|s| s.tokens)
                        .sum::<u64>(),
                )
            })
            .filter(|(_, t)| *t > 0)
            .collect();
        out.sort_by_key(|o| std::cmp::Reverse(o.1));
        out
    }

    /// Everything as plain text, for the clipboard.
    pub fn as_text(&self) -> String {
        let mut out = String::new();
        for s in &self.segments {
            out.push_str(&format!(
                "===== {} · {} · ~{} tokens =====\n{}\n\n",
                s.origin.label(),
                s.label,
                s.tokens,
                s.text
            ));
        }
        out
    }
}
