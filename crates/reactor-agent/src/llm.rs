//! The seam to a model.
//!
//! [`Llm`] is the one thing the loop needs from a provider: given a request, stream
//! its progress and return the finished reply. [`RigLlm`] implements it over rig's
//! `CompletionModel`, which is what buys the 20-odd providers (ADR-0033); the
//! conversion to and from rig's message types lives here and nowhere else, so what is
//! stored on disk never depends on a rig release.
//!
//! [`ScriptedLlm`] is the test double: replies queued in advance, requests recorded.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::Mutex;

use futures::StreamExt;
use rig_core::completion::message::{AssistantContent, Message, Reasoning, ReasoningContent, ToolResultContent, UserContent};
use rig_core::completion::{CompletionModel, CompletionRequestBuilder, ToolDefinition};
use rig_core::streaming::StreamedAssistantContent;
use serde_json::Value;

use crate::context::Msg;
use crate::entry::{Block, Usage};
use crate::error::{Error, Result};

/// A tool as the model is told about it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments.
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmRequest {
    pub system: String,
    pub messages: Vec<Msg>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: Option<u64>,
}

/// Progress while a reply streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delta {
    Text(String),
    Thinking(String),
    /// A tool call has begun (its arguments may still be arriving).
    ToolCall { name: String },
}

/// A finished reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub blocks: Vec<Block>,
    pub usage: Option<Usage>,
    pub stop: Option<String>,
}

impl Reply {
    pub fn tool_calls(&self) -> impl Iterator<Item = (&str, &str, &Value)> {
        self.blocks.iter().filter_map(|b| match b {
            Block::ToolCall { id, name, arguments } => Some((id.as_str(), name.as_str(), arguments)),
            _ => None,
        })
    }
}

pub trait Llm: Send + Sync {
    /// Stream a reply, reporting progress to `on_delta`, and return it whole.
    fn complete(
        &self,
        req: LlmRequest,
        on_delta: &mut (dyn FnMut(Delta) + Send),
    ) -> impl Future<Output = Result<Reply>> + Send;

    /// A name for logs and entries.
    fn name(&self) -> String;
}

// -- rig ------------------------------------------------------------------------------

/// A model reached through rig.
#[derive(Clone)]
pub struct RigLlm<M> {
    model: M,
    name: String,
}

impl<M: CompletionModel + Clone> RigLlm<M> {
    pub fn new(model: M, name: impl Into<String>) -> Self {
        RigLlm { model, name: name.into() }
    }
}

/// Our messages → rig's. Consecutive tool results become the one user message
/// providers expect.
pub fn to_rig(messages: &[Msg]) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::new();
    for m in messages {
        match m {
            Msg::User { text } => out.push(Message::user(text.clone())),
            Msg::Assistant { blocks } => {
                let content: Vec<AssistantContent> = blocks
                    .iter()
                    .map(|b| match b {
                        Block::Text { text } => AssistantContent::text(text.clone()),
                        Block::Thinking { text, signature } => {
                            AssistantContent::Reasoning(Reasoning::new_with_signature(text, signature.clone()))
                        }
                        Block::ToolCall { id, name, arguments } => AssistantContent::tool_call(id.clone(), name.clone(), arguments.clone()),
                    })
                    .collect();
                if !content.is_empty() {
                    out.push(Message::Assistant { id: None, content });
                }
            }
            Msg::ToolResult { call_id, name, content, is_error } => {
                let text = if *is_error { format!("error: {content}") } else { content.clone() };
                let part = UserContent::tool_result_from_wire(call_id.clone(), name.clone(), vec![ToolResultContent::text(text)]);
                match out.last_mut() {
                    Some(Message::User { content }) if content.iter().all(|c| matches!(c, UserContent::ToolResult(_))) => {
                        content.push(part);
                    }
                    _ => out.push(Message::User { content: vec![part] }),
                }
            }
        }
    }
    out
}

