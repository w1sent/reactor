//! serde views of pi's RPC protocol: responses, events, and the extension-UI
//! sub-protocol.
//!
//! Tolerant by design ([`gui/SPEC.md`](../../../gui/SPEC.md) §2): unknown
//! fields are ignored everywhere, and every `from_value` falls back to a raw
//! variant rather than failing — an event pi grew in a later release renders
//! as its JSON in the debug view instead of crashing the client. Facts and
//! evidence live in `docs/pi-api-notes.md`, "Facts for reactor-gui".

use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// `{type:"response", command, success, id?, data?|error?}`.
#[derive(Debug, Clone)]
pub struct Response {
    pub id: Option<String>,
    pub command: String,
    pub success: bool,
    pub data: Option<Value>,
    pub error: Option<String>,
}

impl Response {
    pub fn from_value(v: &Value) -> Option<Self> {
        if v.get("type")?.as_str()? != "response" {
            return None;
        }
        Some(Self {
            id: v.get("id").and_then(Value::as_str).map(str::to_owned),
            command: v.get("command")?.as_str()?.to_owned(),
            success: v.get("success")?.as_bool()?,
            data: v.get("data").cloned().filter(Value::is_object),
            error: v.get("error").and_then(Value::as_str).map(str::to_owned),
        })
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// One streamed event from the pi child, typed where the GUI consumes it and
/// raw where it doesn't — an unknown `type` arrives as [`Event::Unknown`]
/// and is rendered as JSON, never dropped and never fatal.
#[derive(Debug, Clone)]
pub enum Event {
    AgentStart,
    /// One low-level agent run; `will_retry` says an automatic retry follows.
    AgentEnd {
        messages: Vec<Value>,
        will_retry: bool,
    },
    /// Nothing is left pending: no retry, no compaction retry, no queue.
    AgentSettled,
    TurnStart,
    TurnEnd {
        message: Value,
        tool_results: Vec<Value>,
    },
    MessageStart {
        message: Value,
    },
    MessageUpdate {
        usage: Option<Value>,
        delta: AssistantMessageEvent,
    },
    MessageEnd {
        message: Value,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: Value,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        partial_result: Value,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: Value,
        is_error: bool,
    },
    QueueUpdate {
        steering: Vec<String>,
        follow_up: Vec<String>,
    },
    CompactionStart {
        reason: Option<String>,
    },
    CompactionEnd {
        reason: Option<String>,
        result: Option<Value>,
        aborted: bool,
        error_message: Option<String>,
    },
    AutoRetryStart {
        attempt: Option<u64>,
        error_message: Option<String>,
    },
    AutoRetryEnd {
        success: bool,
        final_error: Option<String>,
    },
    /// Direct output of an RPC `bash` command; `id` correlates it when the
    /// command carried one.
    BashExecutionUpdate {
        id: Option<String>,
        delta: String,
    },
    ExtensionError {
        extension_path: Option<String>,
        event: Option<String>,
        error: Option<String>,
    },
    /// First line of the stream — the session header
    /// (`{"type":"session","version":…,"id":…,"cwd":…}`).
    SessionHeader {
        raw: Value,
    },
    /// pi edited its session log (e.g. erasing a failed auto-retry
    /// attempt's entry). No client-visible effect: the GUI does not track
    /// entries by id at the transcript level, and the entry this refers to
    /// was never rendered from anyway (gui/SPEC.md §2's live-stream path
    /// only reads `message_end`/deltas, never raw entries).
    EntryAppended,
    /// Any event type this client does not know. Rendered raw; a drift of
    /// pi's protocol surfaces here instead of as a crash.
    Unknown {
        raw: Value,
    },
}

impl Event {
    pub fn from_value(v: Value) -> Event {
        let kind = v.get("type").and_then(Value::as_str).unwrap_or_default();
        match kind {
            "agent_start" => Event::AgentStart,
            "agent_end" => Event::AgentEnd {
                messages: v
                    .get("messages")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                will_retry: v.get("willRetry").and_then(Value::as_bool).unwrap_or(false),
            },
            "agent_settled" => Event::AgentSettled,
            // pi's own session-log bookkeeping (verified live: a
            // `context_edit` with `replacement: null` retroactively erases
            // the entry a failed auto-retry attempt left behind). Not
            // protocol drift — a client that renders nothing for it is
            // exactly right, so it is a recognized no-op rather than falling
            // through to `Unknown`'s raw-JSON debug rendering.
            "entry_appended" => Event::EntryAppended,
            "turn_start" => Event::TurnStart,
            "turn_end" => Event::TurnEnd {
                message: v.get("message").cloned().unwrap_or(Value::Null),
                tool_results: v
                    .get("toolResults")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            },
            "message_start" => Event::MessageStart {
                message: v.get("message").cloned().unwrap_or(Value::Null),
            },
            "message_update" => Event::MessageUpdate {
                usage: v.get("usage").cloned().filter(|u| !u.is_null()),
                delta: v
                    .get("assistantMessageEvent")
                    .cloned()
                    .map(AssistantMessageEvent::from_value)
                    .unwrap_or(AssistantMessageEvent::Unknown),
            },
            "message_end" => Event::MessageEnd {
                message: v.get("message").cloned().unwrap_or(Value::Null),
            },
            "tool_execution_start" => Event::ToolExecutionStart {
                tool_call_id: v
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                tool_name: v
                    .get("toolName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                args: v.get("args").cloned().unwrap_or(Value::Null),
            },
            "tool_execution_update" => Event::ToolExecutionUpdate {
                tool_call_id: v
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                tool_name: v
                    .get("toolName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                partial_result: v.get("partialResult").cloned().unwrap_or(Value::Null),
            },
            "tool_execution_end" => Event::ToolExecutionEnd {
                tool_call_id: v
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                tool_name: v
                    .get("toolName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                result: v.get("result").cloned().unwrap_or(Value::Null),
                is_error: v.get("isError").and_then(Value::as_bool).unwrap_or(false),
            },
            "queue_update" => Event::QueueUpdate {
                steering: strings(v.get("steering")),
                follow_up: strings(v.get("followUp")),
            },
            "compaction_start" => Event::CompactionStart {
                reason: v.get("reason").and_then(Value::as_str).map(str::to_owned),
            },
            "compaction_end" => Event::CompactionEnd {
                reason: v.get("reason").and_then(Value::as_str).map(str::to_owned),
                result: v.get("result").cloned(),
                aborted: v.get("aborted").and_then(Value::as_bool).unwrap_or(false),
                error_message: v
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
            "auto_retry_start" => Event::AutoRetryStart {
                attempt: v.get("attempt").and_then(Value::as_u64),
                error_message: v
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
            "auto_retry_end" => Event::AutoRetryEnd {
                success: v.get("success").and_then(Value::as_bool).unwrap_or(false),
                final_error: v
                    .get("finalError")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
            "bash_execution_update" => Event::BashExecutionUpdate {
                id: v.get("id").and_then(Value::as_str).map(str::to_owned),
                delta: v
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            },
            "extension_error" => Event::ExtensionError {
                extension_path: v
                    .get("extensionPath")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                event: v.get("event").and_then(Value::as_str).map(str::to_owned),
                error: v.get("error").and_then(Value::as_str).map(str::to_owned),
            },
            "session" => Event::SessionHeader { raw: v },
            _ => Event::Unknown { raw: v },
        }
    }
}

/// One streaming delta of the assistant message, keyed by `contentIndex`
/// (the index into the message's content array). `message_end.message` is
/// the authoritative record; these build the live partial.
#[derive(Debug, Clone, PartialEq)]
pub enum AssistantMessageEvent {
    TextStart {
        content_index: usize,
    },
    TextDelta {
        content_index: usize,
        delta: String,
    },
    TextEnd {
        content_index: usize,
        text: Option<String>,
    },
    ThinkingStart {
        content_index: usize,
    },
    ThinkingDelta {
        content_index: usize,
        delta: String,
    },
    ThinkingEnd {
        content_index: usize,
        thinking: Option<String>,
    },
    ToolcallStart {
        content_index: usize,
        id: String,
        tool_name: String,
    },
    ToolcallDelta {
        content_index: usize,
        delta: String,
    },
    ToolcallEnd {
        content_index: usize,
        tool_call: Value,
    },
    Unknown,
}

impl AssistantMessageEvent {
    pub fn from_value(v: Value) -> Self {
        let idx = || {
            v.get("contentIndex")
                .and_then(Value::as_u64)
                .unwrap_or_default() as usize
        };
        match v.get("type").and_then(Value::as_str) {
            Some("text_start") => Self::TextStart {
                content_index: idx(),
            },
            Some("text_delta") => Self::TextDelta {
                content_index: idx(),
                delta: v
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            },
            Some("text_end") => Self::TextEnd {
                content_index: idx(),
                text: v.get("content").and_then(Value::as_str).map(str::to_owned),
            },
            Some("thinking_start") => Self::ThinkingStart {
                content_index: idx(),
            },
            Some("thinking_delta") => Self::ThinkingDelta {
                content_index: idx(),
                delta: v
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            },
            Some("thinking_end") => Self::ThinkingEnd {
                content_index: idx(),
                // pi names it `content`, like `text_end` (verified live
                // against 0.87.0). Reading `thinking` here silently yielded
                // `None` on every turn, so the thinking block was never
                // finalized — `message_end` then pushed a second one.
                thinking: v
                    .get("content")
                    .or_else(|| v.get("thinking"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
            Some("toolcall_start") => Self::ToolcallStart {
                content_index: idx(),
                id: v.get("id").and_then(Value::as_str).unwrap_or("").to_owned(),
                tool_name: v
                    .get("toolName")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            },
            Some("toolcall_delta") => Self::ToolcallDelta {
                content_index: idx(),
                delta: v
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            },
            Some("toolcall_end") => Self::ToolcallEnd {
                content_index: idx(),
                tool_call: v.get("toolCall").cloned().unwrap_or(Value::Null),
            },
            _ => Self::Unknown,
        }
    }
}

// ---------------------------------------------------------------------------
// Extension UI sub-protocol
// ---------------------------------------------------------------------------

/// Which blocking behaviour the request carries — dialogs need an answer on
/// stdin, fire-and-forget requests do not (see gui/SPEC.md §4.5).
#[derive(Debug, Clone, PartialEq)]
pub enum UiMethod {
    Select {
        title: String,
        options: Vec<String>,
        timeout_ms: Option<u64>,
    },
    Confirm {
        title: String,
        message: Option<String>,
        timeout_ms: Option<u64>,
    },
    Input {
        title: String,
        placeholder: Option<String>,
    },
    Editor {
        title: String,
        prefill: Option<String>,
    },
    Notify {
        message: String,
        notify_type: String,
    },
    SetStatus {
        status_key: String,
        status_text: Option<String>,
    },
    /// Text lines only over RPC — component factories never leave the TUI.
    SetWidget {
        widget_key: String,
        widget_lines: Option<Vec<String>>,
        widget_placement: Option<String>,
    },
    SetTitle {
        title: String,
    },
    SetEditorText {
        text: String,
    },
    Unknown {
        method: String,
    },
}

/// `{type:"extension_ui_request", id, method, …}`.
#[derive(Debug, Clone)]
pub struct ExtensionUiRequest {
    pub id: String,
    pub method: UiMethod,
    /// The whole request, for the raw/debug view.
    pub raw: Value,
}

impl ExtensionUiRequest {
    pub fn from_value(v: Value) -> Option<Self> {
        if v.get("type")?.as_str()? != "extension_ui_request" {
            return None;
        }
        let id = v.get("id")?.as_str()?.to_owned();
        let method = match v.get("method").and_then(Value::as_str)? {
            "select" => UiMethod::Select {
                title: str_field(&v, "title"),
                options: v
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                timeout_ms: v.get("timeout").and_then(Value::as_u64),
            },
            "confirm" => UiMethod::Confirm {
                title: str_field(&v, "title"),
                message: v.get("message").and_then(Value::as_str).map(str::to_owned),
                timeout_ms: v.get("timeout").and_then(Value::as_u64),
            },
            "input" => UiMethod::Input {
                title: str_field(&v, "title"),
                placeholder: v
                    .get("placeholder")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
            "editor" => UiMethod::Editor {
                title: str_field(&v, "title"),
                prefill: v.get("prefill").and_then(Value::as_str).map(str::to_owned),
            },
            "notify" => UiMethod::Notify {
                message: str_field(&v, "message"),
                notify_type: v
                    .get("notifyType")
                    .and_then(Value::as_str)
                    .unwrap_or("info")
                    .to_owned(),
            },
            "setStatus" => UiMethod::SetStatus {
                status_key: str_field(&v, "statusKey"),
                status_text: v
                    .get("statusText")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
            "setWidget" => UiMethod::SetWidget {
                widget_key: str_field(&v, "widgetKey"),
                widget_lines: v.get("widgetLines").and_then(Value::as_array).map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                }),
                widget_placement: v
                    .get("widgetPlacement")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
            "setTitle" => UiMethod::SetTitle {
                title: str_field(&v, "title"),
            },
            "set_editor_text" => UiMethod::SetEditorText {
                text: v
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            },
            other => UiMethod::Unknown {
                method: other.to_owned(),
            },
        };
        Some(Self { id, method, raw: v })
    }
}

/// The stdin answer to a dialog request. `Cancel` works for every dialog
/// method; pi resolves it to `undefined`/`false` on the extension side.
#[derive(Debug, Clone, PartialEq)]
pub enum UiResponse {
    /// `select`/`input`/`editor` — the chosen or edited value.
    Value(String),
    /// `confirm`.
    Confirmed(bool),
    Cancelled,
}

impl UiResponse {
    pub fn to_value(&self, id: &str) -> Value {
        let mut o = json!({ "type": "extension_ui_response", "id": id });
        match self {
            UiResponse::Value(v) => {
                o["value"] = json!(v);
            }
            UiResponse::Confirmed(b) => {
                o["confirmed"] = json!(b);
            }
            UiResponse::Cancelled => {
                o["cancelled"] = json!(true);
            }
        }
        o
    }
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
