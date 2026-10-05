//! MCP servers, as the catalogue sees them (ADR-0048).
//!
//! `~/.reactor/mcp.json` says which servers there are; `mcp-cache.json` says what the agent
//! last found on them. This module reads and writes both and turns what they say into
//! catalogue entries, so that an MCP tool is listed, activated, tagged and put into toolsets
//! exactly like a tool in `tools.toml`. Connecting to a server is not done here -- that needs
//! an async MCP client, and this crate is files and subprocesses only (ADR-0034); the agent
//! crate connects and writes the cache.
//!
//! An MCP tool differs from a catalogue tool in one way that matters: it is advertised to the
//! model as a tool definition with its whole JSON schema, on every request, for as long as it
//! is active. A catalogue tool costs one registry line. That cost is recorded here as an
//! estimate per tool, so the user can see what activating it does to the context.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::catalogue::{DetectKind, McpRef, OrderedMap, Tool};
use crate::err;
use crate::error::Result;
use crate::json::{io_reason, write_json_atomic};
use crate::paths::Paths;

/// The tag every MCP tool carries, so a toolset can select them all (`tags = ["mcp"]`).
pub const TAG: &str = "mcp";

/// The longest name a model API accepts for a tool.
const MAX_ID: usize = 64;

/// How a server is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    /// A subprocess spoken to over its standard input and output.
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    /// A streamable-HTTP endpoint.
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub name: String,
    pub transport: Transport,
    /// The user's own tags: every tool of this server carries them, which is how a toolset
    /// picks a server's tools up.
    pub tags: Vec<String>,
    /// A server that is switched off is not connected to.
    pub enabled: bool,
}

impl Server {
    pub fn is_http(&self) -> bool {
        matches!(self.transport, Transport::Http { .. })
    }

    /// What identifies it in a list: the command line or the URL.
    pub fn target(&self) -> String {
        match &self.transport {
            Transport::Stdio { command, args, .. } => std::iter::once(command.as_str())
                .chain(args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "),
            Transport::Http { url, .. } => url.clone(),
        }
    }
}

/// `mcp.json`, in the `mcpServers` shape other MCP clients use, so an entry can be pasted
/// from a server's own documentation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub servers: Vec<Server>,
    /// Whether a tool is named `mcp__<server>__<tool>` rather than by its own name. Off by
    /// default: the shorter name is what the server calls it, and what a model has most
    /// likely seen. Switching it changes every id, so activation entries for MCP tools do
    /// not survive it.
    pub prefix: bool,
}

/// A server name: it is part of every tool's id, which is the name the model calls it by
/// (letters, digits and `-` only -- `__` separates the parts of an id).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 24
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !name.starts_with('-')
}

pub fn valid_tag(tag: &str) -> bool {
    !tag.is_empty() && !tag.chars().any(char::is_whitespace) && !tag.contains(',')
}

impl Config {
    pub fn path(paths: &Paths) -> std::path::PathBuf {
        paths.live("mcp.json")
    }

    /// The servers on disk. A missing file is no servers; a file that cannot be understood is
    /// an error, so that saving never writes over something the user wrote by hand.
    pub fn load(paths: &Paths) -> Result<Config> {
        let path = Config::path(paths);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(err!("{}: {}", path.display(), io_reason(&e))),
        };
        if text.trim().is_empty() {
            return Ok(Config::default());
        }
        let doc: Value =
            serde_json::from_str(&text).map_err(|e| err!("{}: {e}", path.display()))?;
        Config::from_value(&doc).map_err(|e| err!("{}: {e}", path.display()))
    }

    pub fn from_value(doc: &Value) -> std::result::Result<Config, String> {
        let prefix = match doc.get("prefixToolNames") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err("prefixToolNames: expected true or false".into()),
        };
        let servers = match doc.get("mcpServers") {
            None | Some(Value::Null) => {
                return Ok(Config {
                    servers: vec![],
                    prefix,
                });
            }
            Some(Value::Object(m)) => m,
            Some(_) => return Err("mcpServers: expected an object".into()),
        };
        let mut out = Vec::new();
        for (name, spec) in servers {
            out.push(
                Server::from_value(name, spec).map_err(|e| format!("mcpServers.{name}: {e}"))?,
            );
        }
        Ok(Config {
            servers: out,
            prefix,
        })
    }

    pub fn to_value(&self) -> Value {
        let mut servers = Map::new();
        for s in &self.servers {
            servers.insert(s.name.clone(), s.to_value());
        }
        let mut doc = Map::new();
        if self.prefix {
            doc.insert("prefixToolNames".into(), true.into());
        }
        doc.insert("mcpServers".into(), Value::Object(servers));
        Value::Object(doc)
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        write_json_atomic(&Config::path(paths), &self.to_value())
    }

    pub fn get(&self, name: &str) -> Option<&Server> {
        self.servers.iter().find(|s| s.name == name)
    }

    /// Add a server, or replace the one of the same name in place.
    pub fn put(&mut self, server: Server) {
        match self.servers.iter_mut().find(|s| s.name == server.name) {
            Some(slot) => *slot = server,
            None => self.servers.push(server),
        }
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.servers.len();
        self.servers.retain(|s| s.name != name);
        self.servers.len() != before
    }
}

