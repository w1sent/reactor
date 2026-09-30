//! `ReactorClient` — the seam between the GUI and the catalogue.
//!
//! The GUI never parses `tools.toml` and never re-implements catalogue
//! semantics ([ADR-0005](../../../docs/adr/0005-reactor-cli-stdlib-python.md)).
//! It asks for facts and renders them, through one trait with two
//! implementations ([ADR-0034](../../../docs/adr/0034-reactor-cli-becomes-a-rust-library-with-a-binary.md)):
//!
//! - [`LibClient`] calls `reactor-core` in-process — no process per panel
//!   refresh. This is the default.
//! - [`CliClient`] shells out to the `reactor` binary and parses
//!   `--format json`. It is the debug fallback (`REACTOR_GUI_CLIENT=cli`): if
//!   the two ever disagree, that is a bug at the library boundary, and being
//!   able to run both is how it gets found. `crates/reactor-cli/tests/agree.rs`
//!   runs them side by side.
//!
//! Both paths use the same probe TTLs and `cache.json`, so the GUI adds no
//! second probe policy
//! ([ADR-0014](../../../docs/adr/0014-extensions-share-the-cache-not-each-other.md)).
//!
//! Every payload struct tolerates unknown fields: the JSON contract may grow,
//! and a GUI that hard-fails on a new field is a GUI that breaks on every
//! update. [`LibClient`] hands core's reports to these same structs through
//! their serialized form, so the GUI's view of the contract is one thing
//! whichever path filled it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

/// How long one CLI invocation may run: a wedged CLI costs one slow refresh, not a
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
    /// `reactor-core` refused (bad catalogue, unknown tool, unwritable state):
    /// the same message the CLI would have printed after `reactor:`.
    Core(String),
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
            Error::Core(message) => write!(f, "reactor: {message}"),
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
    /// Extra environment for the child (`REACTOR_CONFIG_DIR`, in tests).
    pub envs: Vec<(String, String)>,
}

impl CliClient {
    pub fn new(cwd: Option<PathBuf>) -> Self {
        Self {
            program: "reactor".to_owned(),
            cwd,
            envs: Vec::new(),
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
            .stdin(Stdio::null())
            .envs(self.envs.iter().map(|(k, v)| (k, v)));
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
            // In `--format json` the CLI reports failure as `{"error": …}` on
            // stdout and leaves stderr empty; surface that message rather than
            // a blank "reactor failed".
            let stderr = if stderr_text.trim().is_empty() {
                serde_json::from_str::<Value>(&out)
                    .ok()
                    .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned))
                    .unwrap_or(stderr_text)
            } else {
                stderr_text
            };
            return Err(Error::Failed { code: status.code(), stderr });
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

// ---------------------------------------------------------------------------
// LibClient
// ---------------------------------------------------------------------------

/// Calls `reactor-core` in-process: the same functions the `reactor` binary
/// runs, without the process. `cwd` selects project-scoped activation state
/// exactly as the CLI's working directory does.
#[derive(Debug, Clone)]
pub struct LibClient {
    paths: reactor_core::Paths,
}

impl LibClient {
    /// Config dir and shipped catalogue from the environment, like the CLI.
    pub fn new(cwd: Option<PathBuf>) -> Self {
        let mut paths = reactor_core::Paths::from_env();
        paths.cwd = cwd.or(paths.cwd);
        Self { paths }
    }

    /// Explicit paths — for tests and embedders that must not read the ambient
    /// environment.
    pub fn with_paths(paths: reactor_core::Paths) -> Self {
        Self { paths }
    }

    /// A report as the `--format json` contract spells it, parsed into the
    /// GUI's tolerant view of it. Going through the serialized form (not a
    /// field-by-field copy) is what keeps this path and [`CliClient`] unable
    /// to drift: there is one description of the payload, and it is the wire.
    fn wire<R: reactor_core::Report, T: for<'de> Deserialize<'de>>(
        done: reactor_core::Result<reactor_core::Done<R>>,
    ) -> Result<T> {
        let report = done.map_err(|e| Error::Core(e.to_string()))?.report;
        let value = serde_json::to_value(reactor_core::report::Envelope {
            schema: reactor_core::SCHEMA,
            payload: &report,
        })
        .map_err(|e| Error::Core(e.to_string()))?;
        serde_json::from_value(value).map_err(|e| Error::Parse {
            reason: e.to_string(),
            stdout: String::new(),
        })
    }
}

impl ReactorClient for LibClient {
    fn tools(&self) -> Result<ToolsPayload> {
        Self::wire(reactor_core::commands::tools_list(&self.paths, &Default::default()))
    }

    fn toolsets(&self) -> Result<ToolsetsPayload> {
        Self::wire(reactor_core::commands::toolsets_list(&self.paths))
    }

