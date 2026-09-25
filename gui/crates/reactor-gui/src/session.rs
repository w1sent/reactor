//! The session view model: what the transcript, status bar and composer
//! render, built from the RPC event stream.
//!
//! The GUI holds *no facts* (gui/SPEC.md §4.4) — it renders what pi streams
//! and reconciles against `get_entries` on load. `message_end.message` is
//! authoritative; `message_update` deltas build the live partial. An unknown
//! event type lands in the debug view raw, never dropped and never fatal
//! (gui/SPEC.md §2's protocol-drift rule).

use serde_json::Value;

/// One renderable row of the transcript.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatItem {
    /// The user's prompt (plain text — markdown is the assistant's voice).
    User { text: String },
    /// Streaming or final assistant text, rendered as markdown.
    AssistantText { text: String, streaming: bool },
    /// A thinking block, rendered collapsed.
    Thinking { text: String, streaming: bool },
    /// One tool call with its live/final state.
    ToolCall {
        call_id: String,
        name: String,
        args: Value,
        output: String,
        is_error: bool,
        done: bool,
    },
    /// A custom session entry — reactor's extensions append these
    /// (`reactor-detail`, `pi-goal-setting`, `reactor-scenario`, …);
    /// rendered generically from the entry's data (gui/SPEC.md §6).
    CustomEntry { custom_type: String, raw: Value },
    /// An error pi reported — an assistant error or a compaction failure.
    Error { message: String },
    /// An event or record the GUI cannot render. Raw JSON in a debug view
    /// (gui/SPEC.md §2's protocol-drift rule).
    Unknown { raw: Value },
}

/// Where in the turn cycle the agent is — drives the composer's mode
/// (send vs follow-up) and the interrupt affordance (gui/SPEC.md §6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AgentPhase {
    /// The agent is settled; `Enter` sends.
    #[default]
    Idle,
    /// The agent is streaming; `Enter` queues a follow-up.
    Working,
    /// An automatic retry or compaction retry is in progress.
    Retrying,
    /// Compaction is running; prompts are rejected until it ends.
    Compacting,
}

