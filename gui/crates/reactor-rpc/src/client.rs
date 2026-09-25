//! The RPC client: owns the `pi --mode rpc` child, one reader thread, and a
//! request-correlation map.
//!
//! Everything a line can be — a response, an agent event, an
//! `extension_ui_request`, the session header — arrives as an
//! [`Incoming`] on the channel [`RpcClient::take_receiver`] hands out.
//! Requests correlate by `id`; dialog requests (`select`, `confirm`,
//! `input`, `editor`) are answered through [`RpcClient::respond_ui`].
//!
//! No async runtime: one reader thread and a mutex-guarded writer are
//! exactly enough for one child and a linear protocol, and they keep the
//! GUI crate free of an executor war with gpui's own (gui/SPEC.md §2).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::framing::{LineSplitter, write_line};
pub use crate::protocol::{Event, ExtensionUiRequest, Response, UiMethod, UiResponse};

/// One thing the child said, on its way from the reader thread to the GUI.
#[derive(Debug, Clone)]
pub enum Incoming {
    /// A reply to a command this client sent.
    Response(Response),
    /// A streamed agent/compaction/tool event (never a response).
    Event(Event),
    /// An extension asking for UI, via pi's extension-UI sub-protocol.
    ExtensionUiRequest(ExtensionUiRequest),
    /// The line was not valid JSON — malformed input from the child. Shown
    /// raw in the debug view; never fatal.
    Malformed { line: String, reason: String },
    /// The child closed stdout. The reader thread ends; nothing follows.
    Eof,
}

/// How the client launches pi and what it passes through — the launch flags
/// of gui/SPEC.md §3 map onto this one-to-one. reactor-gui always adds
/// `REACTOR_GUI=1` (the handshake of ADR-0032).
#[derive(Debug, Clone, Default)]
pub struct SpawnConfig {
    /// Default: `pi`, resolved from `PATH`.
    pub program: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<std::path::PathBuf>,
    pub env: Vec<(String, String)>,
}

/// A typed constructor per RPC command the GUI uses (gui/SPEC.md §6). The
/// full wire surface is pi's documented protocol; anything untyped here
/// still goes through [`Command::Raw`].
#[derive(Debug, Clone)]
pub enum Command {
    Prompt {
        message: String,
        images: Vec<Value>,
        /// Required while the agent streams: `"steer"` or `"followUp"`.
        streaming_behavior: Option<String>,
    },
    Steer(String),
    FollowUp(String),
    Abort,
    ClearQueue,
    GetState,
    GetMessages,
    GetCommands,
    GetTree,
    GetEntries {
        since: Option<String>,
    },
    GetSessionStats,
    GetAvailableModels,
    SetModel {
        provider: String,
        model_id: String,
    },
    GetAvailableThinkingLevels,
    SetThinkingLevel(String),
    SwitchSession(String),
    NewSession,
    SetSessionName(String),
    Compact {
        custom_instructions: Option<String>,
    },
    Bash {
        command: String,
    },
    AbortBash,
    /// Any command not typed here — the GUI's raw/debug view keeps unknown
    /// pi commands reachable.
    Raw(Value),
}

impl Command {
    pub fn to_value(&self) -> Value {
        match self {
            Command::Prompt {
                message,
                images,
                streaming_behavior,
            } => {
                let mut o = json!({ "type": "prompt", "message": message });
                if !images.is_empty() {
                    o["images"] = json!(images);
                }
                if let Some(sb) = streaming_behavior {
                    o["streamingBehavior"] = json!(sb);
                }
                o
            }
            Command::Steer(m) => json!({ "type": "steer", "message": m }),
            Command::FollowUp(m) => json!({ "type": "follow_up", "message": m }),
            Command::Abort => json!({ "type": "abort" }),
            Command::ClearQueue => json!({ "type": "clear_queue" }),
            Command::GetState => json!({ "type": "get_state" }),
            Command::GetMessages => json!({ "type": "get_messages" }),
            Command::GetCommands => json!({ "type": "get_commands" }),
            Command::GetTree => json!({ "type": "get_tree" }),
            Command::GetEntries { since } => match since {
                Some(s) => json!({ "type": "get_entries", "since": s }),
                None => json!({ "type": "get_entries" }),
            },
            Command::GetSessionStats => json!({ "type": "get_session_stats" }),
            Command::GetAvailableModels => json!({ "type": "get_available_models" }),
            Command::SetModel { provider, model_id } => {
                json!({ "type": "set_model", "provider": provider, "modelId": model_id })
            }
            Command::GetAvailableThinkingLevels => {
                json!({ "type": "get_available_thinking_levels" })
            }
            Command::SetThinkingLevel(level) => {
                json!({ "type": "set_thinking_level", "level": level })
            }
            Command::SwitchSession(path) => {
                json!({ "type": "switch_session", "sessionPath": path })
            }
            Command::NewSession => json!({ "type": "new_session" }),
            Command::SetSessionName(name) => json!({ "type": "set_session_name", "name": name }),
            Command::Compact {
                custom_instructions,
            } => match custom_instructions {
                Some(ci) => json!({ "type": "compact", "customInstructions": ci }),
                None => json!({ "type": "compact" }),
            },
            Command::Bash { command } => json!({ "type": "bash", "command": command }),
            Command::AbortBash => json!({ "type": "abort_bash" }),
            Command::Raw(v) => v.clone(),
        }
    }
}

