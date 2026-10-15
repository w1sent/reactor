//! The MCP client: connects to the servers in `~/.reactor/mcp.json` and gives the agent their
//! tools ([ADR-0048](../../../docs/adr/0048-mcp-tools-are-catalogue-entries-the-model-gets-as-definitions.md)).
//!
//! rig's own MCP support (`rmcp_tools`) plugs tools into rig's agent, which REactor does not
//! use -- the loop is ours ([ADR-0041](../../../docs/adr/0041-the-agent-loop-owns-the-message-list.md)).
//! So this talks to the `rmcp` client directly and hands the loop what it already understands:
//! a [`ToolSpec`] to advertise and an output to record.
//!
//! One [`McpHost`] is shared by every session in a process. It has a runtime of its own, so a
//! server's connection does not end with the session that happened to start it. Which of the
//! tools are advertised is the catalogue's activation (`reactor-core`); this knows only what
//! the servers offer. What it finds is written to `mcp-cache.json`, which is how the catalogue
//! -- a library with no async client in it -- lists the tools.
//!
//! Like a bash command, a call is not gated by activation: deactivating a tool stops it being
//! advertised, nothing more ([ADR-0007](../../../docs/adr/0007-deactivation-is-soft.md)).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reactor_core::Paths;
use reactor_core::mcp::{self, Cache, CachedServer, CachedTool, Config, Reach, Server, Transport};
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, ContentBlock, Implementation,
};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::context::estimate_tokens;
use crate::llm::ToolSpec;
use crate::tools::ToolOutput;

/// How long a server has to start and list its tools.
/// How long a connected server has to answer a tool-list request before it counts as down.
const PING_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How much of a server's own error output is kept to explain a failure to start.
const STDERR_KEPT: usize = 2000;

type Client = RunningService<RoleClient, ClientConfig>;
type Callback = Arc<dyn Fn() + Send + Sync>;

/// One tool a server offers, as the model is told about it.
#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    /// The catalogue id, and the name the model calls it by (`reactor_core::mcp::assign_ids`).
    pub id: String,
    pub server: String,
    /// The name the server knows it by.
    pub name: String,
    pub description: String,
    pub schema: Value,
}

impl McpTool {
    pub fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.id.clone(),
            description: self.description.clone(),
            parameters: self.schema.clone(),
        }
    }

    /// What the definition costs every request while it is advertised.
    pub fn tokens(&self) -> u64 {
        let s = self.spec();
        estimate_tokens(&s.name)
            + estimate_tokens(&s.description)
            + estimate_tokens(&s.parameters.to_string())
    }
}

/// Where one server stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// Switched off in the config: not connected to.
    Off,
    Connecting,
    Connected,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStatus {
    pub name: String,
    pub standing: Standing,
    pub tools: usize,
}

struct Live {
    /// The config this connection was made from: a changed one is a new connection.
    server: Server,
    standing: Standing,
    tools: Vec<McpTool>,
    client: Option<Arc<Client>>,
    /// Told apart from a newer attempt for the same server, whose result must not be
    /// overwritten by a slow older one.
    attempt: u64,
}

#[derive(Default)]
struct Inner {
    live: BTreeMap<String, Live>,
    attempts: u64,
}

pub struct McpHost {
    paths: Paths,
    rt: Mutex<Option<tokio::runtime::Runtime>>,
    handle: tokio::runtime::Handle,
    inner: Arc<Mutex<Inner>>,
    /// Called whenever a server's standing changes, from the host's own threads: it tells a
    /// frontend to look again.
    on_change: Arc<Mutex<Option<Callback>>>,
    /// Counts the changes, for a frontend that polls rather than being called.
    generation: Arc<std::sync::atomic::AtomicU64>,
}

impl Drop for McpHost {
    fn drop(&mut self) {
        // A runtime cannot be dropped from inside another; this lets go without waiting. The
        // child processes end with their connections.
        if let Some(rt) = self.rt.lock().unwrap().take() {
            rt.shutdown_background();
        }
    }
}

