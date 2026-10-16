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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use rig_core::completion::message::{
    AssistantContent, Message, Reasoning, ReasoningContent, ToolChoice, ToolResultContent,
    UserContent,
};
use rig_core::completion::{CompletionModel, CompletionRequestBuilder, ToolDefinition};
use rig_core::streaming::StreamedAssistantContent;
use serde_json::Value;

use crate::classify::{StopKind, classify};
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

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LlmRequest {
    pub system: String,
    pub messages: Vec<Msg>,
    pub tools: Vec<ToolSpec>,
    /// The most the reply may be. Always set by the loop: a thinking model spends part of it
    /// thinking, and a provider with no default of its own refuses a request without one.
    pub max_tokens: Option<u64>,
    /// The context window the request is sized for. A server that sizes its window per
    /// request (Ollama) is told it; every other provider ignores it.
    pub num_ctx: Option<u64>,
    /// The model is told not to call any tool (the tools are there only so the request begins
    /// as the one before it did). A provider that cannot be told may call one anyway.
    pub no_tools: bool,
}

/// Progress while a reply streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delta {
    Text(String),
    Thinking(String),
    /// A tool call has begun (its arguments may still be arriving).
    ToolCall {
        name: String,
    },
}

/// A finished reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub blocks: Vec<Block>,
    pub usage: Option<Usage>,
    pub stop: Option<String>,
}

impl Reply {
    /// Why the reply ended.
    pub fn stop_kind(&self) -> StopKind {
        StopKind::from_label(self.stop.as_deref())
    }