/// What can go wrong waiting for a correlated response.
#[derive(Debug, Clone)]
pub enum RequestError {
    /// The child died before answering.
    Disconnected,
    /// The timed-out wait gave up; the response may still arrive on the
    /// event channel (routed as a late [`Incoming::Response`]).
    Timeout,
    /// The write itself failed.
    Send(String),
}

/// The live connection to one pi child. `Clone` shares one connection —
/// the GUI sends from the composer while the reader thread routes.
#[derive(Clone)]
pub struct RpcClient {
    inner: Arc<Inner>,
    receiver: Arc<Mutex<Option<mpsc::Receiver<Incoming>>>>,
    child: Arc<Mutex<Option<Child>>>,
}

struct Inner {
    stdin: Mutex<Box<dyn Write + Send>>,
    pending: Arc<Mutex<HashMap<String, mpsc::Sender<Response>>>>,
    next_id: AtomicU64,
}

impl RpcClient {
    /// Spawn `pi --mode rpc …` and connect.
    ///
    /// stderr stays piped but is drained on its own thread into a sink: a
    /// full stderr pipe would block the child mid-protocol.
    pub fn spawn(config: &SpawnConfig) -> std::io::Result<Self> {
        let mut cmd = std::process::Command::new(config.program.as_deref().unwrap_or("pi"));
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("stdout not piped"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("stdin not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("stderr not piped"))?;
        std::thread::spawn(move || {
            // Drained, not displayed: pi's stderr carries crash traces only.
            let mut sink = std::io::sink();
            let _ = std::io::copy(&mut BufReader::new(stderr), &mut sink);
        });
        Ok(Self::connect(
            BufReader::new(stdout),
            Box::new(stdin),
            Some(child),
        ))
    }

    /// Connect to an already-running transport — the spawned child, or a
    /// test peer. The reader thread owns `read`; `child` (None in tests)
    /// is parked for [`RpcClient::shutdown`].
    pub fn connect(
        mut read: impl BufRead + Send + 'static,
        write: Box<dyn Write + Send>,
        child: Option<Child>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let pending: Arc<Mutex<HashMap<String, mpsc::Sender<Response>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let reader_pending = Arc::clone(&pending);
        std::thread::Builder::new()
            .name("reactor-rpc-reader".into())
            .spawn(move || read_loop(&mut read, reader_pending, tx))
            .expect("reader thread");

        let child_slot = Arc::new(Mutex::new(child));
        if child_slot.lock().unwrap().is_some() {
            // A reaper: pi is reaped when it exits, whether or not the GUI
            // noticed. Polling `try_wait` because the slot is shared with
            // [`RpcClient::shutdown`], which takes the child out to kill it.
            let slot = Arc::clone(&child_slot);
            std::thread::Builder::new()
                .name("reactor-rpc-reaper".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(Duration::from_millis(200));
                        let mut guard = slot.lock().unwrap();
                        match guard.as_mut() {
                            Some(c) => {
                                if matches!(c.try_wait(), Ok(Some(_)) | Err(_)) {
                                    guard.take();
                                    break;
                                }
                            }
                            None => break, // shutdown() took it
                        }
                    }
                })
                .expect("reaper thread");
        }

        Self {
            inner: Arc::new(Inner {
                stdin: Mutex::new(write),
                pending,
                next_id: AtomicU64::new(1),
            }),
            receiver: Arc::new(Mutex::new(Some(rx))),
            child: child_slot,
        }
    }

    /// The single event stream. Take it once, at startup; the receiver is
    /// `mpsc`, which is why this is a once-per-client operation.
    pub fn take_receiver(&self) -> Option<mpsc::Receiver<Incoming>> {
        self.receiver.lock().unwrap().take()
    }