fn string_map(
    v: Option<&Value>,
    what: &str,
) -> std::result::Result<BTreeMap<String, String>, String> {
    match v {
        None | Some(Value::Null) => Ok(BTreeMap::new()),
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| {
                v.as_str()
                    .map(|s| (k.clone(), s.to_string()))
                    .ok_or_else(|| format!("{what}.{k}: expected a string"))
            })
            .collect(),
        Some(_) => Err(format!("{what}: expected an object of strings")),
    }
}

impl Server {
    pub fn from_value(name: &str, spec: &Value) -> std::result::Result<Server, String> {
        if !valid_name(name) {
            return Err(format!(
                "`{name}` is not a usable server name: up to 24 letters, digits and `-`, not starting with `-`"
            ));
        }
        let spec = spec.as_object().ok_or("expected an object")?;
        let text = |key: &str| {
            spec.get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        };
        let transport = match (text("command"), text("url")) {
            (Some(command), None) => Transport::Stdio {
                command: command.to_string(),
                args: match spec.get("args") {
                    None | Some(Value::Null) => vec![],
                    Some(Value::Array(a)) => a
                        .iter()
                        .map(|a| a.as_str().map(str::to_string))
                        .collect::<Option<_>>()
                        .ok_or("args: expected a list of strings")?,
                    Some(_) => return Err("args: expected a list of strings".into()),
                },
                env: string_map(spec.get("env"), "env")?,
            },
            (None, Some(url)) => Transport::Http {
                url: url.to_string(),
                headers: string_map(spec.get("headers"), "headers")?,
            },
            (Some(_), Some(_)) => {
                return Err("has both `command` and `url`: it is one or the other".into());
            }
            (None, None) => {
                return Err("needs a `command` (a local server) or a `url` (a remote one)".into());
            }
        };
        let tags: Vec<String> = match spec.get("tags") {
            None | Some(Value::Null) => vec![],
            Some(Value::Array(a)) => a
                .iter()
                .map(|a| a.as_str().map(str::to_string))
                .collect::<Option<_>>()
                .ok_or("tags: expected a list of strings")?,
            Some(_) => return Err("tags: expected a list of strings".into()),
        };
        if let Some(bad) = tags.iter().find(|t| !valid_tag(t)) {
            return Err(format!("tag `{bad}`: no spaces or commas"));
        }
        Ok(Server {
            name: name.to_string(),
            transport,
            tags,
            enabled: spec.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        })
    }

    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        match &self.transport {
            Transport::Stdio { command, args, env } => {
                m.insert("command".into(), command.clone().into());
                if !args.is_empty() {
                    m.insert("args".into(), args.clone().into());
                }
                if !env.is_empty() {
                    m.insert("env".into(), strings(env));
                }
            }
            Transport::Http { url, headers } => {
                m.insert("url".into(), url.clone().into());
                if !headers.is_empty() {
                    m.insert("headers".into(), strings(headers));
                }
            }
        }
        if !self.tags.is_empty() {
            m.insert("tags".into(), self.tags.clone().into());
        }
        if !self.enabled {
            m.insert("enabled".into(), false.into());
        }
        Value::Object(m)
    }
}

