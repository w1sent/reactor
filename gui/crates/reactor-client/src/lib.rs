//! `ReactorClient` — the seam between the GUI and the `reactor` CLI.
//!
//! The GUI never parses `tools.toml` and never re-implements catalogue
//! semantics ([ADR-0005](../../docs/adr/0005-reactor-cli-stdlib-python.md)):
//! it asks the CLI — `--format json` where a payload is rendered on screen,
//! plain text where it is prose (the registry block, detail pages). Today
//! the answer comes from shelling out: the CLI owns the probe TTLs and
//! `cache.json`, and the GUI adds no second probe policy
//! ([ADR-0014](../../docs/adr/0014-extensions-share-the-cache-not-each-other.md)).
//! A future Rust port of the CLI lands as a sibling repo and drops in
//! behind the same trait as a crate (gui/SPEC.md §5) — that is the whole
//! point of the seam.
//!
//! Every payload struct tolerates unknown fields: the CLI's JSON contract
//! may grow, and a GUI that hard-fails on a new field is a GUI that breaks
//! on every CLI update.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

/// How long one CLI invocation may run. pi's own exec gives the same call
/// 20s (`extensions/status/`); a wedged CLI costs one slow refresh, not a
/// hung GUI.
pub const EXEC_TIMEOUT: Duration = Duration::from_secs(20);

pub type Result<T> = std::result::Result<T, Error>;

// ---------------------------------------------------------------------------
// Payloads — the CLI's `--format json` contracts, tolerant of new fields
// ---------------------------------------------------------------------------

/// `reactor tools list --format json`.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolsPayload {
    pub schema: u32,
    #[serde(default)]
    pub tools: Vec<ToolRow>,
}

/// One catalogue entry, as the CLI renders it.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolRow {
    pub id: String,
    pub name: String,
    pub desc: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub invoke: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// `present` | `absent` | `unknown` — the probe's answer.
    pub status: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    pub active: bool,
    /// `override` is a Rust keyword — the wire name is pinned with a rename.
    #[serde(rename = "override", default)]
    pub override_kind: Option<String>,
    /// Declaration or live state, per the CLI's contract — passed through
    /// untouched, because the services panel renders it from
    /// [`ReactorClient::services`], not from here.
    #[serde(default)]
    pub service: Value,
    #[serde(default)]
    pub skill: Option<Value>,
}

/// `reactor toolsets list --format json`.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolsetsPayload {
    pub schema: u32,
    #[serde(default)]
    pub toolsets: Vec<ToolsetRow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolsetRow {
    pub id: String,
    pub desc: String,
    pub active: bool,
    #[serde(default)]
    pub tools: Vec<String>,
}

/// `reactor services --format json` — the same shape the `status` extension's
/// footer consumes, so the GUI's panel and the TUI's footer cannot disagree.
#[derive(Debug, Clone, Deserialize)]
pub struct ServicesPayload {
    pub schema: u32,
    #[serde(default)]
    pub services: Vec<ServiceRow>,
    #[serde(default)]
    pub summary: BTreeMap<String, u32>,
}

/// Service state for one tool: `up`/`down`/`unknown` over
/// `present`/`absent`/`unknown` — the two-axis grammar of the footer.
#[derive(Debug, Clone, Deserialize)]
pub struct ServiceRow {
    pub id: String,
    pub name: String,
    pub label: String,
    pub state: String,
    #[serde(default)]
    pub detail: Option<String>,
    pub status: String,
    pub active: bool,
}

/// `reactor state --format json` — activation state plus the resolved
/// `active` list, which is what the agent is currently told about.
#[derive(Debug, Clone, Deserialize)]
pub struct StatePayload {
    pub schema: u32,
    #[serde(default)]
    pub state: Value,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub active: Vec<String>,
}

/// `reactor tools show <id> --format json` — one entry in full, including
/// install recipes.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolDetailPayload {
    pub schema: u32,
    #[serde(default)]
    pub tool: Value,
}

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// The GUI's only source of catalogue facts. Errors are enums, but their
/// Display is what the GUI shows — the CLI's stderr is a human-readable
/// diagnosis (`reactor doctor` is the follow-up), never something to match
/// on.
pub trait ReactorClient {
    fn tools(&self) -> Result<ToolsPayload>;
    fn toolsets(&self) -> Result<ToolsetsPayload>;
    /// `refresh: true` costs a cold probe — the GUI passes it only when the
    /// user asks to refresh, never on its timer (the CLI's TTLs decide).
    fn services(&self, refresh: bool) -> Result<ServicesPayload>;
    fn state(&self) -> Result<StatePayload>;
    /// One catalogue entry in full, including install recipes.
    fn tool_detail(&self, id: &str) -> Result<ToolDetailPayload>;
    /// The registry block as text — the system-prompt injection, rendered
    /// deterministically by the CLI (ADR-0006). Debug surface in the GUI.
    fn registry(&self) -> Result<String>;
    /// Enable/disable one tool; the CLI writes the override.
    fn set_tool(&self, id: &str, enable: bool) -> Result<Value>;
    /// Enable/disable one toolset.
    fn set_toolset(&self, id: &str, enable: bool) -> Result<Value>;
}