/// The one session the GUI is attached to (gui/SPEC.md §3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Session {
    /// Transcript items, in stream order. Tool-call cards are keyed by
    /// `call_id` and updated in place as tool events arrive.
    pub items: Vec<ChatItem>,
    /// Extension statuses, key-sorted (`0-reactor` anchor first) — the
    /// footer's grammar survives into the status bar.
    pub statuses: Vec<(String, String)>,
    /// Non-envelope text widgets (`setWidget` from extensions), by key —
    /// rendered above the composer, `aboveEditor` parity with the TUI.
    pub widgets: Vec<(String, Vec<String>)>,
    /// Pending steering and follow-up queues, from `queue_update`.
    pub steering: Vec<String>,
    pub follow_up: Vec<String>,
    /// Current model, as `get_state` reports it — `provider/id`.
    pub model: Option<String>,
    /// Thinking level, e.g. `"medium"`.
    pub thinking_level: Option<String>,
    /// Context usage percent, from the last usage report.
    pub context_percent: Option<u64>,
    pub session_file: Option<String>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub phase: AgentPhase,
    /// The last error pi reported, if any.
    pub last_error: Option<String>,
    /// Where the assistant message currently streaming began in [`Self::items`].
    ///
    /// `message_end` is authoritative (gui/SPEC.md §2), so it rebuilds this
    /// message's blocks from the final message rather than trusting whatever
    /// the deltas left behind. Without the marker it could only guess — and
    /// guessing wrong meant pushing a second copy of a block instead of
    /// replacing the first.
    message_start_at: Option<usize>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// History: a session-file entry from `get_entries` (gui/SPEC.md §2) —
    /// a resumed session's transcript comes from here, since the event
    /// stream only carries new activity. Message entries render as their
    /// role; custom entries (`reactor-detail`, `pi-goal-setting`, …) render
    /// as their type with a generic fallback (gui/SPEC.md §6).
    pub fn ingest_entry(&mut self, entry: &Value) {
        if entry.get("type").and_then(Value::as_str) != Some("message") {
            // Other entry types (compaction records, settings entries) are
            // deliberately not transcript rows — v0.1 skips them.
            return;
        }
        let Some(message) = entry.get("message") else {
            return;
        };
        match message.get("role").and_then(Value::as_str) {
            Some("user") => self.items.push(ChatItem::User {
                text: content_text(message.get("content").unwrap_or(&Value::Null)),
            }),
            Some("assistant") => {
                if let Some(thinking) = assistant_thinking(message) {
                    self.push_thinking(thinking, false);
                }
                if let Some(text) = assistant_text(message) {
                    self.items.push(ChatItem::AssistantText {
                        text,
                        streaming: false,
                    });
                }
            }
            Some("toolResult") => {
                if let (Some(call_id), Some(tool_name)) = (
                    message.get("toolCallId").and_then(Value::as_str),
                    message.get("toolName").and_then(Value::as_str),
                ) {
                    self.items.push(ChatItem::ToolCall {
                        call_id: call_id.to_owned(),
                        name: tool_name.to_owned(),
                        args: Value::Null,
                        output: content_text(message.get("content").unwrap_or(&Value::Null)),
                        is_error: message
                            .get("isError")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        done: true,
                    });
                }
            }
            Some("custom") => {
                let custom_type = message
                    .get("customType")
                    .and_then(Value::as_str)
                    .unwrap_or("custom")
                    .to_owned();
                self.items.push(ChatItem::CustomEntry {
                    custom_type,
                    raw: message.clone(),
                });
            }
            _ => {}
        }
    }

    /// Ingest one incoming line. The GUI calls this from its pump task; the
    /// only thing that follows is a re-render.
    pub fn ingest(&mut self, incoming: &reactor_rpc::Incoming) {
        match incoming {
            reactor_rpc::Incoming::Event(event) => self.ingest_event(event),
            reactor_rpc::Incoming::ExtensionUiRequest(request) => {
                // Dialogs are answered by the GUI's overlay layer
                // (gui/SPEC.md §4.5); the view model only learns about
                // statuses and widgets.
                use reactor_rpc::protocol::UiMethod;
                match &request.method {
                    UiMethod::SetStatus {
                        status_key,
                        status_text,
                    } => {
                        self.set_status(status_key, status_text.clone());
                    }
                    UiMethod::SetWidget {
                        widget_key,
                        widget_lines,
                        widget_placement,
                    } => {
                        let _ = widget_placement; // above-editor parity; placement is the key's
                        self.set_widget(widget_key, widget_lines.clone().unwrap_or_default());
                    }
                    _ => {}
                }
            }
            // Late responses are debug-view material; nothing to render.
            reactor_rpc::Incoming::Response(_) => {}
            reactor_rpc::Incoming::Malformed { line, reason } => {
                self.items.push(ChatItem::Unknown {
                    raw: serde_json::json!({ "malformed": line, "reason": reason }),
                });
            }
            reactor_rpc::Incoming::Eof => {
                self.last_error = Some("pi exited".to_owned());
            }
        }
    }

    fn ingest_event(&mut self, event: &reactor_rpc::protocol::Event) {
        use reactor_rpc::protocol::Event as E;
        match event {
            E::SessionHeader { raw } => {
                self.session_id = raw.get("id").and_then(Value::as_str).map(str::to_owned);
                self.cwd = raw.get("cwd").and_then(Value::as_str).map(str::to_owned);
            }
            E::MessageUpdate { delta, .. } => {
                // Usage arrives here for the token counters; the context
                // percent comes from `get_session_stats` (gui/SPEC.md §6).
                self.apply_delta(delta);
            }
            E::MessageEnd { message } => {
                // `message_end.message` is authoritative (gui/SPEC.md §2):
                // drop whatever the deltas built for *this* message and lay
                // it out again from the final content, in the order the
                // message carries it. Reconciling block by block instead
                // meant a block the deltas had already closed got pushed a
                // second time rather than replaced.
                if message.get("role").and_then(Value::as_str) == Some("assistant") {
                    self.rebuild_assistant_message(message);
                }
                // Custom messages an extension sent via `pi.sendMessage`
                // (e.g. a scenario briefing) render as their type.
                if message.get("role").and_then(Value::as_str) == Some("custom") {
                    if let Some(custom_type) = message.get("customType").and_then(Value::as_str) {
                        self.items.push(ChatItem::CustomEntry {
                            custom_type: custom_type.to_owned(),
                            raw: message.clone(),
                        });
                    }
                }
            }
            E::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                self.items.push(ChatItem::ToolCall {
                    call_id: tool_call_id.to_owned(),
                    name: tool_name.to_owned(),
                    args: args.clone(),
                    output: String::new(),
                    is_error: false,
                    done: false,
                });
            }
            E::ToolExecutionUpdate {
                tool_call_id,
                partial_result,
                ..
            } => {
                // `partialResult` is the accumulated output so far — replace,
                // never append (gui/SPEC.md §2).
                if let Some(ChatItem::ToolCall { output, .. }) = self.tool_call_mut(tool_call_id) {
                    *output = content_text(partial_result);
                }
            }
            E::ToolExecutionEnd {
                tool_call_id,
                result,
                is_error: _,
                ..
            } => {
                if let Some(ChatItem::ToolCall {
                    output,
                    is_error,
                    done,
                    ..
                }) = self.tool_call_mut(tool_call_id)
                {
                    *output = content_text(result);
                    *is_error = *is_error;
                    *done = true;
                }
            }
            E::QueueUpdate {
                steering,
                follow_up,
            } => {
                self.steering = steering.clone();
                self.follow_up = follow_up.clone();
            }
            E::AgentStart => {
                self.phase = AgentPhase::Working;
                // A fresh agent run is starting. If the previous one left a
                // streaming item unfinished — pi's own auto-retry (verified
                // against a live 502/TLS-timeout run: a failed attempt can
                // stream partial thinking/text deltas, then fail *without*
                // ever sending that attempt's `message_end`, straight into
                // `auto_retry_start`) — drop it before the retry's fresh
                // `text_start`/`thinking_start` deltas arrive. Otherwise
                // `apply_delta`'s "append to the existing streaming item"
                // rule (`TextStart` only pushes a new item when none is
                // already streaming) appends the retry's full response onto
                // the failed attempt's stale partial text with no
                // separator — this *was* the "the same prompt got answered
                // twice, concatenated" bug; it was never a double send.
                self.discard_unfinished_streaming_items();
            }
            E::AgentSettled => self.phase = AgentPhase::Idle,
            E::AutoRetryStart { .. } => self.phase = AgentPhase::Retrying,
            E::AutoRetryEnd { success, .. } => {
                self.phase = AgentPhase::Working;
                if !success {
                    // Final failure: the error text arrived on the tail event.
                }
            }
            E::CompactionStart { .. } => self.phase = AgentPhase::Compacting,
            E::CompactionEnd { error_message, .. } => {
                self.phase = AgentPhase::Working;
                if let Some(error) = error_message {
                    self.last_error = Some(error.clone());
                }
            }
            E::ExtensionError { error, .. } => {
                if let Some(error) = error {
                    self.last_error = Some(error.clone());
                }
            }
            E::Unknown { raw } => {
                // Rendered raw in the debug view, never dropped (gui/SPEC.md §2).
                self.items.push(ChatItem::Unknown { raw: raw.clone() });
            }
            // Events the transcript does not render (the composer, the status
            // bar and the bridge consume them where relevant).
            E::AgentEnd { .. } | E::TurnStart | E::TurnEnd { .. } => {}
            E::MessageStart { message } => {
                // Remember where this message's blocks start so `message_end`
                // can replace them wholesale.
                if message.get("role").and_then(Value::as_str) == Some("assistant") {
                    self.message_start_at = Some(self.items.len());
                }
            }
            E::BashExecutionUpdate { .. } => {}
            E::EntryAppended => {}
        }
    }

    /// Apply one streaming delta to the in-progress message item.
    fn apply_delta(&mut self, delta: &reactor_rpc::protocol::AssistantMessageEvent) {
        use reactor_rpc::protocol::AssistantMessageEvent as D;
        match delta {
            D::TextStart { .. } => {
                if !self.has_streaming_assistant() {
                    self.items.push(ChatItem::AssistantText {
                        text: String::new(),
                        streaming: true,
                    });
                }
            }
            D::TextDelta { delta, .. } => self.append_to_streaming(delta),
            D::TextEnd {
                text: Some(text), ..
            } => {
                // `text_end.content` is the block's *complete* text, not a
                // trailing delta (verified live against pi 0.87.0).
                // Appending it to the deltas already accumulated is what
                // rendered every answer twice, run together with no
                // separator: "…what would you like to do?Hi! I'm ready…".
                self.set_streaming_text(text);
                self.finalize_streaming_assistant();
            }
            D::ThinkingStart { .. } => {
                if !self.has_streaming_thinking() {
                    self.items.push(ChatItem::Thinking {
                        text: String::new(),
                        streaming: true,
                    });
                }
            }
            D::ThinkingDelta { delta, .. } => self.append_to_thinking(delta),
            D::ThinkingEnd {
                thinking: Some(thinking),
                ..
            } => {
                // Same contract as `text_end`: the complete block, replacing
                // the deltas rather than extending them.
                self.set_streaming_thinking(thinking);
                self.finalize_thinking();
            }
            _ => {}
        }
    }

    /// Index of the block a thinking/text delta belongs to: the newest one
    /// of that kind still streaming.
    ///
    /// Searched from the back rather than read off `last()`, because pi
    /// interleaves content blocks — it closes the *thinking* block (index 0)
    /// only after the *text* block (index 1) has started streaming — so the
    /// block a delta belongs to is routinely not the newest item. Reading
    /// `last()` there appended thinking onto the text block, or pushed a
    /// duplicate block when the kinds did not match.
    fn streaming_block(&self, thinking: bool) -> Option<usize> {
        self.items.iter().rposition(|item| match item {
            ChatItem::Thinking { streaming, .. } => thinking && *streaming,
            ChatItem::AssistantText { streaming, .. } => !thinking && *streaming,
            _ => false,
        })
    }

    fn has_streaming_assistant(&self) -> bool {
        self.streaming_block(false).is_some()
    }

    fn has_streaming_thinking(&self) -> bool {
        self.streaming_block(true).is_some()
    }

    fn append_to_streaming(&mut self, delta: &str) {
        match self.streaming_block(false) {
            Some(at) => {
                if let ChatItem::AssistantText { text, .. } = &mut self.items[at] {
                    text.push_str(delta);
                }
            }
            None => self.items.push(ChatItem::AssistantText {
                text: delta.to_owned(),
                streaming: true,
            }),
        }
    }

    fn append_to_thinking(&mut self, delta: &str) {
        match self.streaming_block(true) {
            Some(at) => {
                if let ChatItem::Thinking { text, .. } = &mut self.items[at] {
                    text.push_str(delta);
                }
            }
            None => self.items.push(ChatItem::Thinking {
                text: delta.to_owned(),
                streaming: true,
            }),
        }
    }

    /// Replace the streaming text block's contents outright — for
    /// `text_end`, which carries the whole block rather than a delta.
    fn set_streaming_text(&mut self, full: &str) {
        match self.streaming_block(false) {
            Some(at) => {
                if let ChatItem::AssistantText { text, .. } = &mut self.items[at] {
                    *text = full.to_owned();
                }
            }
            None => self.items.push(ChatItem::AssistantText {
                text: full.to_owned(),
                streaming: true,
            }),
        }
    }

    /// The thinking-block counterpart of [`Self::set_streaming_text`].
    fn set_streaming_thinking(&mut self, full: &str) {
        match self.streaming_block(true) {
            Some(at) => {
                if let ChatItem::Thinking { text, .. } = &mut self.items[at] {
                    *text = full.to_owned();
                }
            }
            None => self.items.push(ChatItem::Thinking {
                text: full.to_owned(),
                streaming: true,
            }),
        }
    }

    fn push_thinking(&mut self, thinking: String, streaming: bool) {
        self.items.push(ChatItem::Thinking {
            text: thinking,
            streaming,
        });
    }

    /// Lay out a finished assistant message from its authoritative content,
    /// discarding the preview its deltas built.
    ///
    /// Only this message's own thinking/text blocks are dropped — anything
    /// else that landed in the meantime (a tool card, an error) is left
    /// where it is, and so is every earlier message.
    fn rebuild_assistant_message(&mut self, message: &Value) {
        // No marker means the deltas arrived without a `message_start`
        // (history replay, a truncated stream). Fall back to the run of
        // thinking/text blocks at the tail: those are what this message
        // built.
        let fallback = self.items.len()
            - self
                .items
                .iter()
                .rev()
                .take_while(|item| {
                    matches!(
                        item,
                        ChatItem::AssistantText { .. } | ChatItem::Thinking { .. }
                    )
                })
                .count();
        let from = self
            .message_start_at
            .take()
            .unwrap_or(fallback)
            .min(self.items.len());
        let mut at = from;
        while at < self.items.len() {
            if matches!(
                self.items[at],
                ChatItem::AssistantText { .. } | ChatItem::Thinking { .. }
            ) {
                self.items.remove(at);
            } else {
                at += 1;
            }
        }
        if let Some(thinking) = assistant_thinking(message) {
            self.push_thinking(thinking, false);
        }
        if let Some(text) = assistant_text(message) {
            self.items.push(ChatItem::AssistantText {
                text,
                streaming: false,
            });
        }
    }

    fn finalize_streaming_assistant(&mut self) {
        if let Some(at) = self.streaming_block(false) {
            if let ChatItem::AssistantText { streaming, .. } = &mut self.items[at] {
                *streaming = false;
            }
        }
    }

    fn finalize_thinking(&mut self) {
        if let Some(at) = self.streaming_block(true) {
            if let ChatItem::Thinking { streaming, .. } = &mut self.items[at] {
                *streaming = false;
            }
        }
    }

    fn tool_call_mut(&mut self, call_id: &str) -> Option<&mut ChatItem> {
        self.items
            .iter_mut()
            .rev()
            .find(|item| matches!(item, ChatItem::ToolCall { call_id: id, .. } if id == call_id))
    }

    /// Drop trailing items still marked `streaming: true` — content an
    /// aborted attempt never got a `message_end` to finalize. Called when a
    /// fresh `agent_start` arrives (see there for why): whatever was
    /// mid-stream when the previous attempt died belongs to a turn that is
    /// being retried, not to the one about to start.
    fn discard_unfinished_streaming_items(&mut self) {
        while matches!(
            self.items.last(),
            Some(
                ChatItem::AssistantText {
                    streaming: true,
                    ..
                } | ChatItem::Thinking {
                    streaming: true,
                    ..
                }
            )
        ) {
            self.items.pop();
        }
    }
}