fn strings(m: &BTreeMap<String, String>) -> Value {
    Value::Object(
        m.iter()
            .map(|(k, v)| (k.clone(), v.clone().into()))
            .collect(),
    )
}

/// `${NAME}` in a configured value is the environment variable `NAME`, so a token need not be
/// written into the file. An unset variable becomes empty.
pub fn expand(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find('}') {
            Some(end) => {
                let name = &rest[start + 2..start + 2 + end];
                out.push_str(&std::env::var(name).unwrap_or_default());
                rest = &rest[start + 3 + end..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

// -- ids ------------------------------------------------------------------------------------

/// The names the agent itself gives its tools: an MCP tool must never take one, or the
/// model's call would go to ours. A test in `reactor-agent` keeps this list true.
pub const RESERVED: &[&str] = &[
    "bash",
    "read",
    "write",
    "edit",
    "history_index",
    "history_read",
    "history_search",
    "update_steps",
    "reactor_phase_complete",
    "graph_add",
    "graph_annotate",
    "graph_query",
    "graph_pull",
    "artifact_register",
    "docs_search",
];

fn clean(tool: &str) -> String {
    tool.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The prefixed id of a server's tool, `mcp__<server>__<tool>`, the form other MCP clients
/// use. A tool name from a server may hold characters a model API rejects; those become `_`.
pub fn tool_id(server: &str, tool: &str) -> String {
    let mut id = format!("mcp__{server}__{}", clean(tool));
    id.truncate(MAX_ID);
    id
}

/// The ids of tools, given as (server, tool) in the order they are listed -- the catalogue
/// id, the `state.json` entry, and the name the model calls the tool by. With the prefix
/// off a tool is called by its own name, unless that is taken (by a tool of `tools.toml`, by
/// one of the agent's own, or by an earlier MCP tool): then it keeps the prefix, so nothing
/// is ever shadowed. `None` marks a tool whose id repeats an earlier one and is left out.
pub fn assign_ids(
    prefix: bool,
    taken: &std::collections::BTreeSet<String>,
    tools: &[(&str, &str)],
) -> Vec<Option<String>> {
    let mut used: std::collections::BTreeSet<String> = taken.clone();
    used.extend(RESERVED.iter().map(|s| s.to_string()));
    tools
        .iter()
        .map(|(server, tool)| {
            let mut id = tool_id(server, tool);
            if !prefix {
                let mut plain = clean(tool);
                plain.truncate(MAX_ID);
                if !plain.is_empty() && !used.contains(&plain) {
                    id = plain;
                }
            }
            used.insert(id.clone()).then_some(id)
        })
        .collect()
}

// -- the cache ------------------------------------------------------------------------------

/// What the last attempt to reach a server came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Reach {
    Connected,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedTool {
    /// The name the server knows it by.
    pub name: String,
    /// The first line of its description.
    pub desc: String,
    /// What the definition costs the context while the tool is active: an estimate.
    pub tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedServer {
    pub reach: Reach,
    pub error: Option<String>,
    pub tools: Vec<CachedTool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cache {
    pub servers: BTreeMap<String, CachedServer>,
}

impl Cache {
    pub fn path(paths: &Paths) -> std::path::PathBuf {
        paths.live("mcp-cache.json")
    }

    /// Forgiving: a cache that cannot be read is an empty one -- it is rebuilt by connecting.
    pub fn load(paths: &Paths) -> Cache {
        let Ok(text) = std::fs::read_to_string(Cache::path(paths)) else {
            return Cache::default();
        };
        let Ok(doc) = serde_json::from_str::<Value>(&text) else {
            return Cache::default();
        };
        let mut servers = BTreeMap::new();
        for (name, s) in doc
            .get("servers")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let reach = match s.get("reach").and_then(Value::as_str) {
                Some("connected") => Reach::Connected,
                Some("failed") => Reach::Failed,
                _ => continue,
            };
            let tools = s
                .get("tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|t| {
                    Some(CachedTool {
                        name: t.get("name")?.as_str()?.to_string(),
                        desc: t
                            .get("desc")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        tokens: t.get("tokens").and_then(Value::as_u64).unwrap_or(0),
                    })
                })
                .collect();
            servers.insert(
                name.clone(),
                CachedServer {
                    reach,
                    error: s.get("error").and_then(Value::as_str).map(str::to_string),
                    tools,
                },
            );
        }
        Cache { servers }
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let mut servers = Map::new();
        for (name, s) in &self.servers {
            let mut m = Map::new();
            m.insert(
                "reach".into(),
                match s.reach {
                    Reach::Connected => "connected",
                    Reach::Failed => "failed",
                }
                .into(),
            );
            if let Some(e) = &s.error {
                m.insert("error".into(), e.clone().into());
            }
            let tools: Vec<Value> = s
                .tools
                .iter()
                .map(|t| {
                    let mut o = Map::new();
                    o.insert("name".into(), t.name.clone().into());
                    o.insert("desc".into(), t.desc.clone().into());
                    o.insert("tokens".into(), t.tokens.into());
                    Value::Object(o)
                })
                .collect();
            m.insert("tools".into(), tools.into());
            servers.insert(name.clone(), Value::Object(m));
        }
        let mut doc = Map::new();
        doc.insert("version".into(), 1.into());
        doc.insert("servers".into(), Value::Object(servers));
        write_json_atomic(&Cache::path(paths), &Value::Object(doc))
    }
}

// -- into the catalogue ---------------------------------------------------------------------

/// The catalogue entries of every enabled server's cached tools, servers in the order the
/// file lists them and tools in the order the server gave them: fixed, so nothing reorders.
/// A server that was never reached has no tools to list yet. `taken` is every id already in
/// the catalogue (see [`assign_ids`]).
pub fn entries(
    config: &Config,
    cache: &Cache,
    taken: &std::collections::BTreeSet<String>,
) -> Vec<Tool> {
    let listed: Vec<(&Server, &CachedServer, &CachedTool)> = config
        .servers
        .iter()
        .filter(|s| s.enabled)
        .filter_map(|s| cache.servers.get(&s.name).map(|c| (s, c)))
        .flat_map(|(s, c)| c.tools.iter().map(move |t| (s, c, t)))
        .collect();
    let pairs: Vec<(&str, &str)> = listed
        .iter()
        .map(|(s, _, t)| (s.name.as_str(), t.name.as_str()))
        .collect();
    let ids = assign_ids(config.prefix, taken, &pairs);
    let mut out: Vec<Tool> = Vec::new();
    for ((server, found, t), id) in listed.into_iter().zip(ids) {
        let Some(id) = id else { continue };
        let mut tags = vec![TAG.to_string(), server.name.clone()];
        for t in &server.tags {
            if !tags.contains(t) {
                tags.push(t.clone());
            }
        }
        out.push(Tool {
            id: id.clone(),
            name: t.name.clone(),
            desc: t.desc.clone(),
            invoke: id,
            detect_kind: DetectKind::Mcp,
            detect_value: server.name.clone(),
            tags,
            version_argv: None,
            source: None,
            service_probe: None,
            service_label: None,
            service_count: None,
            skill: None,
            install: OrderedMap::default(),
            mcp: Some(McpRef {
                server: server.name.clone(),
                tool: t.name.clone(),
                tokens: t.tokens,
                up: found.reach == Reach::Connected,
            }),
        });
    }
    out
}

/// The toolset of each enabled server that has tools to list: `mcp-<server>`, every tool of
/// that server at once. It selects by the tags `mcp` and the server's name, so it follows the
/// server's tools as they change; a toolset of the same id in `toolsets.toml` takes its place.
pub fn toolsets(config: &Config, cache: &Cache) -> Vec<crate::catalogue::Toolset> {
    config
        .servers
        .iter()
        .filter(|s| s.enabled)
        .filter_map(|server| {
            let found = cache
                .servers
                .get(&server.name)
                .filter(|c| !c.tools.is_empty())?;
            let tokens: u64 = found.tools.iter().map(|t| t.tokens).sum();
            Some(crate::catalogue::Toolset {
                id: format!("mcp-{}", server.name),
                desc: format!(
                    "every tool of the MCP server {} (about {tokens} tokens in each request)",
                    server.name
                ),
                tools: vec![],
                tags: vec![TAG.to_string(), server.name.clone()],
                everything: false,
            })
        })
        .collect()
}
