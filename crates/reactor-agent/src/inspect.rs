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
    CaseGraph,
    Manifest,
    Reporting,
    Scenario,
    ToolDefinition,
    /// A tool of an MCP server (ADR-0048): its definition costs the same, and is told apart
    /// because the user chose it and its server decides how long it is.
    McpDefinition,
    User,
    /// A document, passages or a file the person gave the agent.
    Document,
    Assistant,
    ToolResult,
    Reduction,
    Reminder,
}

impl Origin {
    pub const ALL: [Origin; 16] = [
        Origin::Base,
        Origin::Identity,
        Origin::Registry,
        Origin::Skills,
        Origin::CaseGraph,
        Origin::Manifest,
        Origin::Reporting,
        Origin::Scenario,
        Origin::ToolDefinition,
        Origin::McpDefinition,
        Origin::User,
        Origin::Document,
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
            Origin::CaseGraph => "Case graph",
            Origin::Manifest => "Manifest",
            Origin::Reporting => "Reporting",
            Origin::Scenario => "Scenario",
            Origin::ToolDefinition => "Tool definitions",
            Origin::McpDefinition => "MCP tool definitions",
            Origin::User => "You",
            Origin::Document => "Given document",
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
            Origin::CaseGraph => {
                "How to use the case's graph. The same text in every request; the *Case graph* switch above the prompt takes it out, with the graph tools."
            }
            Origin::Manifest => {
                "The manifest: the case's goal and guidelines (`/goal`, `/guidelines`) and this session's steps. `/manifest off` leaves it out."
            }
            Origin::Reporting => "The reporting discipline: `/report on`, level and folder.",
            Origin::Scenario => "The current scenario phase's briefing: `/reactor-scenario`.",
            Origin::ToolDefinition => {
                "The tools the model may call, with their argument schemas. Sent with every request. A large group (the case graph's tools) is left out and listed by `tool_search`, which loads what is asked for."
            }
            Origin::McpDefinition => {
                "The definitions of the active tools of MCP servers, sent with every request once loaded. A server whose tools together pass the limit in Settings › Tools is listed by `tool_search` instead, and only what the agent loaded is here. Activate only what the work needs: Tools panel, `/tool`, `/toolset`."
            }
            Origin::User => "A prompt you sent.",
            Origin::Document => {
                "Text you gave the agent: from the Documents panel, or a # reference in a message. Remove it from the list of given documents to take it out."
            }
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

/// What an assistant message is made of: the model's reply text, its thinking and its tool
/// calls, in estimated tokens (the rest of the message's cost is counted as reply).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssistantParts {
    pub reply: u64,
    pub thinking: u64,
    pub calls: u64,
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
    /// For an assistant message: what it is made of.
    pub parts: Option<AssistantParts>,
}

/// What the tokens of a request are *of*, in the order the usage bar draws them. Finer than
/// [`Origin`] where it matters to the person (thinking apart from the reply, MCP apart from
/// our own tools) and coarser where it does not (the system prompt's five parts are one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    System,
    Manifest,
    Scenario,
    Tools,
    Mcp,
    You,
    /// Documents, passages and files the person gave the agent.
    Documents,
    Reply,
    Thinking,
    ToolCalls,
    ToolResults,
    Reductions,
}

/// The three parts of a request, as the bar groups its kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    System,
    Tools,
    Messages,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::System => "System prompt",
            Group::Tools => "Tool definitions",
            Group::Messages => "Messages",
        }
    }
}

impl Kind {
    pub const ALL: [Kind; 12] = [
        Kind::System,
        Kind::Manifest,
        Kind::Scenario,
        Kind::Tools,
        Kind::Mcp,
        Kind::You,
        Kind::Documents,
        Kind::Reply,
        Kind::Thinking,
        Kind::ToolCalls,
        Kind::ToolResults,
        Kind::Reductions,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kind::System => "Base, identity, registry, skills, graph",
            Kind::Manifest => "Manifest frame",
            Kind::Scenario => "Scenario and reporting",
            Kind::Tools => "REactor tools",
            Kind::Mcp => "MCP tools",
            Kind::You => "You",
            Kind::Documents => "Given documents",
            Kind::Reply => "Agent replies",
            Kind::Thinking => "Thinking",
            Kind::ToolCalls => "Tool calls",
            Kind::ToolResults => "Tool results",
            Kind::Reductions => "Reductions",
        }
    }

    pub fn group(self) -> Group {
        match self {
            Kind::System | Kind::Manifest | Kind::Scenario => Group::System,
            Kind::Tools | Kind::Mcp => Group::Tools,
            _ => Group::Messages,
        }
    }

    /// The pieces of the request this kind is made of, for showing them.
    pub fn origins(self) -> &'static [Origin] {
        match self {
            Kind::System => &[
                Origin::Base,
                Origin::Identity,
                Origin::Registry,
                Origin::Skills,
                Origin::CaseGraph,
            ],
            Kind::Manifest => &[Origin::Manifest],
            Kind::Scenario => &[Origin::Scenario, Origin::Reporting, Origin::Reminder],
            Kind::Tools => &[Origin::ToolDefinition],
            Kind::Mcp => &[Origin::McpDefinition],
            Kind::You => &[Origin::User],
            Kind::Documents => &[Origin::Document],
            Kind::Reply | Kind::Thinking | Kind::ToolCalls => &[Origin::Assistant],
            Kind::ToolResults => &[Origin::ToolResult],
            Kind::Reductions => &[Origin::Reduction],
        }
    }
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

    /// Estimated tokens per [`Kind`], in the bar's order, kinds with none left out.
    pub fn by_kind(&self) -> Vec<(Kind, u64)> {
        let mut totals: Vec<(Kind, u64)> = Kind::ALL.iter().map(|k| (*k, 0)).collect();
        let mut add = |kind: Kind, tokens: u64| {
            if let Some(slot) = totals.iter_mut().find(|(k, _)| *k == kind) {
                slot.1 += tokens;
            }
        };
        for s in &self.segments {
            match (s.origin, &s.parts) {
                (Origin::Assistant, Some(p)) => {
                    let parts = p.reply + p.thinking + p.calls;
                    // What the parts do not account for (the message's own overhead) is the
                    // reply's, so that the kinds add up to the segment.
                    add(Kind::Reply, p.reply + s.tokens.saturating_sub(parts));
                    add(Kind::Thinking, p.thinking);
                    add(Kind::ToolCalls, p.calls);
                }
                (Origin::Assistant, None) => add(Kind::Reply, s.tokens),
                (
                    Origin::Base
                    | Origin::Identity
                    | Origin::Registry
                    | Origin::Skills
                    | Origin::CaseGraph,
                    _,
                ) => add(Kind::System, s.tokens),
                (Origin::Manifest, _) => add(Kind::Manifest, s.tokens),
                (Origin::Scenario | Origin::Reporting | Origin::Reminder, _) => {
                    add(Kind::Scenario, s.tokens)
                }
                (Origin::ToolDefinition, _) => add(Kind::Tools, s.tokens),
                (Origin::McpDefinition, _) => add(Kind::Mcp, s.tokens),
                (Origin::User, _) => add(Kind::You, s.tokens),
                (Origin::Document, _) => add(Kind::Documents, s.tokens),
                (Origin::ToolResult, _) => add(Kind::ToolResults, s.tokens),
                (Origin::Reduction, _) => add(Kind::Reductions, s.tokens),
            }
        }
        totals.retain(|(_, t)| *t > 0);
        totals
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