    /// Write one command line, without waiting for its response. `Ok(())`
    /// means the record left the process — acceptance is pi's business and
    /// arrives as the correlated response.
    pub fn send(&self, mut cmd: Value) -> std::io::Result<()> {
        if let Some(o) = cmd.as_object_mut() {
            if !o.contains_key("id") {
                let id = self.next_id();
                o.insert("id".into(), Value::String(id));
            }
        }
        let mut stdin = self.inner.stdin.lock().unwrap();
        write_line(&mut *stdin, &cmd)
    }

    /// Send with correlation and wait for the response, without a bound.
    /// pi answers every command eventually (`abort` waits for idle; a
    /// rejected prompt answers `success: false`) — bound it yourself with
    /// [`RpcClient::request_timeout`] when a hang is unacceptable.
    pub fn request(&self, cmd: Command) -> Result<Response, RequestError> {
        let (_id, rx) = self.send_request(cmd)?;
        rx.recv().map_err(|_| RequestError::Disconnected)
    }

    /// The bounded form: give up waiting after `timeout`. The response may
    /// still arrive later — it surfaces as an [`Incoming::Response`] on the
    /// event channel, never lost.
    pub fn request_timeout(
        &self,
        cmd: Command,
        timeout: Duration,
    ) -> Result<Response, RequestError> {
        let (_id, rx) = self.send_request(cmd)?;
        rx.recv_timeout(timeout).map_err(|e| match e {
            mpsc::RecvTimeoutError::Timeout => RequestError::Timeout,
            mpsc::RecvTimeoutError::Disconnected => RequestError::Disconnected,
        })
    }

    fn send_request(
        &self,
        cmd: Command,
    ) -> Result<(String, mpsc::Receiver<Response>), RequestError> {
        let mut v = cmd.to_value();
        let id = self.next_id();
        if let Some(o) = v.as_object_mut() {
            o.insert("id".into(), Value::String(id.clone()));
        }

        let (tx, rx) = mpsc::channel();
        self.inner.pending.lock().unwrap().insert(id.clone(), tx);

        if let Err(e) = self.send(v) {
            // The line never left; nobody will answer this id.
            self.inner.pending.lock().unwrap().remove(&id);
            return Err(RequestError::Send(e.to_string()));
        }
        Ok((id, rx))
    }

    /// Answer a dialog request (gui/SPEC.md §4.5). Fire-and-forget methods
    /// (`notify`, `setWidget`, …) take no answer — sending one is harmless
    /// but pointless.
    pub fn respond_ui(&self, request_id: &str, response: UiResponse) -> std::io::Result<()> {
        self.send(response.to_value(request_id))
    }

    /// Kill the child. pi handles SIGTERM with its own cleanup handler and
    /// exits; this is the window-close path. Idempotent.
    pub fn shutdown(&self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }

    fn next_id(&self) -> String {
        format!("req-{}", self.inner.next_id.fetch_add(1, Ordering::Relaxed))
    }
}

/// Route one line: responses to their pending sender, UI requests and events
/// to the channel, everything unparsable to [`Incoming::Malformed`].
///
/// Extracted from the reader thread so the routing rules test without a
/// child process — the thread only contributes bytes. A late response (an id
/// nobody waits for) still surfaces as [`Incoming::Response`]: a GUI that
/// timed out a request sees the answer when it comes.
fn route(
    line: &str,
    pending: &mut HashMap<String, mpsc::Sender<Response>>,
    tx: &mpsc::Sender<Incoming>,
) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    let value = match serde_json::from_str::<Value>(trimmed) {
        Ok(v) => v,
        Err(e) => {
            let _ = tx.send(Incoming::Malformed {
                line: trimmed.to_owned(),
                reason: e.to_string(),
            });
            return;
        }
    };

    if value.get("type").and_then(Value::as_str) == Some("response") {
        if let Some(response) = Response::from_value(&value) {
            let waiter = response.id.as_ref().and_then(|id| pending.remove(id));
            if let Some(waiter_tx) = waiter {
                let _ = waiter_tx.send(response);
                return;
            }
            let _ = tx.send(Incoming::Response(response));
            return;
        }
    }

    if let Some(request) = ExtensionUiRequest::from_value(value.clone()) {
        let _ = tx.send(Incoming::ExtensionUiRequest(request));
        return;
    }

    let _ = tx.send(Incoming::Event(Event::from_value(value)));
}