#[derive(Debug)]
pub enum Error {
    /// The CLI could not be spawned — not installed, or not on `PATH`. The
    /// GUI surfaces `reactor doctor` as the follow-up.
    Spawn(std::io::Error),
    /// Non-zero exit, with whatever the CLI said on stderr.
    Failed { code: Option<i32>, stderr: String },
    /// stdout was not the JSON the payload expects.
    Parse { stdout: String, reason: String },
    /// The CLI did not answer in time; it was killed.
    Timeout,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Spawn(e) => write!(f, "could not run reactor: {e}"),
            Error::Failed { code, stderr } => {
                write!(f, "reactor failed (code {code:?}): {}", stderr.trim())
            }
            Error::Parse { reason, .. } => write!(f, "unreadable reactor output: {reason}"),
            Error::Timeout => write!(f, "reactor timed out — try `reactor doctor`"),
        }
    }
}

impl std::error::Error for Error {}

// ---------------------------------------------------------------------------
// CliClient
// ---------------------------------------------------------------------------

/// Shells out to the `reactor` binary. `cwd` is passed through so the CLI's
/// own cwd-bound behaviour matches the session's.
#[derive(Debug, Clone)]
pub struct CliClient {
    pub program: String,
    pub cwd: Option<PathBuf>,
}

impl CliClient {
    pub fn new(cwd: Option<PathBuf>) -> Self {
        Self {
            program: "reactor".to_owned(),
            cwd,
        }
    }

    /// Run `reactor <args>`, capture stdout and stderr, bound by
    /// [`EXEC_TIMEOUT`].
    ///
    /// The watchdog kills a wedged CLI so the reads unblock — the same
    /// backstop the extensions use (`EXEC_TIMEOUT_MS`), because a hung CLI
    /// must cost a slow refresh, not a frozen panel. stderr is drained on
    /// its own thread: a full stderr pipe would block the child mid-run.
    fn run(&self, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new(&self.program);
        cmd.args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }
        let mut child = cmd.spawn().map_err(Error::Spawn)?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Failed { code: None, stderr: "stdout not piped".into() })?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::Failed { code: None, stderr: "stderr not piped".into() })?;

        // stderr on a side thread, so a chatty CLI cannot deadlock the read
        // of stdout by filling its pipe.
        let stderr_thread = std::thread::spawn(move || {
            let mut buf = String::new();
            let mut reader = std::io::BufReader::new(stderr);
            let _ = std::io::Read::read_to_string(&mut reader, &mut buf);
            buf
        });

        // Watchdog: on timeout, kill the child so the read below unblocks.
        let killer: Arc<std::sync::Mutex<Option<Child>>> = Arc::new(std::sync::Mutex::new(Some(child)));
        let watchdog_child = Arc::clone(&killer);
        let (done_tx, done_rx) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("reactor-cli-watchdog".into())
            .spawn(move || {
                // `drop(done_tx)` signals the read finished — which
                // `recv_timeout` reports as Disconnected, the same enum a
                // real timeout arrives as. Only the timeout is the watchdog's
                // business; a disconnect means stand down.
                match done_rx.recv_timeout(EXEC_TIMEOUT) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if let Some(mut c) = watchdog_child.lock().unwrap().take() {
                            let _ = c.kill();
                        }
                    }
                }
            })
            .expect("watchdog thread");

        let mut out = String::new();
        let read = std::io::Read::read_to_string(&mut stdout, &mut out);
        drop(done_tx); // unblock the watchdog: the read finished
        read.map_err(|e| Error::Failed { code: None, stderr: e.to_string() })?;
        let stderr_text = stderr_thread.join().unwrap_or_default();

        // Reap and judge: stdout's EOF is not the process exiting, so wait()
        // on the parked child — the stderr side thread keeps its pipe moving,
        // and the watchdog is the only thing that takes the child before
        // this, which is exactly the timeout case.
        let status = match killer.lock().unwrap().as_mut() {
            Some(child) => child.wait().map_err(|e| Error::Failed {
                code: None,
                stderr: e.to_string(),
            }),
            None => return Err(Error::Timeout),
        }?;
        if !status.success() {
            return Err(Error::Failed {
                code: status.code(),
                stderr: stderr_text,
            });
        }
        Ok(out)
    }

    fn run_json<T: for<'de> Deserialize<'de>>(&self, args: &[&str]) -> Result<T> {
        let stdout = self.run_with_format(args, true)?;
        serde_json::from_str(&stdout).map_err(|e| Error::Parse {
            reason: e.to_string(),
            stdout: stdout.chars().take(400).collect(),
        })
    }

    fn run_json_value(&self, args: &[&str]) -> Result<Value> {
        let stdout = self.run_with_format(args, true)?;
        serde_json::from_str(&stdout).map_err(|e| Error::Parse {
            reason: e.to_string(),
            stdout: stdout.chars().take(400).collect(),
        })
    }

    fn run_text(&self, args: &[&str]) -> Result<String> {
        self.run_with_format(args, false)
    }

    /// `run`, with `--format json` appended when the caller wants JSON — the
    /// flag is part of the CLI's output contract (ADR-0005), appended here so
    /// no call site forgets it.
    fn run_with_format(&self, args: &[&str], json: bool) -> Result<String> {
        if !json {
            return self.run(args);
        }
        let mut argv: Vec<&str> = args.to_vec();
        argv.push("--format");
        argv.push("json");
        self.run(&argv)
    }
}