    fn services(&self, refresh: bool) -> Result<ServicesPayload> {
        let flags = reactor_core::commands::ProbeFlags { refresh, cached: false };
        Self::wire(reactor_core::commands::services(&self.paths, flags))
    }

    fn state(&self) -> Result<StatePayload> {
        Self::wire(reactor_core::commands::state(&self.paths))
    }

    fn tool_detail(&self, id: &str) -> Result<ToolDetailPayload> {
        Self::wire(reactor_core::commands::tools_show(&self.paths, id, Default::default()))
    }

    fn registry(&self) -> Result<String> {
        let done = reactor_core::commands::registry(&self.paths, Default::default())
            .map_err(|e| Error::Core(e.to_string()))?;
        // What `reactor registry` prints: the block and a newline.
        Ok(format!("{}\n", done.report.block))
    }

    fn set_tool(&self, id: &str, enable: bool) -> Result<Value> {
        let ids = [id.to_owned()];
        Self::wire(reactor_core::commands::set_activation(&self.paths, &ids, true, Some(enable)))
    }

    fn set_toolset(&self, id: &str, enable: bool) -> Result<Value> {
        let ids = [id.to_owned()];
        Self::wire(reactor_core::commands::set_activation(&self.paths, &ids, false, Some(enable)))
    }
}

// ---------------------------------------------------------------------------
// Client — the one the GUI holds
// ---------------------------------------------------------------------------

/// Which implementation the GUI is running on. A plain enum rather than a
/// boxed trait object so it is `Clone` — panels clone the client into
/// background tasks.
#[derive(Debug, Clone)]
pub enum Client {
    Lib(LibClient),
    Cli(CliClient),
}

/// `REACTOR_GUI_CLIENT=cli` selects the subprocess path.
pub const CLIENT_ENV: &str = "REACTOR_GUI_CLIENT";

impl Client {
    /// The library, unless the environment asks for the CLI.
    pub fn from_env(cwd: Option<PathBuf>) -> Self {
        match std::env::var(CLIENT_ENV).as_deref() {
            Ok("cli") => Client::Cli(CliClient::new(cwd)),
            _ => Client::Lib(LibClient::new(cwd)),
        }
    }

    /// Like [`Client::from_env`], but for a session whose activation is scoped to it
    /// (ADR-0038): the library reads exactly the given `paths`. The subprocess fallback has
    /// no notion of a session, so under `REACTOR_GUI_CLIENT=cli` the panels show the machine's
    /// state instead — a debugging aid, and said so by `kind()`.
    pub fn for_session(paths: reactor_core::Paths, cwd: Option<PathBuf>) -> Self {
        match std::env::var(CLIENT_ENV).as_deref() {
            Ok("cli") => Client::Cli(CliClient::new(cwd)),
            _ => Client::Lib(LibClient::with_paths(paths)),
        }
    }

    /// For the status bar and bug reports: which path answered.
    pub fn kind(&self) -> &'static str {
        match self {
            Client::Lib(_) => "lib",
            Client::Cli(_) => "cli",
        }
    }
}

impl ReactorClient for Client {
    fn tools(&self) -> Result<ToolsPayload> {
        match self { Client::Lib(c) => c.tools(), Client::Cli(c) => c.tools() }
    }
    fn toolsets(&self) -> Result<ToolsetsPayload> {
        match self { Client::Lib(c) => c.toolsets(), Client::Cli(c) => c.toolsets() }
    }
    fn services(&self, refresh: bool) -> Result<ServicesPayload> {
        match self { Client::Lib(c) => c.services(refresh), Client::Cli(c) => c.services(refresh) }
    }
    fn state(&self) -> Result<StatePayload> {
        match self { Client::Lib(c) => c.state(), Client::Cli(c) => c.state() }
    }
    fn tool_detail(&self, id: &str) -> Result<ToolDetailPayload> {
        match self { Client::Lib(c) => c.tool_detail(id), Client::Cli(c) => c.tool_detail(id) }
    }
    fn registry(&self) -> Result<String> {
        match self { Client::Lib(c) => c.registry(), Client::Cli(c) => c.registry() }
    }
    fn set_tool(&self, id: &str, enable: bool) -> Result<Value> {
        match self { Client::Lib(c) => c.set_tool(id, enable), Client::Cli(c) => c.set_tool(id, enable) }
    }
    fn set_toolset(&self, id: &str, enable: bool) -> Result<Value> {
        match self { Client::Lib(c) => c.set_toolset(id, enable), Client::Cli(c) => c.set_toolset(id, enable) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The GUI's panels render whatever the CLI says; an unknown field must
    /// never break them — the CLI's schema may grow (SPEC.md §5).
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
        assert_object_safe(&CliClient::new(None));
        assert_object_safe(&LibClient::new(None));
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