impl<M: CompletionModel + Clone + 'static> Llm for RigLlm<M> {
    async fn complete(&self, req: LlmRequest, on_delta: &mut (dyn FnMut(Delta) + Send)) -> Result<Reply> {
        let mut history = to_rig(&req.messages);
        let Some(prompt) = history.pop() else {
            return Err(Error::Model("nothing to send".into()));
        };
        let mut builder = CompletionRequestBuilder::new(self.model.clone(), prompt)
            .preamble(req.system.clone())
            .messages(history)
            .tools(
                req.tools
                    .iter()
                    .map(|t| ToolDefinition { name: t.name.clone(), description: t.description.clone(), parameters: t.parameters.clone() })
                    .collect(),
            );
        if let Some(n) = req.max_tokens {
            builder = builder.max_tokens(n);
        }
        let mut stream = builder.stream().await.map_err(|e| Error::Model(e.to_string()))?;

        let (mut usage, mut stop) = (None, None);
        while let Some(item) = stream.next().await {
            match item.map_err(|e| Error::Model(e.to_string()))? {
                StreamedAssistantContent::Text(t) => on_delta(Delta::Text(t.text)),
                StreamedAssistantContent::ReasoningDelta { reasoning, .. } => on_delta(Delta::Thinking(reasoning)),
                StreamedAssistantContent::ToolCall { tool_call, .. } => on_delta(Delta::ToolCall { name: tool_call.function.name.clone() }),
                StreamedAssistantContent::Final(f) => {
                    usage = Some(Usage {
                        input_tokens: f.usage.input_tokens,
                        output_tokens: f.usage.output_tokens,
                        cached_input_tokens: f.usage.cached_input_tokens,
                    });
                    stop = f.finish_reason.map(|r| format!("{r:?}").to_lowercase());
                }
                _ => {}
            }
        }

        // The stream's own aggregation is the reply: text, whole tool calls, and
        // reasoning with its signature (which a provider needs handed back).
        let blocks = stream
            .choice
            .iter()
            .filter_map(|c| match c {
                AssistantContent::Text(t) if !t.text.is_empty() => Some(Block::Text { text: t.text.clone() }),
                AssistantContent::ToolCall(call) => Some(Block::ToolCall {
                    id: call.wire_call_id().to_string(),
                    name: call.function.name.clone(),
                    arguments: call.function.arguments.clone(),
                }),
                AssistantContent::Reasoning(r) => {
                    let mut text = String::new();
                    let mut signature = None;
                    for part in &r.content {
                        if let ReasoningContent::Text { text: t, signature: s } = part {
                            text.push_str(t);
                            signature = s.clone().or(signature);
                        }
                    }
                    (!text.is_empty()).then_some(Block::Thinking { text, signature })
                }
                _ => None,
            })
            .collect();
        Ok(Reply { blocks, usage, stop })
    }

    fn name(&self) -> String {
        self.name.clone()
    }
}

// -- scripted ---------------------------------------------------------------------------

/// A model that says what it was told to, and remembers what it was asked.
pub struct ScriptedLlm {
    replies: Mutex<VecDeque<Result<Reply>>>,
    pub requests: Mutex<Vec<LlmRequest>>,
}

impl ScriptedLlm {
    pub fn new(replies: impl IntoIterator<Item = Result<Reply>>) -> Self {
        ScriptedLlm { replies: Mutex::new(replies.into_iter().collect()), requests: Mutex::new(vec![]) }
    }

    /// A reply that only talks.
    pub fn say(text: &str) -> Result<Reply> {
        Ok(Reply { blocks: vec![Block::Text { text: text.into() }], usage: None, stop: Some("stop".into()) })
    }

    /// A reply that calls tools.
    pub fn call(text: &str, calls: &[(&str, &str, Value)]) -> Result<Reply> {
        let mut blocks = Vec::new();
        if !text.is_empty() {
            blocks.push(Block::Text { text: text.into() });
        }
        blocks.extend(calls.iter().map(|(id, name, args)| Block::ToolCall { id: (*id).into(), name: (*name).into(), arguments: args.clone() }));
        Ok(Reply { blocks, usage: None, stop: Some("tool_use".into()) })
    }

    pub fn requests(&self) -> Vec<LlmRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Llm for ScriptedLlm {
    async fn complete(&self, req: LlmRequest, on_delta: &mut (dyn FnMut(Delta) + Send)) -> Result<Reply> {
        self.requests.lock().unwrap().push(req);
        let reply = self.replies.lock().unwrap().pop_front().unwrap_or_else(|| Err(Error::Model("the script ran out of replies".into())))?;
        for b in &reply.blocks {
            match b {
                Block::Text { text } => on_delta(Delta::Text(text.clone())),
                Block::Thinking { text, .. } => on_delta(Delta::Thinking(text.clone())),
                Block::ToolCall { name, .. } => on_delta(Delta::ToolCall { name: name.clone() }),
            }
        }
        Ok(reply)
    }

    fn name(&self) -> String {
        "scripted".into()
    }
}

// -- sharing and summarizing ---------------------------------------------------------------

impl<T: Llm + ?Sized> Llm for std::sync::Arc<T> {
    fn complete(&self, req: LlmRequest, on_delta: &mut (dyn FnMut(Delta) + Send)) -> impl Future<Output = Result<Reply>> + Send {
        (**self).complete(req, on_delta)
    }
    fn name(&self) -> String {
        (**self).name()
    }
}

/// A summarizer that asks a model — the same one by default, or a cheaper one
/// configured separately (ADR-0038).
pub struct LlmSummarizer<L> {
    pub llm: L,
}

impl<L: Llm> crate::budget::Summarizer for LlmSummarizer<L> {
    async fn summarize(&self, req: crate::budget::SummaryRequest) -> Result<String> {
        let reply = self
            .llm
            .complete(
                LlmRequest {
                    system: req.system,
                    messages: vec![Msg::User { text: req.transcript }],
                    tools: vec![],
                    max_tokens: Some(req.max_tokens),
                },
                &mut |_| {},
            )
            .await?;
        Ok(reply
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}