impl ReactorClient for CliClient {
    fn tools(&self) -> Result<ToolsPayload> {
        self.run_json(&["tools", "list"])
    }

    fn toolsets(&self) -> Result<ToolsetsPayload> {
        self.run_json(&["toolsets", "list"])
    }

    fn services(&self, refresh: bool) -> Result<ServicesPayload> {
        if refresh {
            self.run_json(&["services", "--format", "json", "--refresh"])
        } else {
            self.run_json(&["services", "--format", "json"])
        }
    }

    fn state(&self) -> Result<StatePayload> {
        self.run_json(&["state"])
    }

    fn tool_detail(&self, id: &str) -> Result<ToolDetailPayload> {
        self.run_json(&["tools", "show", id])
    }

    fn registry(&self) -> Result<String> {
        self.run_text(&["registry"])
    }

    fn set_tool(&self, id: &str, enable: bool) -> Result<Value> {
        let verb = if enable { "enable" } else { "disable" };
        self.run_json_value(&["tools", verb, id])
    }

    fn set_toolset(&self, id: &str, enable: bool) -> Result<Value> {
        let verb = if enable { "enable" } else { "disable" };
        self.run_json_value(&["toolsets", verb, id])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The GUI's panels render whatever the CLI says; an unknown field must
    /// never break them — the CLI's schema may grow (gui/SPEC.md §5).
    #[test]
    fn tools_payload_tolerates_unknown_fields() {
        let p: ToolsPayload = serde_json::from_str(
            r#"{"schema": 1, "tools": [{"id": "bn", "name": "Binary Ninja",
               "desc": "d", "status": "present", "active": true,
               "future_field": {"x": 1}}]}"#,
        )
        .unwrap();
        assert_eq!(p.tools.len(), 1);
        assert_eq!(p.tools[0].id, "bn");
        assert_eq!(p.tools[0].status, "present");
    }

    /// The services payload and the `status` extension's footer consume the
    /// same contract — verified against `reactor services --format json`
    /// output on this machine.
    #[test]
    fn services_payload_matches_the_status_extensions_contract() {
        let p: ServicesPayload = serde_json::from_str(
            r#"{"schema": 1, "services": [
                {"id": "adb", "name": "adb", "label": "adb", "state": "up",
                 "detail": "2 devices", "status": "present", "active": true},
                {"id": "bn", "name": "Binary Ninja", "label": "BN session",
                 "state": "unknown", "detail": null, "status": "absent", "active": true}],
                "summary": {"up": 1, "unknown": 1}}"#,
        )
        .unwrap();
        assert_eq!(p.services[0].state, "up");
        assert_eq!(p.services[0].detail.as_deref(), Some("2 devices"));
        assert_eq!(p.summary.get("up"), Some(&1));
    }

    /// `override` is a Rust keyword — the serde rename must keep the wire
    /// name, or activation overrides would silently read as None.
    #[test]
    fn override_field_is_read_from_the_wire_name() {
        let p: ToolsPayload = serde_json::from_str(
            r#"{"schema": 1, "tools": [{"id": "yara", "name": "YARA", "desc": "d",
               "status": "present", "active": true, "override": "enabled"}]}"#,
        )
        .unwrap();
        assert_eq!(p.tools[0].override_kind.as_deref(), Some("enabled"));
    }

    /// The trait is object-safe — the GUI holds `Box<dyn ReactorClient>` so
    /// the future LibClient drops in behind the same seam.
    #[test]
    fn trait_is_object_safe() {
        fn assert_object_safe(_: &dyn ReactorClient) {}
        assert_object_safe(&CliClient {
            program: "reactor".into(),
            cwd: None,
        });
    }

    /// A live round trip against the real CLI — run manually, since CI may
    /// not have reactor installed: `cargo test -p reactor-client -- --ignored`.
    #[test]
    #[ignore = "needs `reactor` on PATH"]
    fn live_tools_list_round_trips() {
        let client = CliClient::new(None);
        let tools = client.tools().expect("tools list");
        assert_eq!(tools.schema, 1);
        assert!(!tools.tools.is_empty(), "the catalogue is never empty");
    }

    /// A live round trip through `set_tool` — confirmed against the real
    /// CLI while diagnosing the GUI's inverted-toggle bug (the CLI call
    /// itself was never the problem: `reactor tools enable/disable <id>
    /// --format json` round-trips cleanly).
    #[test]
    #[ignore = "needs `reactor` on PATH"]
    fn live_set_tool_round_trips() {
        let client = CliClient::new(None);
        let response = client.set_tool("bn", true).expect("set_tool");
        assert_eq!(response.get("schema").and_then(Value::as_i64), Some(1));
    }
}