impl Session {
    /// One extension status entry: `None` clears it (pi's setStatus
    /// contract). Key-sorted on read so REactor's anchor (`0-reactor`)
    /// leads — the footer grammar survives into the status bar.
    pub fn set_status(&mut self, key: &str, text: Option<String>) {
        self.statuses.retain(|(k, _)| k != key);
        if let Some(text) = text {
            self.statuses.push((key.to_owned(), text));
            self.statuses.sort_by(|a, b| a.0.cmp(&b.0));
        }
    }

    /// One non-envelope text widget (`setWidget` without the `reactor:`
    /// prefix); `None`/empty clears it.
    pub fn set_widget(&mut self, key: &str, lines: Vec<String>) {
        self.widgets.retain(|(k, _)| k != key);
        if !lines.is_empty() {
            self.widgets.push((key.to_owned(), lines));
        }
    }
}

/// The text of a content value: `"a string"` or
/// `[{"type":"text","text":"…"}, …]` — the shape of tool results.
pub fn content_text(content: &Value) -> String {
    // Tool results arrive as the whole message (`{"content": […]}`) in the
    // `result` field — unwrap the wrapper when it is one.
    let content = match content {
        Value::Object(o) if o.contains_key("content") && o.get("role").is_none() => {
            o.get("content").unwrap()
        }
        other => other,
    };
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| {
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    block.get("text").and_then(Value::as_str)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// The concatenated text content of an assistant message (`content` array of
/// `{"type":"text","text":…}` blocks).
pub fn assistant_text(message: &Value) -> Option<String> {
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let text = content_text(message.get("content")?);
    if text.is_empty() { None } else { Some(text) }
}

/// The concatenated thinking content of an assistant message.
pub fn assistant_thinking(message: &Value) -> Option<String> {
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let thinking = message
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|block| {
            if block.get("type").and_then(Value::as_str) == Some("thinking") {
                block.get("thinking").and_then(Value::as_str)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("");
    if thinking.is_empty() {
        None
    } else {
        Some(thinking)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// One streaming turn, from deltas to the authoritative `message_end` —
    /// the exact sequence pi sends for a text answer (gui/SPEC.md §2).
    #[test]
    fn streaming_assembles_from_deltas_and_message_end_is_authoritative() {
        let mut s = Session::new();
        for line in [
            r#"{"type":"agent_start"}"#,
            r#"{"type":"turn_start"}"#,
            r#"{"type":"message_start","message":{"role":"assistant","content":[]}}"#,
            r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Hello"}}"#,
            r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":" world"}}"#,
        ] {
            s.ingest_event(&reactor_rpc::Event::from_value(
                serde_json::from_str::<Value>(line).unwrap(),
            ));
        }

        // The live partial is streaming.
        assert_eq!(
            s.items,
            vec![ChatItem::AssistantText {
                text: "Hello world".into(),
                streaming: true
            }]
        );
        assert_eq!(s.phase, AgentPhase::Working);

        // `message_end` is authoritative — its message replaces the partial.
        let final_message = json!({
            "role": "assistant",
            "content": [{"type": "text", "text": "Hello, world — final."}]
        });
        s.ingest_event(&reactor_rpc::Event::from_value(
            json!({"type": "message_end", "message": final_message}),
        ));
        assert_eq!(
            s.items,
            vec![ChatItem::AssistantText {
                text: "Hello, world — final.".into(),
                streaming: false
            }]
        );
    }

    #[test]
    fn tool_calls_update_in_place_by_call_id() {
        let mut s = Session::new();
        for event in [
            json!({"type":"tool_execution_start","toolCallId":"c1","toolName":"bash","args":{"command":"ls"}}),
            json!({"type":"tool_execution_update","toolCallId":"c1","partialResult":{"content":[{"type":"text","text":"total 0"}]}}),
            json!({"type":"tool_execution_end","toolCallId":"c1","result":{"content":[{"type":"text","text":"total 0"}]},"isError":false}),
        ] {
            s.ingest_event(&reactor_rpc::Event::from_value(event));
        }
        assert_eq!(
            s.items,
            vec![ChatItem::ToolCall {
                call_id: "c1".into(),
                name: "bash".into(),
                args: json!({"command": "ls"}),
                output: "total 0".into(),
                is_error: false,
                done: true,
            }]
        );
    }

    /// The exact delta order pi 0.87.0 streams for a thinking model, captured
    /// live: the thinking block opens first but is *closed after* the text
    /// block has already started, and both `*_end` events carry the whole
    /// block in `content` rather than a trailing delta.
    ///
    /// Every part of that tripped the old assembly: `text_end`'s full text
    /// was appended to the deltas (the answer rendered twice, run together
    /// with no separator), `thinking_end` was read from a `thinking` field pi
    /// does not send (so the block never closed), and `message_end` then
    /// pushed fresh blocks beside the ones already there. One "hi" rendered
    /// as thinking, doubled answer, thinking again, answer again.
    #[test]
    fn interleaved_thinking_and_text_blocks_each_render_once() {
        let mut s = Session::new();
        let events = [
            json!({"type":"message_start","message":{"role":"assistant","content":[]}}),
            json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":0}}),
            json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"The user "}}),
            json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"said hi."}}),
            json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":1}}),
            json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":1,"delta":"Hi! I'm "}}),
            json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":1,"delta":"ready."}}),
            // Closes the *thinking* block, two blocks back — not `last()`.
            json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_end","contentIndex":0,"content":"The user said hi."}}),
            json!({"type":"message_update","assistantMessageEvent":{"type":"text_end","contentIndex":1,"content":"Hi! I'm ready."}}),
        ];
        for event in events {
            s.ingest_event(&reactor_rpc::Event::from_value(event));
        }

        // The live preview: one thinking block, one answer, neither doubled.
        assert_eq!(
            s.items,
            vec![
                ChatItem::Thinking {
                    text: "The user said hi.".into(),
                    streaming: false
                },
                ChatItem::AssistantText {
                    text: "Hi! I'm ready.".into(),
                    streaming: false
                },
            ]
        );

        // And `message_end` replaces them rather than adding a second set.
        s.ingest_event(&reactor_rpc::Event::from_value(json!({
            "type": "message_end",
            "message": {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "The user said hi."},
                {"type": "text", "text": "Hi! I'm ready."}
            ]}
        })));
        assert_eq!(
            s.items,
            vec![
                ChatItem::Thinking {
                    text: "The user said hi.".into(),
                    streaming: false
                },
                ChatItem::AssistantText {
                    text: "Hi! I'm ready.".into(),
                    streaming: false
                },
            ],
            "one turn must render as exactly one thinking block and one answer"
        );
    }

    /// A live-verified pi trace (a real 502/TLS-handshake-timeout auto-retry
    /// against a cloud provider): the failed attempt streams a partial
    /// delta, then dies straight into `auto_retry_start` — no `message_end`
    /// ever finalizes it. The retry's fresh `agent_start` must drop that
    /// dangling partial, or the retry's full text lands appended onto it
    /// with no separator: one run-on paragraph that reads as "the same
    /// answer twice" — reported as "you send the prompt two times to pi",
    /// which the RPC trace showed was never true; pi answered once, after
    /// one retry.
    #[test]
    fn a_failed_attempts_partial_text_does_not_survive_into_the_retry() {
        let mut s = Session::new();
        for line in [
            r#"{"type":"agent_start"}"#,
            r#"{"type":"turn_start"}"#,
            r#"{"type":"message_start","message":{"role":"assistant","content":[]}}"#,
            r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Hi! I'm ready to help"}}"#,
            r#"{"type":"auto_retry_start","attempt":1,"errorMessage":"502 TLS handshake timeout"}"#,
            r#"{"type":"agent_start"}"#,
            r#"{"type":"turn_start"}"#,
            r#"{"type":"message_start","message":{"role":"assistant","content":[]}}"#,
        ] {
            s.ingest_event(&reactor_rpc::Event::from_value(
                serde_json::from_str::<Value>(line).unwrap(),
            ));
        }
        // The failed attempt's partial must be gone before the retry starts
        // streaming, not sitting there waiting to be appended onto.
        assert_eq!(s.items, vec![]);

        let final_message = json!({
            "role": "assistant",
            "content": [{"type": "text", "text": "Hi! I'm ready to help with your project."}]
        });
        s.ingest_event(&reactor_rpc::Event::from_value(
            json!({"type": "message_end", "message": final_message}),
        ));
        assert_eq!(
            s.items,
            vec![ChatItem::AssistantText {
                text: "Hi! I'm ready to help with your project.".into(),
                streaming: false
            }],
            "the retry's answer must appear exactly once, not concatenated with the failed attempt's partial"
        );
    }

    /// pi reports prompt failures through the normal stream, not as a
    /// response error — an assistant error message is the GUI's failure path.
    #[test]
    fn statuses_are_key_sorted_so_the_anchor_leads() {
        let mut s = Session::new();
        s.set_status("status", Some("Turn 3 running…".to_owned()));
        s.set_status("0-reactor", Some("12 tools · 9 active".to_owned()));
        s.set_status("identity", Some("reverse-engineer".to_owned()));

        let keys: Vec<&str> = s.statuses.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["0-reactor", "identity", "status"]);

        // Clearing: pi's setStatus contract — `statusText: undefined` clears.
        s.set_status("identity", None);
        let keys: Vec<&str> = s.statuses.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["0-reactor", "status"]);
    }

    #[test]
    fn thinking_blocks_arrive_collapsed() {
        let mut s = Session::new();
        s.ingest_event(&reactor_rpc::Event::from_value(json!({
            "type": "message_update",
            "assistantMessageEvent": {"type": "thinking_delta", "contentIndex": 0, "delta": "User is…"}
        })));
        s.ingest_event(&reactor_rpc::Event::from_value(json!({
            "type": "message_end",
            "message": {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "User is asking about themes."},
                {"type": "text", "text": "Answer."}
            ]}
        })));
        // `message_end` is authoritative: the final thinking replaces the
        // streaming partial, and the final text lands as its own block.
        assert_eq!(
            s.items,
            vec![
                ChatItem::Thinking {
                    text: "User is asking about themes.".into(),
                    streaming: false
                },
                ChatItem::AssistantText {
                    text: "Answer.".into(),
                    streaming: false
                }
            ]
        );
    }

    /// A custom session entry (`reactor-detail` et al.) renders generically
    /// from its data — the GUI knows the reactor types by name (gui/SPEC.md §6).
    #[test]
    fn custom_entries_render_as_their_type() {
        let mut s = Session::new();
        s.ingest_event(&reactor_rpc::Event::from_value(json!({
            "type": "message_end",
            "message": {"role": "custom", "customType": "reactor-detail",
                        "content": [], "details": {"title": "skill: bn", "body": "# bn"}}
        })));
        // A custom message an extension sent via `pi.sendMessage` renders as
        // its type — `raw` is the message, not the event envelope.
        assert_eq!(
            s.items,
            vec![ChatItem::CustomEntry {
                custom_type: "reactor-detail".into(),
                raw: json!({"role": "custom", "customType": "reactor-detail",
                            "content": [], "details": {"title": "skill: bn", "body": "# bn"}})
            }]
        );
    }

    #[test]
    fn unknown_events_reach_the_debug_view() {
        let mut s = Session::new();
        s.ingest_event(&reactor_rpc::Event::from_value(json!({
            "type": "session_tree_v2", "payload": {"x": 1}
        })));
        match &s.items[0] {
            ChatItem::Unknown { raw } => assert_eq!(raw["payload"]["x"], 1),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    /// The extension-UI contract of ADR-0032 rides setWidget's string lines —
    /// the view model learns about widgets and statuses from the same
    /// `extension_ui_request` the dialogs arrive on.
    #[test]
    fn set_widget_and_set_status_flow_through_the_ui_request() {
        let mut s = Session::new();
        let request = reactor_rpc::ExtensionUiRequest::from_value(json!({
            "type": "extension_ui_request", "id": "u1", "method": "setWidget",
            "widgetKey": "plain-panel",
            "widgetLines": ["services panel lines"],
            "widgetPlacement": "aboveEditor"
        }))
        .unwrap();
        s.ingest(&reactor_rpc::Incoming::ExtensionUiRequest(request));
        assert_eq!(
            s.widgets,
            vec![(
                "plain-panel".to_owned(),
                vec!["services panel lines".to_owned()]
            )]
        );
    }

    #[test]
    fn queue_updates_drive_the_composer_mode() {
        let mut s = Session::new();
        s.ingest_event(&reactor_rpc::Event::from_value(json!({
            "type": "queue_update",
            "steering": ["Change direction"],
            "followUp": ["After that, summarize"]
        })));
        assert_eq!(s.steering, vec!["Change direction".to_owned()]);
        assert_eq!(s.follow_up, vec!["After that, summarize".to_owned()]);
    }
}

#[cfg(test)]
mod replay_capture {
    use super::*;

    /// Replay a recorded pi stream through the real assembly and print the
    /// transcript it produces.
    ///
    /// Not an assertion — a bench for reading. The duplicated-answer bug was
    /// only findable by recording an actual stream and seeing which blocks
    /// came out, so the tool that found it stays. Record one with:
    ///
    /// ```text
    /// mkfifo /tmp/f; (pi --mode rpc --no-session < /tmp/f > /tmp/cap.jsonl &)
    /// exec 4>/tmp/f; echo '{"type":"prompt","message":"hi"}' >&4; sleep 30; exec 4>&-
    /// PI_REPLAY=/tmp/cap.jsonl cargo test -p reactor-gui replay -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs PI_REPLAY=<recorded jsonl>"]
    fn replay() {
        let path = std::env::var("PI_REPLAY").expect("set PI_REPLAY to a recorded stream");
        let mut session = Session::new();
        for line in std::fs::read_to_string(&path).unwrap().lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(line).unwrap();
            // The same classification the reader thread applies: responses
            // are answers to commands, not transcript content.
            if value.get("type").and_then(Value::as_str) == Some("response") {
                continue;
            }
            let incoming = match reactor_rpc::ExtensionUiRequest::from_value(value.clone()) {
                Some(request) => reactor_rpc::Incoming::ExtensionUiRequest(request),
                None => reactor_rpc::Incoming::Event(reactor_rpc::Event::from_value(value)),
            };
            session.ingest(&incoming);
        }
        for (at, item) in session.items.iter().enumerate() {
            match item {
                ChatItem::Thinking { text, streaming } => {
                    println!("[{at}] thinking (streaming={streaming}) {text:?}")
                }
                ChatItem::AssistantText { text, streaming } => {
                    println!("[{at}] text (streaming={streaming}) {text:?}")
                }
                other => println!("[{at}] {other:?}"),
            }
        }
    }
}