    /// Whether the reply says anything a person or the next request could use: text or a
    /// tool call. Thinking alone is not an answer.
    pub fn has_output(&self) -> bool {
        self.blocks.iter().any(|b| match b {
            Block::Text { text } => !text.trim().is_empty(),
            Block::ToolCall { .. } => true,
            Block::Thinking { .. } => false,
        })
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = (&str, &str, &Value)> {
        self.blocks.iter().filter_map(|b| match b {
            Block::ToolCall {
                id,
                name,
                arguments,
            } => Some((id.as_str(), name.as_str(), arguments)),
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
    /// Provider-specific request parameters, such as the thinking level.
    params: Option<Value>,
    /// The server sizes its context window per request, so each request says how big
    /// (`num_ctx`): without it Ollama serves its own default and silently cuts the prompt.
    sets_num_ctx: bool,
    /// The provider's input count leaves out what it read from or wrote to its cache
    /// (Anthropic's does; OpenAI's counts it in), so those are added back.
    cache_counted_apart: bool,
}

impl<M: CompletionModel + Clone> RigLlm<M> {
    pub fn new(model: M, name: impl Into<String>) -> Self {
        RigLlm {
            model,
            name: name.into(),
            params: None,
            sets_num_ctx: false,
            cache_counted_apart: false,
        }
    }

    /// The provider reports cached input tokens apart from `input_tokens`, not within it.
    pub fn with_cache_counted_apart(mut self, on: bool) -> Self {
        self.cache_counted_apart = on;
        self
    }

    /// Send the request's `num_ctx` with it (for a server that sizes its window per request).
    pub fn with_num_ctx(mut self, on: bool) -> Self {
        self.sets_num_ctx = on;
        self
    }

    /// Send these provider parameters with every request.
    pub fn with_params(mut self, params: Option<Value>) -> Self {
        self.params = params;
        self
    }
}

/// Our messages → rig's. Consecutive tool results become the one user message
/// providers expect.
pub fn to_rig(messages: &[Msg]) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::new();
    for m in messages {
        match m {
            // Two messages of the person's in a row are one: some providers refuse
            // adjacent turns of the same role, and the rest read them as one anyway.
            Msg::User { text } => match out.last_mut() {
                Some(Message::User { content })
                    if content.iter().all(|c| matches!(c, UserContent::Text(_))) =>
                {
                    content.push(UserContent::text(text.clone()));
                }
                _ => out.push(Message::user(text.clone())),
            },
            Msg::Assistant { blocks } => {
                let content: Vec<AssistantContent> = blocks
                    .iter()
                    .map(|b| match b {
                        Block::Text { text } => AssistantContent::text(text.clone()),
                        Block::Thinking { text, signature } => AssistantContent::Reasoning(
                            Reasoning::new_with_signature(text, signature.clone()),
                        ),
                        Block::ToolCall {
                            id,
                            name,
                            arguments,
                        } => {
                            AssistantContent::tool_call(id.clone(), name.clone(), arguments.clone())
                        }
                    })
                    .collect();
                if !content.is_empty() {
                    out.push(Message::Assistant { id: None, content });
                }
            }
            Msg::ToolResult {
                call_id,
                name,
                content,
                is_error,
            } => {
                let text = if *is_error {
                    format!("error: {content}")
                } else {
                    content.clone()
                };
                let part = UserContent::tool_result_from_wire(
                    call_id.clone(),
                    name.clone(),
                    vec![ToolResultContent::text(text)],
                );
                match out.last_mut() {
                    Some(Message::User { content })
                        if content
                            .iter()
                            .all(|c| matches!(c, UserContent::ToolResult(_))) =>
                    {
                        content.push(part);
                    }
                    _ => out.push(Message::User {
                        content: vec![part],
                    }),
                }
            }
        }
    }
    out
}

impl<M: CompletionModel + Clone> RigLlm<M> {
    /// The provider parameters of one request: the configured ones, fitted to the reply's
    /// room, plus the window for a server that wants it.
    fn request_params(&self, req: &LlmRequest) -> Option<Value> {
        let mut params = self.params.clone();
        if let Some(p) = params.as_mut() {
            fit_thinking(p, req.max_tokens);
        }
        if self.sets_num_ctx
            && let Some(n) = req.num_ctx
        {
            let p = params.get_or_insert_with(|| Value::Object(Default::default()));
            if let Some(o) = p.as_object_mut() {
                o.insert("num_ctx".into(), n.into());
            }
        }
        params.filter(|p| p.as_object().is_none_or(|o| !o.is_empty()))
    }
}

/// What a provider reported, as REactor counts it: `input_tokens` is everything sent, and
/// `cached_input_tokens` the part of it that came from a cache. Anthropic reports the cached
/// part (and what it wrote to its cache) *apart* from its input count, so with caching on its
/// `input_tokens` is a few hundred while the prompt is tens of thousands; the others count it in.
pub fn usage_of(u: &rig_core::completion::Usage, cache_counted_apart: bool) -> Usage {
    let input = if cache_counted_apart {
        u.input_tokens + u.cached_input_tokens + u.cache_creation_input_tokens
    } else {
        u.input_tokens
    };
    Usage {
        input_tokens: input,
        output_tokens: u.output_tokens,
        cached_input_tokens: u.cached_input_tokens,
        fixed_estimate: 0,
    }
}

/// The least a thinking model must be left to answer with after it has thought.
pub const MIN_ANSWER_TOKENS: u64 = 1024;
/// The least thinking budget Anthropic accepts.
const MIN_THINKING_BUDGET: u64 = 1024;

/// Keep an Anthropic thinking budget under the reply's limit, with room left to answer.
/// A budget that cannot fit is dropped: thinking is off for this request rather than the
/// request refused (`budget_tokens` must be below `max_tokens`).
pub fn fit_thinking(params: &mut Value, max_tokens: Option<u64>) {
    let Some(max) = max_tokens else { return };
    let Some(obj) = params.as_object_mut() else {
        return;
    };
    let Some(t) = obj.get_mut("thinking").and_then(Value::as_object_mut) else {
        return;
    };
    if t.get("type").and_then(Value::as_str) != Some("enabled") {
        return;
    }
    let Some(budget) = t.get("budget_tokens").and_then(Value::as_u64) else {
        return;
    };
    let allowed = max.saturating_sub(MIN_ANSWER_TOKENS);
    if allowed < MIN_THINKING_BUDGET {
        obj.remove("thinking");
    } else if budget > allowed {
        t.insert("budget_tokens".into(), allowed.into());
    }
}

impl<M: CompletionModel + Clone + 'static> Llm for RigLlm<M> {
    async fn complete(
        &self,
        req: LlmRequest,
        on_delta: &mut (dyn FnMut(Delta) + Send),
    ) -> Result<Reply> {
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
                    .map(|t| ToolDefinition {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: t.parameters.clone(),
                    })
                    .collect(),
            );
        if let Some(n) = req.max_tokens {
            builder = builder.max_tokens(n);
        }
        if req.no_tools {
            builder = builder.tool_choice(ToolChoice::None);
        }
        if let Some(p) = self.request_params(&req) {
            builder = builder.additional_params(p);
        }
        let mut stream = builder
            .stream()
            .await
            .map_err(|e| classify(e.to_string()))?;

        let (mut usage, mut stop) = (None, None);
        while let Some(item) = stream.next().await {
            match item.map_err(|e| classify(e.to_string()))? {
                StreamedAssistantContent::Text(t) => on_delta(Delta::Text(t.text)),
                StreamedAssistantContent::ReasoningDelta { reasoning, .. } => {
                    on_delta(Delta::Thinking(reasoning))
                }
                StreamedAssistantContent::ToolCall { tool_call, .. } => on_delta(Delta::ToolCall {
                    name: tool_call.function.name.clone(),
                }),
                StreamedAssistantContent::Final(f) => {
                    usage = Some(usage_of(&f.usage, self.cache_counted_apart));
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
                AssistantContent::Text(t) if !t.text.is_empty() => Some(Block::Text {
                    text: t.text.clone(),
                }),
                AssistantContent::ToolCall(call) => Some(Block::ToolCall {
                    id: call.wire_call_id().to_string(),
                    name: call.function.name.clone(),
                    arguments: call.function.arguments.clone(),
                }),
                AssistantContent::Reasoning(r) => {
                    let mut text = String::new();
                    let mut signature = None;
                    for part in &r.content {
                        if let ReasoningContent::Text {
                            text: t,
                            signature: s,
                        } = part
                        {
                            text.push_str(t);
                            signature = s.clone().or(signature);
                        }
                    }
                    (!text.is_empty()).then_some(Block::Thinking { text, signature })
                }
                _ => None,
            })
            .collect();
        Ok(Reply {
            blocks,
            usage,
            stop,
        })
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
    name: String,
}

impl ScriptedLlm {
    pub fn new(replies: impl IntoIterator<Item = Result<Reply>>) -> Self {
        ScriptedLlm {
            replies: Mutex::new(replies.into_iter().collect()),
            requests: Mutex::new(vec![]),
            name: "scripted".into(),
        }
    }

    /// Another name, for a test that needs two models told apart.
    pub fn named(mut self, name: &str) -> Self {
        self.name = name.into();
        self
    }

    /// A reply that only talks.
    pub fn say(text: &str) -> Result<Reply> {
        Ok(Reply {
            blocks: vec![Block::Text { text: text.into() }],
            usage: None,
            stop: Some("stop".into()),
        })
    }

    /// A reply that calls tools.
    pub fn call(text: &str, calls: &[(&str, &str, Value)]) -> Result<Reply> {
        let mut blocks = Vec::new();
        if !text.is_empty() {
            blocks.push(Block::Text { text: text.into() });
        }
        blocks.extend(calls.iter().map(|(id, name, args)| Block::ToolCall {
            id: (*id).into(),
            name: (*name).into(),
            arguments: args.clone(),
        }));
        Ok(Reply {
            blocks,
            usage: None,
            stop: Some("tool_use".into()),
        })
    }

    pub fn requests(&self) -> Vec<LlmRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Llm for ScriptedLlm {
    async fn complete(
        &self,
        req: LlmRequest,
        on_delta: &mut (dyn FnMut(Delta) + Send),
    ) -> Result<Reply> {
        self.requests.lock().unwrap().push(req);
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(Error::Model("the script ran out of replies".into())))?;
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
        self.name.clone()
    }
}

// -- switching ---------------------------------------------------------------------------

/// A model that can be swapped while a session is open (a picker, `/model`). Each call
/// uses whatever is current when it starts; a turn already streaming finishes on the
/// model it began with.
pub struct Switchable<L> {
    inner: std::sync::Arc<Mutex<L>>,
}

impl<L> Clone for Switchable<L> {
    fn clone(&self) -> Self {
        Switchable {
            inner: self.inner.clone(),
        }
    }
}

impl<L: Llm + Clone> Switchable<L> {
    pub fn new(llm: L) -> Self {
        Switchable {
            inner: std::sync::Arc::new(Mutex::new(llm)),
        }
    }

    pub fn set(&self, llm: L) {
        *self.inner.lock().unwrap() = llm;
    }

    pub fn current(&self) -> L {
        self.inner.lock().unwrap().clone()
    }
}

impl<L: Llm + Clone> Llm for Switchable<L> {
    async fn complete(
        &self,
        req: LlmRequest,
        on_delta: &mut (dyn FnMut(Delta) + Send),
    ) -> Result<Reply> {
        let llm = self.current();
        llm.complete(req, on_delta).await
    }

    fn name(&self) -> String {
        self.current().name()
    }
}

// -- sharing and summarizing ---------------------------------------------------------------

impl<T: Llm + ?Sized> Llm for std::sync::Arc<T> {
    fn complete(
        &self,
        req: LlmRequest,
        on_delta: &mut (dyn FnMut(Delta) + Send),
    ) -> impl Future<Output = Result<Reply>> + Send {
        (**self).complete(req, on_delta)
    }
    fn name(&self) -> String {
        (**self).name()
    }
}

/// How much of a rejected request the next try aims at, after each "too long" rejection
/// (opencode's steps): the estimate ran low, so the first try aims well under it.
pub const SHRINK_STEPS: [f64; 3] = [0.7, 0.5, 0.35];

/// Call a model and give up on it if it goes quiet: no part of a reply, not even a thought,
/// for `idle`. A local model that has hung looks exactly like one that is thinking
/// hard, and nothing else would ever end the wait. `idle` of zero waits for ever.
pub async fn complete_watched<L: Llm + ?Sized>(
    llm: &L,
    req: LlmRequest,
    idle: Duration,
    mut on_delta: impl FnMut(Delta) + Send,
) -> Result<Reply> {
    if idle.is_zero() {
        return llm.complete(req, &mut on_delta).await;
    }
    let last = Arc::new(Mutex::new(Instant::now()));
    let seen = last.clone();
    let mut cb = move |d: Delta| {
        *seen.lock().unwrap() = Instant::now();
        on_delta(d);
    };
    let call = llm.complete(req, &mut cb);
    let stalled = async {
        loop {
            let quiet = last.lock().unwrap().elapsed();
            if quiet >= idle {
                break;
            }
            tokio::time::sleep(idle - quiet).await;
        }
    };
    tokio::select! {
        r = call => r,
        _ = stalled => Err(Error::Transient(format!(
            "no output from the model for {} seconds",
            idle.as_secs().max(1)
        ))),
    }
}

/// How long to wait before retry number `attempt` (1-based): doubling, with a ceiling.
pub fn backoff(base: Duration, attempt: usize) -> Duration {
    let doubled = base.saturating_mul(1u32 << (attempt.saturating_sub(1)).min(5) as u32);
    doubled.min(Duration::from_secs(15).max(base))
}

/// A summarizer that asks a model — the same one by default, or a cheaper one
/// configured separately (ADR-0038).
pub struct LlmSummarizer<L> {
    pub llm: L,
    /// Asked once, with the same request, if `llm` fails and is not this model: a
    /// summarizer that is down or out of quota should not stop the work.
    pub fallback: Option<L>,
    /// Tries after a failure worth another go (a busy server, a dropped stream).
    pub retries: usize,
    /// The wait before the first of them; each doubles it.
    pub retry_delay: Duration,
    /// How long the model may say nothing before it is given up on.
    pub idle_timeout: Duration,
}

impl<L> LlmSummarizer<L> {
    pub fn new(llm: L) -> Self {
        LlmSummarizer {
            llm,
            fallback: None,
            retries: 2,
            retry_delay: Duration::from_secs(1),
            idle_timeout: Duration::from_secs(300),
        }
    }

    pub fn with_fallback(mut self, fallback: L) -> Self {
        self.fallback = Some(fallback);
        self
    }
}

impl<L: Llm> LlmSummarizer<L> {
    /// The second look: given its own notes and asked what they leave out, a model writes them
    /// again with it added, or the same. A summary cannot be had again later, and one that
    /// lost a finding costs more than the call, so this is worth asking for when the reduction is
    /// one a person asked for. It is a check, not a rewrite: a failure, an empty answer, one that
    /// ignores the headings or one less than half as long as the notes (a model that took
    /// "write them again" as "write less") leaves the first notes as they were.
    async fn verify(
        &self,
        llm: &L,
        mut request: LlmRequest,
        notes: &str,
        template: bool,
    ) -> Option<String> {
        request.messages.push(Msg::Assistant {
            blocks: vec![Block::Text {
                text: notes.to_string(),
            }],
        });
        request.messages.push(Msg::User {
            text: crate::budget::SUMMARY_VERIFY.to_string(),
        });
        let reply = complete_watched(llm, request, self.idle_timeout, |_| {})
            .await
            .ok()?;
        let again = reply
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let usable = !again.trim().is_empty()
            && (!template || crate::budget::follows_template(&again))
            && again.trim().len() * 2 >= notes.trim().len();
        usable.then(|| again.trim().to_string())
    }

    async fn ask(&self, llm: &L, req: &crate::budget::SummaryRequest) -> Result<String> {
        // The notes are asked to stay short (`req.max_tokens` is what the budget plans for), but
        // the output limit is far wider: a reasoning model spends part of it thinking first, and
        // with only the notes' length to spend it can end with thinking and no notes at all.
        let system = format!(
            "{} Keep the notes under about {} words.",
            req.system,
            req.max_tokens * 3 / 4
        );
        // In a small window the reply gets no more than half of it.
        let out_cap = if req.window > 0 {
            (req.window / 2).max(512)
        } else {
            u64::MAX
        };
        let mut limit = req.max_tokens.saturating_mul(4).max(4096).min(out_cap);
        let fit = |target: u64| -> String {
            if req.parts.is_empty() {
                if crate::context::estimate_tokens(&req.transcript) <= target {
                    req.transcript.clone()
                } else {
                    crate::budget::cap(
                        &req.transcript,
                        (target as usize).saturating_mul(4).max(256),
                    )
                }
            } else {
                crate::budget::fit_parts(&req.parts, target)
            }
        };
        let system_tokens = crate::context::estimate_tokens(&system);
        let mut text = if req.input_cap > 0 {
            fit(req.input_cap.saturating_sub(system_tokens).max(256))
        } else {
            req.transcript.clone()
        };

        // Ask the way the main request was made, if this is the main model and the whole of
        // that request fits: the provider has most of it done already. It cannot be shrunk, so
        // a rejection, or a reply that is not notes, goes on to the flattened form below.
        let replay = req.replay.as_ref().filter(|r| r.model == llm.name());
        let replay_tokens = |r: &crate::budget::Replay| -> u64 {
            crate::context::estimate_tokens(&r.system)
                + r.tools.iter().map(crate::deferral::cost).sum::<u64>()
                + r.messages
                    .iter()
                    .map(crate::context::msg_tokens)
                    .sum::<u64>()
                + system_tokens
                + 64
        };
        let mut replaying =
            replay.is_some_and(|r| req.input_cap == 0 || replay_tokens(r) <= req.input_cap);

        let (mut thought, mut reminded, mut rungs, mut tries) = (false, false, 0usize, 0usize);
        let mut rejected: Option<u64> = None;
        loop {
            let reminder = if reminded {
                format!("\n\n{}", crate::budget::SUMMARY_REMINDER)
            } else {
                String::new()
            };
            let request = match replay.filter(|_| replaying) {
                Some(r) => {
                    let mut messages = r.messages.clone();
                    messages.push(Msg::User {
                        text: format!(
                            "{system}\n\nEverything above is the part of the session to be summarized. Do not call any tool; reply with the notes only.{reminder}"
                        ),
                    });
                    LlmRequest {
                        system: r.system.clone(),
                        messages,
                        tools: r.tools.clone(),
                        max_tokens: Some(limit),
                        num_ctx: (req.window > 0).then_some(req.window),
                        no_tools: true,
                    }
                }
                None => LlmRequest {
                    system: system.clone(),
                    messages: vec![Msg::User {
                        text: format!("{text}{reminder}"),
                    }],
                    tools: vec![],
                    max_tokens: Some(limit),
                    num_ctx: (req.window > 0).then_some(req.window),
                    no_tools: false,
                },
            };
            // The request that produces the notes is the start of the one that checks them.
            let asked = req.verify.then(|| request.clone());
            let sent = complete_watched(llm, request, self.idle_timeout, |_| {}).await;
            let reply = match sent {
                Ok(r) => r,
                // The whole request was too long: it cannot be cut, so ask the other way.
                Err(Error::Overflow(_)) if replaying => {
                    replaying = false;
                    continue;
                }
                // Too long for the model: aim lower, and lower again, then give up.
                Err(Error::Overflow(why)) => {
                    let Some(step) = SHRINK_STEPS.get(rungs) else {
                        return Err(Error::Overflow(format!(
                            "{why} (the text to summarize was cut to a third of its size and still did not fit: set the window and reserve in Settings → Context)"
                        )));
                    };
                    rungs += 1;
                    let size = *rejected.get_or_insert(crate::context::estimate_tokens(&text));
                    // Each try is smaller than the last, even where the ways of cutting come in
                    // coarse steps: asking again with the same text would be the same rejection.
                    let target = (((size as f64) * step) as u64)
                        .min(crate::context::estimate_tokens(&text).saturating_sub(1))
                        .max(64);
                    text = fit(target);
                    continue;
                }
                Err(Error::Transient(why)) => {
                    if tries >= self.retries {
                        return Err(Error::Transient(why));
                    }
                    tries += 1;
                    tokio::time::sleep(backoff(self.retry_delay, tries)).await;
                    continue;
                }
                Err(e) => return Err(e),
            };
            let notes = reply
                .blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !notes.trim().is_empty() {
                // Notes that ignore the headings are asked for once more; a second time they
                // are taken as they are, since a small model that cannot follow a template
                // can still write useful notes.
                if req.template && !crate::budget::follows_template(&notes) && !reminded {
                    reminded = true;
                    continue;
                }
                if let Some(asked) = asked
                    && let Some(better) = self.verify(llm, asked, &notes, req.template).await
                {
                    return Ok(better);
                }
                return Ok(notes);
            }
            let thinking: usize = reply
                .blocks
                .iter()
                .map(|b| {
                    if let Block::Thinking { text, .. } = b {
                        text.chars().count()
                    } else {
                        0
                    }
                })
                .sum();
            // A model that called a tool instead (it was told not to, and some providers cannot
            // be told) has not written notes: ask the other way, where there are no tools.
            if replaying && reply.tool_calls().next().is_some() {
                replaying = false;
                continue;
            }
            // Only thinking: it ran out of room before the notes. Once more, with a lot more.
            if thinking > 0 && !thought {
                thought = true;
                limit = limit.saturating_mul(3).min(out_cap.max(limit));
                continue;
            }
            // Nothing usable the main model's way: the flattened way gets one try of its own.
            if replaying {
                replaying = false;
                continue;
            }
            let what = if thinking > 0 {
                format!("only thinking ({thinking} characters) and no notes")
            } else {
                "nothing at all".to_string()
            };
            let cost = reply
                .usage
                .map(|u| format!(", {} output tokens", u.output_tokens))
                .unwrap_or_default();
            return Err(Error::Model(format!(
                "the summarizer returned {what} (stop: {}{cost}). If the model has a small context window, the text to summarize may not have fitted: set the window and reserve in Settings → Context",
                reply.stop.as_deref().unwrap_or("unknown")
            )));
        }
    }
}

impl<L: Llm> crate::budget::Summarizer for LlmSummarizer<L> {
    async fn summarize(&self, req: crate::budget::SummaryRequest) -> Result<String> {
        let first = match self.ask(&self.llm, &req).await {
            Ok(notes) => return Ok(notes),
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => e,
        };
        match &self.fallback {
            Some(main) if main.name() != self.llm.name() => {
                self.ask(main, &req).await.map_err(|second| {
                    Error::Model(format!(
                        "{first}; asking the main model instead failed too: {second}"
                    ))
                })
            }
            _ => Err(first),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_thinking_budget_stays_under_the_reply_limit_with_room_to_answer() {
        let mut p = json!({"thinking": {"type": "enabled", "budget_tokens": 8000}});
        fit_thinking(&mut p, Some(64_000));
        assert_eq!(
            p["thinking"]["budget_tokens"], 8000,
            "room enough: unchanged"
        );

        fit_thinking(&mut p, Some(6_000));
        assert_eq!(
            p["thinking"]["budget_tokens"],
            6_000 - MIN_ANSWER_TOKENS,
            "cut so the answer keeps its room"
        );

        fit_thinking(&mut p, Some(2_000));
        assert!(
            p.get("thinking").is_none(),
            "too little room to think at all: thinking is off for this request, not the request refused"
        );
    }

    #[test]
    fn parameters_that_are_not_an_enabled_budget_are_left_alone() {
        let mut off = json!({"thinking": {"type": "disabled"}});
        fit_thinking(&mut off, Some(10));
        assert_eq!(off, json!({"thinking": {"type": "disabled"}}));
        let mut other = json!({"reasoning": {"effort": "high"}});
        fit_thinking(&mut other, Some(10));
        assert_eq!(other, json!({"reasoning": {"effort": "high"}}));
        let mut nolimit = json!({"thinking": {"type": "enabled", "budget_tokens": 8000}});
        fit_thinking(&mut nolimit, None);
        assert_eq!(nolimit["thinking"]["budget_tokens"], 8000);
    }

    #[test]
    fn two_messages_of_the_persons_in_a_row_are_one_but_a_tool_result_stays_apart() {
        let m = |s: &str| Msg::User { text: s.into() };
        let out = to_rig(&[m("one"), m("two"), m("three")]);
        assert_eq!(out.len(), 1, "{out:?}");

        let result = Msg::ToolResult {
            call_id: "c".into(),
            name: "bash".into(),
            content: "out".into(),
            is_error: false,
        };
        let out = to_rig(&[
            Msg::Assistant {
                blocks: vec![Block::ToolCall {
                    id: "c".into(),
                    name: "bash".into(),
                    arguments: json!({}),
                }],
            },
            result,
            m("a reminder"),
        ]);
        assert_eq!(out.len(), 3, "results are not folded into text: {out:?}");
    }

    #[test]
    fn a_reply_is_what_it_says_not_what_it_thought() {
        let thinking = Reply {
            blocks: vec![Block::Thinking {
                text: "hm".into(),
                signature: None,
            }],
            usage: None,
            stop: Some("length".into()),
        };
        assert!(!thinking.has_output());
        assert_eq!(thinking.stop_kind(), StopKind::Length);
        let blank = Reply {
            blocks: vec![Block::Text {
                text: "  \n".into(),
            }],
            usage: None,
            stop: None,
        };
        assert!(!blank.has_output());
        let call = Reply {
            blocks: vec![Block::ToolCall {
                id: "c".into(),
                name: "bash".into(),
                arguments: json!({}),
            }],
            usage: None,
            stop: Some("tool_use".into()),
        };
        assert!(call.has_output());
    }

    fn rig_usage(
        input: u64,
        cached: u64,
        written: u64,
        output: u64,
    ) -> rig_core::completion::Usage {
        let mut u = rig_core::completion::Usage::new();
        u.input_tokens = input;
        u.cached_input_tokens = cached;
        u.cache_creation_input_tokens = written;
        u.output_tokens = output;
        u
    }

    #[test]
    fn anthropics_input_count_gets_its_cached_part_added_back_and_the_others_are_left_as_they_are()
    {
        // Anthropic: 300 tokens were new, 40,000 came from the cache, 1,200 were written to it.
        let u = rig_usage(300, 40_000, 1_200, 500);
        let a = usage_of(&u, true);
        assert_eq!(a.input_tokens, 41_500, "everything that was sent");
        assert_eq!(
            a.cached_input_tokens, 40_000,
            "and the part that came from the cache"
        );
        assert_eq!(a.output_tokens, 500);
        // OpenAI and the rest count the cached part within the input already.
        let o = usage_of(&rig_usage(41_500, 40_000, 0, 500), false);
        assert_eq!(o.input_tokens, 41_500);
        assert_eq!(o.cached_input_tokens, 40_000);
        // Without caching on, the two agree.
        assert_eq!(usage_of(&rig_usage(900, 0, 0, 5), true).input_tokens, 900);
    }

    /// A model that says nothing, for ever.
    struct Silent;
    impl Llm for Silent {
        async fn complete(
            &self,
            _req: LlmRequest,
            _on_delta: &mut (dyn FnMut(Delta) + Send),
        ) -> Result<Reply> {
            std::future::pending().await
        }
        fn name(&self) -> String {
            "silent".into()
        }
    }

    /// A model that thinks out loud for a while and then answers.
    struct Chatty;
    impl Llm for Chatty {
        async fn complete(
            &self,
            _req: LlmRequest,
            on_delta: &mut (dyn FnMut(Delta) + Send),
        ) -> Result<Reply> {
            for _ in 0..6 {
                tokio::time::sleep(Duration::from_millis(30)).await;
                on_delta(Delta::Thinking("…".into()));
            }
            Ok(Reply {
                blocks: vec![Block::Text {
                    text: "done".into(),
                }],
                usage: None,
                stop: Some("stop".into()),
            })
        }
        fn name(&self) -> String {
            "chatty".into()
        }
    }

    #[tokio::test]
    async fn a_model_that_goes_quiet_is_given_up_on_as_a_failure_worth_retrying() {
        let e = complete_watched(
            &Silent,
            LlmRequest::default(),
            Duration::from_millis(60),
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(e, Error::Transient(_)), "{e}");
    }

    #[tokio::test]
    async fn a_model_that_keeps_talking_is_not_given_up_on_however_long_it_takes() {
        // 180 ms in all, and no silence longer than the 100 ms allowed.
        let r = complete_watched(
            &Chatty,
            LlmRequest::default(),
            Duration::from_millis(100),
            |_| {},
        )
        .await
        .unwrap();
        assert!(r.has_output());
    }

    #[tokio::test]
    async fn a_watch_of_zero_waits_for_ever_so_it_is_off() {
        let waited = tokio::time::timeout(
            Duration::from_millis(80),
            complete_watched(&Silent, LlmRequest::default(), Duration::ZERO, |_| {}),
        )
        .await;
        assert!(waited.is_err(), "still waiting, as asked");
    }

    #[test]
    fn the_wait_between_retries_doubles_up_to_a_ceiling() {
        let base = Duration::from_secs(1);
        assert_eq!(backoff(base, 1), Duration::from_secs(1));
        assert_eq!(backoff(base, 2), Duration::from_secs(2));
        assert_eq!(backoff(base, 3), Duration::from_secs(4));
        assert_eq!(backoff(base, 10), Duration::from_secs(15));
        assert_eq!(backoff(Duration::ZERO, 3), Duration::ZERO);
    }
}