impl McpHost {
    pub fn new(paths: Paths) -> Arc<McpHost> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("reactor-mcp")
            .enable_all()
            .build()
            .expect("the MCP runtime starts");
        Arc::new(McpHost {
            paths,
            handle: rt.handle().clone(),
            rt: Mutex::new(Some(rt)),
            inner: Arc::default(),
            on_change: Arc::default(),
            generation: Arc::default(),
        })
    }

    pub fn on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.on_change.lock().unwrap() = Some(Arc::new(f));
    }

    /// Bring the connections in line with `mcp.json`: connect to servers that are new or
    /// changed, let go of those that are gone or switched off, and leave the rest alone.
    /// Returns at once; connecting happens in the background.
    pub fn sync(self: &Arc<Self>) {
        let config = match Config::load(&self.paths) {
            Ok(c) => c,
            // A config that cannot be read changes nothing: what is connected stays.
            Err(_) => return,
        };
        let mut start: Vec<(Server, u64)> = Vec::new();
        {
            let mut inner = self.inner.lock().unwrap();
            inner
                .live
                .retain(|name, _| config.servers.iter().any(|s| &s.name == name));
            for server in &config.servers {
                let same = inner
                    .live
                    .get(&server.name)
                    .is_some_and(|l| l.server == *server);
                if same {
                    continue;
                }
                inner.attempts += 1;
                let attempt = inner.attempts;
                let enabled = server.enabled;
                inner.live.insert(
                    server.name.clone(),
                    Live {
                        server: server.clone(),
                        standing: if enabled {
                            Standing::Connecting
                        } else {
                            Standing::Off
                        },
                        tools: Vec::new(),
                        client: None,
                        attempt,
                    },
                );
                if enabled {
                    start.push((server.clone(), attempt));
                }
            }
        }
        self.write_cache(&config);
        self.changed();
        for (server, attempt) in start {
            self.spawn_connect(server, attempt);
        }
    }

    /// Connect to one server again, whatever it was doing.
    pub fn reconnect(self: &Arc<Self>, name: &str) {
        let Ok(config) = Config::load(&self.paths) else {
            return;
        };
        let Some(server) = config.get(name).cloned().filter(|s| s.enabled) else {
            return;
        };
        let attempt = {
            let mut inner = self.inner.lock().unwrap();
            inner.attempts += 1;
            let attempt = inner.attempts;
            inner.live.insert(
                name.to_string(),
                Live {
                    server: server.clone(),
                    standing: Standing::Connecting,
                    tools: Vec::new(),
                    client: None,
                    attempt,
                },
            );
            attempt
        };
        self.changed();
        self.spawn_connect(server, attempt);
    }

    /// Find out which connected servers have gone away, and connect again to every server that
    /// is down. A connection that was made once is not known to be alive until it is asked:
    /// each is asked for its tool list, and one that does not answer is marked failed. Returns
    /// at once; the outcome arrives as changes of standing.
    pub fn recheck(self: &Arc<Self>) {
        let asked: Vec<(String, u64, Arc<Client>)> = {
            let inner = self.inner.lock().unwrap();
            inner
                .live
                .iter()
                .filter(|(_, l)| l.standing == Standing::Connected)
                .filter_map(|(n, l)| Some((n.clone(), l.attempt, l.client.clone()?)))
                .collect()
        };
        let host = self.clone();
        self.handle.spawn(async move {
            let mut down = Vec::new();
            for (name, attempt, client) in asked {
                let answer =
                    tokio::time::timeout(PING_TIMEOUT, client.list_all_tools()).await;
                let why = match answer {
                    Ok(Ok(_)) => continue,
                    Ok(Err(e)) => format!("{e}"),
                    Err(_) => format!("no answer in {} seconds", PING_TIMEOUT.as_secs()),
                };
                let mut inner = host.inner.lock().unwrap();
                if let Some(live) = inner
                    .live
                    .get_mut(&name)
                    .filter(|l| l.attempt == attempt)
                {
                    live.standing = Standing::Failed(why);
                    live.client = None;
                    down.push(name);
                }
            }
            if !down.is_empty() {
                if let Ok(config) = Config::load(&host.paths) {
                    host.write_cache(&config);
                }
                host.changed();
            }
            let failed: Vec<String> = host
                .inner
                .lock()
                .unwrap()
                .live
                .iter()
                .filter(|(_, l)| matches!(l.standing, Standing::Failed(_)))
                .map(|(n, _)| n.clone())
                .collect();
            for name in failed {
                host.reconnect(&name);
            }
        });
    }

    fn spawn_connect(self: &Arc<Self>, server: Server, attempt: u64) {
        let host = self.clone();
        self.handle.spawn(async move {
            let result = tokio::time::timeout(CONNECT_TIMEOUT, connect(&server))
                .await
                .unwrap_or_else(|_| {
                    Err(format!(
                        "no answer in {} seconds",
                        CONNECT_TIMEOUT.as_secs()
                    ))
                });
            {
                let mut inner = host.inner.lock().unwrap();
                // Superseded while connecting: this result belongs to nobody.
                let Some(live) = inner
                    .live
                    .get_mut(&server.name)
                    .filter(|l| l.attempt == attempt)
                else {
                    return;
                };
                match result {
                    Ok((client, tools)) => {
                        live.standing = Standing::Connected;
                        live.tools = tools;
                        live.client = Some(Arc::new(client));
                    }
                    Err(why) => live.standing = Standing::Failed(why),
                }
            }
            if let Ok(config) = Config::load(&host.paths) {
                host.write_cache(&config);
            }
            host.changed();
        });
    }

    /// How many times a server's standing has changed: a frontend that polls compares it with
    /// what it saw last.
    pub fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn changed(&self) {
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let f = self.on_change.lock().unwrap().clone();
        if let Some(f) = f {
            f();
        }
    }

    /// What the catalogue is told: for every server that is on, whether it answered and what
    /// it offers. A server that fails keeps the tools it last offered, so they stay listed
    /// (as unavailable) and what the user activated is not forgotten.
    ///
    /// This is also where the tools get their ids. The catalogue derives ids from this very
    /// list (`reactor_core::mcp::assign_ids`), so both must be fed the same names in the same
    /// order: the ones a down server last offered count too.
    fn write_cache(&self, config: &Config) {
        let before = Cache::load(&self.paths);
        let taken: BTreeSet<String> = reactor_core::catalogue::load_catalogue(&self.paths)
            .map(|c| {
                c.tools
                    .iter()
                    .filter(|t| t.mcp.is_none())
                    .map(|t| t.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let mut inner = self.inner.lock().unwrap();

        // Which tools each server offers, as far as is known.
        let mut offered: Vec<(&Server, Option<Vec<String>>)> = Vec::new();
        for server in config.servers.iter().filter(|s| s.enabled) {
            let Some(live) = inner.live.get(&server.name) else {
                continue;
            };
            let names = match (&live.standing, before.servers.get(&server.name)) {
                (Standing::Connected, _) => {
                    Some(live.tools.iter().map(|t| t.name.clone()).collect())
                }
                (_, Some(b)) => Some(b.tools.iter().map(|t| t.name.clone()).collect()),
                (Standing::Failed(_), None) => Some(Vec::new()),
                // Not known yet, and nothing known before: not listed.
                (_, None) => None,
            };
            offered.push((server, names));
        }
        let pairs: Vec<(&str, &str)> = offered
            .iter()
            .flat_map(|(s, names)| {
                names
                    .iter()
                    .flatten()
                    .map(move |n| (s.name.as_str(), n.as_str()))
            })
            .collect();
        let assigned = reactor_core::mcp::assign_ids(config.prefix, &taken, &pairs);
        let id_of: HashMap<(&str, &str), Option<&String>> = pairs
            .iter()
            .copied()
            .zip(assigned.iter().map(Option::as_ref))
            .collect();
        for live in inner.live.values_mut() {
            let server = live.server.name.clone();
            for t in &mut live.tools {
                if let Some(Some(id)) = id_of.get(&(server.as_str(), t.name.as_str())) {
                    t.id = (*id).clone();
                }
            }
            // A name that repeats an earlier tool's id is not offered.
            live.tools.retain(|t| {
                id_of
                    .get(&(server.as_str(), t.name.as_str()))
                    .is_none_or(|id| id.is_some())
            });
        }

        let mut cache = Cache::default();
        for (server, names) in &offered {
            if names.is_none() {
                continue;
            }
            let live = &inner.live[&server.name];
            let entry = match &live.standing {
                Standing::Connected => CachedServer {
                    reach: Reach::Connected,
                    error: None,
                    tools: live.tools.iter().map(cached).collect(),
                },
                Standing::Failed(why) => CachedServer {
                    reach: Reach::Failed,
                    error: Some(why.clone()),
                    tools: before
                        .servers
                        .get(&server.name)
                        .map(|b| b.tools.clone())
                        .unwrap_or_default(),
                },
                // Not known yet: what was known stays, as it was.
                Standing::Connecting | Standing::Off => before.servers[&server.name].clone(),
            };
            cache.servers.insert(server.name.clone(), entry);
        }
        drop(inner);
        // Best-effort: a cache that cannot be written is rebuilt by the next connection.
        let _ = cache.save(&self.paths);
    }

    pub fn status(&self) -> Vec<ServerStatus> {
        let inner = self.inner.lock().unwrap();
        inner
            .live
            .iter()
            .map(|(name, l)| ServerStatus {
                name: name.clone(),
                standing: l.standing.clone(),
                tools: l.tools.len(),
            })
            .collect()
    }

    /// The definitions to advertise: those of `active` ids that a connected server offers, in
    /// id order -- fixed, so the request does not change from one turn to the next.
    pub fn specs(&self, active: &BTreeSet<String>) -> Vec<ToolSpec> {
        self.grouped(active).into_iter().map(|(_, s)| s).collect()
    }

    /// The same, each with the server it comes from: the group it is deferred with
    /// (ADR-0055).
    pub fn grouped(&self, active: &BTreeSet<String>) -> Vec<(String, ToolSpec)> {
        let inner = self.inner.lock().unwrap();
        let mut found: BTreeMap<&str, &McpTool> = BTreeMap::new();
        for l in inner
            .live
            .values()
            .filter(|l| l.standing == Standing::Connected)
        {
            for t in &l.tools {
                if active.contains(&t.id) {
                    found.insert(&t.id, t);
                }
            }
        }
        found
            .values()
            .map(|t| (t.server.clone(), t.spec()))
            .collect()
    }

    /// Whether `name` is a tool of a server this host knows.
    pub fn knows(&self, name: &str) -> bool {
        self.inner
            .lock()
            .unwrap()
            .live
            .values()
            .any(|l| l.tools.iter().any(|t| t.id == name))
    }

    /// Run a tool. Whether it is active makes no difference; a server that is not connected
    /// says so as the tool's output.
    pub async fn call(&self, id: &str, args: Value, cancel: &CancellationToken) -> ToolOutput {
        let (client, tool) = {
            let inner = self.inner.lock().unwrap();
            let Some((live, tool)) = inner
                .live
                .values()
                .find_map(|l| l.tools.iter().find(|t| t.id == id).map(|t| (l, t.clone())))
            else {
                return ToolOutput::err(format!("unknown tool `{id}`"));
            };
            match (&live.standing, &live.client) {
                (Standing::Connected, Some(c)) => (c.clone(), tool),
                (Standing::Failed(why), _) => {
                    return ToolOutput::err(format!(
                        "MCP server `{}` is not connected: {why}",
                        tool.server
                    ));
                }
                _ => {
                    return ToolOutput::err(format!(
                        "MCP server `{}` is not connected",
                        tool.server
                    ));
                }
            }
        };
        let arguments = match args {
            Value::Object(m) => Some(m),
            Value::Null => None,
            other => {
                return ToolOutput::err(format!(
                    "the arguments of `{id}` must be a JSON object, not {other}"
                ));
            }
        };
        let mut params = CallToolRequestParams::new(tool.name.clone());
        params.arguments = arguments;
        let server = tool.server.clone();
        // On the host's runtime, where the connection lives.
        let run = self
            .handle
            .spawn(async move { client.call_tool(params).await });
        tokio::select! {
            _ = cancel.cancelled() => ToolOutput::err("cancelled"),
            joined = run => match joined {
                Ok(Ok(result)) => output_of(result),
                Ok(Err(e)) => ToolOutput::err(format!("MCP server `{server}`: {e}")),
                Err(e) => ToolOutput::err(format!("MCP server `{server}`: {e}")),
            },
        }
    }
}

fn cached(t: &McpTool) -> CachedTool {
    CachedTool {
        name: t.name.clone(),
        desc: first_line(&t.description),
        tokens: t.tokens(),
    }
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut out: String = line.chars().take(160).collect();
    if line.chars().count() > 160 {
        out.push('…');
    }
    out
}

/// What the model reads: the text blocks, with a note for what a model cannot be handed here.
pub fn output_of(result: rmcp::model::CallToolResult) -> ToolOutput {
    let mut parts: Vec<String> = Vec::new();
    for block in &result.content {
        parts.push(match block {
            ContentBlock::Text(t) => t.text.clone(),
            other => describe_block(other),
        });
    }
    if parts.is_empty()
        && let Some(structured) = &result.structured_content
    {
        parts.push(serde_json::to_string_pretty(structured).unwrap_or_default());
    }
    let text = parts.join("\n\n");
    if result.is_error == Some(true) {
        ToolOutput::err(if text.is_empty() {
            "the tool failed".to_string()
        } else {
            text
        })
    } else {
        ToolOutput::ok(text)
    }
}

/// A block that is not text, said in words: the model is given text.
fn describe_block(block: &ContentBlock) -> String {
    let v = serde_json::to_value(block).unwrap_or(Value::Null);
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("content");
    let size = |key: &str| v.get(key).and_then(Value::as_str).map(|d| d.len() * 3 / 4);
    match kind {
        "image" | "audio" => format!(
            "[{kind}: {}, {} bytes -- not shown to you]",
            v.get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("unknown type"),
            size("data").unwrap_or(0)
        ),
        "resource" => {
            let r = v.get("resource").cloned().unwrap_or(Value::Null);
            match r.get("text").and_then(Value::as_str) {
                Some(text) => text.to_string(),
                None => format!(
                    "[resource: {} -- binary, not shown to you]",
                    r.get("uri").and_then(Value::as_str).unwrap_or("?")
                ),
            }
        }
        "resource_link" => format!(
            "[resource link: {}]",
            v.get("uri").and_then(Value::as_str).unwrap_or("?")
        ),
        other => format!("[{other} content]"),
    }
}

/// A tool's JSON Schema as a model API wants it: an object schema, always.
fn schema_of(input: &Map<String, Value>) -> Value {
    let mut schema = input.clone();
    schema
        .entry("type")
        .or_insert_with(|| Value::String("object".into()));
    if schema.get("type").and_then(Value::as_str) == Some("object") {
        schema
            .entry("properties")
            .or_insert_with(|| Value::Object(Map::new()));
    }
    Value::Object(schema)
}

fn tools_of(server: &str, listed: Vec<rmcp::model::Tool>) -> Vec<McpTool> {
    let mut out: Vec<McpTool> = Vec::new();
    for t in listed {
        let id = mcp::tool_id(server, &t.name);
        if out.iter().any(|o| o.id == id) {
            continue;
        }
        let description = t
            .description
            .as_deref()
            .map(str::to_string)
            .or(t.title.clone())
            .unwrap_or_default();
        out.push(McpTool {
            id,
            server: server.to_string(),
            name: t.name.to_string(),
            description,
            schema: schema_of(&t.input_schema),
        });
    }
    out
}

fn client_config() -> ClientConfig {
    let mut me = Implementation::default();
    me.name = "reactor".into();
    me.version = env!("CARGO_PKG_VERSION").into();
    ClientConfig::new(ClientCapabilities::default(), me)
}

/// Start the server, shake hands and list its tools.
async fn connect(server: &Server) -> Result<(Client, Vec<McpTool>), String> {
    let client = match &server.transport {
        Transport::Stdio { command, args, env } => {
            let mut cmd = tokio::process::Command::new(mcp::expand(command));
            cmd.args(args.iter().map(|a| mcp::expand(a)));
            for (k, v) in env {
                cmd.env(k, mcp::expand(v));
            }
            let (transport, stderr) = TokioChildProcess::builder(cmd)
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("cannot start `{command}`: {e}"))?;
            let tail = Arc::new(Mutex::new(String::new()));
            if let Some(stderr) = stderr {
                let tail = tail.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncBufReadExt, BufReader};
                    let mut lines = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let mut t = tail.lock().unwrap();
                        t.push_str(&line);
                        t.push('\n');
                        if t.len() > STDERR_KEPT {
                            let cut = t.len() - STDERR_KEPT;
                            let at = (cut..t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(0);
                            t.drain(..at);
                        }
                    }
                });
            }
            client_config().serve(transport).await.map_err(|e| {
                let said = tail.lock().unwrap().trim().to_string();
                if said.is_empty() {
                    format!("{e}")
                } else {
                    format!("{e}\n{said}")
                }
            })?
        }
        Transport::Http { url, headers } => {
            let mut config = StreamableHttpClientTransportConfig::with_uri(mcp::expand(url));
            let mut custom = HashMap::new();
            for (k, v) in headers {
                let v = mcp::expand(v);
                // A bearer token has its own slot, and the transport adds the prefix.
                if k.eq_ignore_ascii_case("authorization")
                    && let Some(token) = v.strip_prefix("Bearer ")
                {
                    config = config.auth_header(token.trim());
                    continue;
                }
                let name = http::HeaderName::from_bytes(k.as_bytes())
                    .map_err(|e| format!("header `{k}`: {e}"))?;
                let value =
                    http::HeaderValue::from_str(&v).map_err(|e| format!("header `{k}`: {e}"))?;
                custom.insert(name, value);
            }
            let config = config.custom_headers(custom);
            client_config()
                .serve(StreamableHttpClientTransport::from_config(config))
                .await
                .map_err(|e| format!("{e}"))?
        }
    };
    let listed = client
        .list_all_tools()
        .await
        .map_err(|e| format!("listing tools: {e}"))?;
    let tools = tools_of(&server.name, listed);
    Ok((client, tools))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_schema_without_a_type_becomes_an_object_schema() {
        let input = json!({"properties": {"q": {"type": "string"}}});
        let schema = schema_of(input.as_object().unwrap());
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["q"]["type"], "string");
        let empty = schema_of(&Map::new());
        assert_eq!(empty, json!({"type": "object", "properties": {}}));
    }

    #[test]
    fn a_description_is_cut_to_a_line_for_the_list() {
        assert_eq!(first_line("\n  First line.\nSecond."), "First line.");
        assert_eq!(first_line(""), "");
        assert!(first_line(&"x".repeat(500)).chars().count() <= 161);
    }
}