/// The reader thread: bytes in, routed [`Incoming`] out.
fn read_loop(
    reader: &mut impl BufRead,
    pending: Arc<Mutex<HashMap<String, mpsc::Sender<Response>>>>,
    tx: mpsc::Sender<Incoming>,
) {
    let mut splitter = LineSplitter::new();
    loop {
        // Fill → push → consume: the chunk is borrowed from the reader, so
        // its length is taken before the lines it completed are routed.
        let chunk_len = match reader.fill_buf() {
            Ok(chunk) if !chunk.is_empty() => {
                let lines = splitter.push(chunk);
                let len = chunk.len();
                for line in lines {
                    route(&line, &mut pending.lock().unwrap(), &tx);
                }
                len
            }
            _ => break, // EOF or error: same outcome, the child went away
        };
        reader.consume(chunk_len);
    }
    if let Some(line) = splitter.finish() {
        route(&line, &mut pending.lock().unwrap(), &tx);
    }
    let _ = tx.send(Incoming::Eof);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AssistantMessageEvent;

    /// A router for one test: a pending map plus a receiver for everything
    /// routed past correlation.
    struct Fixture {
        pending: HashMap<String, mpsc::Sender<Response>>,
        tx: mpsc::Sender<Incoming>,
        rx: mpsc::Receiver<Incoming>,
    }

    impl Fixture {
        fn new() -> Self {
            let (tx, rx) = mpsc::channel();
            Self {
                pending: HashMap::new(),
                tx,
                rx,
            }
        }

        fn route(&mut self, line: &str) {
            route(line, &mut self.pending, &self.tx);
        }

        fn drain(&self) -> Vec<Incoming> {
            let mut out = Vec::new();
            while let Ok(v) = self.rx.try_recv() {
                out.push(v);
            }
            out
        }
    }

    #[test]
    fn response_with_a_waiter_never_reaches_the_channel() {
        let mut f = Fixture::new();
        let (tx, rx) = mpsc::channel();
        f.pending.insert("req-1".into(), tx);

        f.route(
            r#"{"id":"req-1","type":"response","command":"get_state","success":true,"data":{"isStreaming":false}}"#,
        );

        // Correlated: the requester got it, the channel is silent.
        assert!(f.drain().is_empty());
        let got = rx.recv().unwrap();
        assert_eq!(got.command, "get_state");
        assert!(got.success);
        assert!(f.pending.is_empty(), "answered ids are removed");
    }

    #[test]
    fn late_response_surfaces_on_the_channel() {
        let mut f = Fixture::new();
        f.route(r#"{"id":"req-9","type":"response","command":"prompt","success":true}"#);
        let first = f.drain().remove(0);
        match first {
            Incoming::Response(ref r) => {
                assert_eq!(r.id.as_deref(), Some("req-9"));
                assert!(r.success);
            }
            other => panic!("expected a late response, got {other:?}"),
        }
    }

    /// pi reports prompt failures through the normal stream, not a second
    /// response for the same id — this response IS the GUI's failure path.
    #[test]
    fn failure_response_parses_with_error() {
        let mut f = Fixture::new();
        f.route(
            r#"{"id":"req-2","type":"response","command":"set_model","success":false,"error":"Model not found: x"}"#,
        );
        let first = f.drain().remove(0);
        match first {
            Incoming::Response(ref r) => {
                assert!(!r.success);
                assert_eq!(r.error.as_deref(), Some("Model not found: x"));
            }
            other => panic!("expected a failure response, got {other:?}"),
        }
    }

    #[test]
    fn extension_ui_request_is_typed() {
        let mut f = Fixture::new();
        f.route(
            r#"{"type":"extension_ui_request","id":"u1","method":"select","title":"Allow?","options":["Allow","Block"],"timeout":10000}"#,
        );
        let first = f.drain().remove(0);
        match first {
            Incoming::ExtensionUiRequest(ref req) => {
                assert_eq!(req.id, "u1");
                match &req.method {
                    UiMethod::Select {
                        title,
                        options,
                        timeout_ms,
                    } => {
                        assert_eq!(title, "Allow?");
                        assert_eq!(options, &vec!["Allow".to_owned(), "Block".to_owned()]);
                        assert_eq!(timeout_ms.as_ref(), Some(&10_000u64));
                    }
                    other => panic!("expected select, got {other:?}"),
                }
            }
            other => panic!("expected a UI request, got {other:?}"),
        }
    }

    /// The extension-UI contract of ADR-0032 rides setWidget's string lines,
    /// so the GUI's envelope detection depends on this shape surviving.
    #[test]
    fn set_widget_carries_lines_and_key() {
        let mut f = Fixture::new();
        f.route(
            r#"{"type":"extension_ui_request","id":"u2","method":"setWidget","widgetKey":"reactor:selector","widgetLines":["REACTOR-GUI-VIEW v1 {}","9 active"],"widgetPlacement":"aboveEditor"}"#,
        );
        let first = f.drain().remove(0);
        match first {
            Incoming::ExtensionUiRequest(ref req) => match &req.method {
                UiMethod::SetWidget {
                    widget_key,
                    widget_lines,
                    widget_placement,
                } => {
                    assert_eq!(widget_key, "reactor:selector");
                    assert_eq!(widget_lines.as_ref().unwrap().len(), 2);
                    assert_eq!(widget_placement.as_deref(), Some("aboveEditor"));
                }
                other => panic!("expected setWidget, got {other:?}"),
            },
            other => panic!("expected a UI request, got {other:?}"),
        }
    }

    #[test]
    fn unknown_ui_method_still_arrives() {
        let mut f = Fixture::new();
        f.route(r#"{"type":"extension_ui_request","id":"u3","method":"setView","widgetKey":"x"}"#);
        let first = f.drain().remove(0);
        match first {
            Incoming::ExtensionUiRequest(ref req) => {
                assert_eq!(req.id, "u3");
                assert_eq!(
                    req.method,
                    UiMethod::Unknown {
                        method: "setView".into()
                    }
                );
            }
            other => panic!("expected a UI request, got {other:?}"),
        }
    }

    #[test]
    fn text_delta_event_keeps_its_index() {
        let mut f = Fixture::new();
        f.route(
            r#"{"type":"message_update","usage":{"totalTokens":101},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Hello "}}"#,
        );
        let first = f.drain().remove(0);
        match first {
            Incoming::Event(Event::MessageUpdate { usage, delta }) => {
                assert_eq!(usage.unwrap()["totalTokens"], 101);
                assert_eq!(
                    delta,
                    AssistantMessageEvent::TextDelta {
                        content_index: 0,
                        delta: "Hello ".to_owned()
                    }
                );
            }
            other => panic!("expected message_update, got {other:?}"),
        }
    }

    /// Protocol drift is a documented risk (gui/SPEC.md §2): an event type
    /// pi grew in a later release arrives raw and renders, never crashes.
    #[test]
    fn unknown_event_type_falls_back_to_raw() {
        let mut f = Fixture::new();
        f.route(r#"{"type":"widget_v2","payload":{"x":1}}"#);
        let first = f.drain().remove(0);
        match first {
            Incoming::Event(Event::Unknown { raw }) => {
                assert_eq!(raw["payload"]["x"], 1);
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn malformed_line_is_surfaced_not_fatal() {
        let mut f = Fixture::new();
        f.route("not json at all");
        let first = f.drain().remove(0);
        match first {
            Incoming::Malformed { line, reason } => {
                assert_eq!(line, "not json at all");
                assert!(!reason.is_empty());
            }
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn blank_lines_are_skipped() {
        let mut f = Fixture::new();
        f.route("   ");
        f.route("\r");
        assert!(f.drain().is_empty());
    }

    #[test]
    fn session_header_arrives_raw() {
        let mut f = Fixture::new();
        f.route(r#"{"type":"session","version":3,"id":"abc","cwd":"/tmp"}"#);
        let first = f.drain().remove(0);
        match first {
            Incoming::Event(Event::SessionHeader { raw }) => {
                assert_eq!(raw["version"], 3);
            }
            other => panic!("expected session header, got {other:?}"),
        }
    }

    /// The stdin answer a dialog gets: the id must match pi's request.
    #[test]
    fn ui_responses_correlate_by_id() {
        assert_eq!(
            UiResponse::Value("Allow".into()).to_value("u1"),
            json!({"type": "extension_ui_response", "id": "u1", "value": "Allow"})
        );
        assert_eq!(
            UiResponse::Confirmed(false).to_value("u2"),
            json!({"type": "extension_ui_response", "id": "u2", "confirmed": false})
        );
        assert_eq!(
            UiResponse::Cancelled.to_value("u3"),
            json!({"type": "extension_ui_response", "id": "u3", "cancelled": true})
        );
    }
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestError::Disconnected => write!(f, "rpc disconnected"),
            RequestError::Timeout => write!(f, "rpc request timed out"),
            RequestError::Send(e) => write!(f, "rpc send failed: {e}"),
        }
    }
}
