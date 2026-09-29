//! `tools.toml` and `toolsets.toml`, validated into types.
//!
//! Validation messages are user-facing (`reactor doctor` prints them), so they
//! name the file and the table the way the Python loader did.

use std::collections::BTreeMap;

use serde::Serialize;
use toml::{Table, Value};

use crate::err;
use crate::error::Result;
use crate::json::io_reason;
use crate::paths::Paths;

pub const DEFAULT_TIMEOUT: f64 = 5.0;
pub const DEFAULT_DETECT_TTL: i64 = 300;
pub const DEFAULT_SERVICE_TTL: i64 = 30;
pub const DEFAULT_PYTHON: &str = "python3";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectKind {
    Binary,
    PythonModule,
}

impl DetectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DetectKind::Binary => "binary",
            DetectKind::PythonModule => "python_module",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SkillSpec {
    pub source: Option<String>,
    pub path: Option<String>,
    pub git_ref: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ServiceCount {
    pub pattern: Option<String>,
    pub noun: Option<String>,
}

/// Insertion-ordered string map. Install recipes are read in declaration order
/// (it breaks ties between equally-preferred managers) and serialized in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrderedMap<V>(pub Vec<(String, V)>);

impl<V> OrderedMap<V> {
    pub fn get(&self, key: &str) -> Option<&V> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.0.iter().map(|(k, _)| k)
    }
    pub fn iter(&self) -> impl Iterator<Item = &(String, V)> {
        self.0.iter()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn insert(&mut self, key: impl Into<String>, value: V) {
        let key = key.into();
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key, value)),
        }
    }
}

impl<V: Serialize> Serialize for OrderedMap<V> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_map(self.0.iter().map(|(k, v)| (k, v)))
    }
}

#[derive(Debug, Clone)]
pub struct Tool {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub invoke: String,
    pub detect_kind: DetectKind,
    pub detect_value: String,
    pub tags: Vec<String>,
    pub version_argv: Option<Vec<String>>,
    pub source: Option<String>,
    pub service_probe: Option<Vec<String>>,
    pub service_label: Option<String>,
    pub service_count: Option<ServiceCount>,
    pub skill: Option<SkillSpec>,
    pub install: OrderedMap<String>,
}

impl Tool {
    /// A minimal tool, for tests and callers that build one by hand.
    pub fn simple(id: &str, invoke: &str, kind: DetectKind, detect_value: &str) -> Self {
        Tool {
            id: id.into(),
            name: id.into(),
            desc: String::new(),
            invoke: invoke.into(),
            detect_kind: kind,
            detect_value: detect_value.into(),
            tags: vec![],
            version_argv: None,
            source: None,
            service_probe: None,
            service_label: None,
            service_count: None,
            skill: None,
            install: OrderedMap::default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Toolset {
    pub id: String,
    pub desc: String,
    pub tools: Vec<String>,
    pub tags: Vec<String>,
    pub everything: bool,
}

#[derive(Debug, Clone)]
pub struct Manager {
    pub id: String,
    pub binary: String,
    pub os: Option<String>,
    pub sudo: bool,
}

#[derive(Debug, Clone)]
pub struct Catalogue {
    /// Where it was read from, for messages.
    pub path: String,
    /// True when falling back to the shipped copy.
    pub shipped: bool,
    /// Declaration order — fixed, so the registry never reorders (ADR-0006).
    pub tools: Vec<Tool>,
    pub managers: BTreeMap<String, Manager>,
    pub prefer: Vec<String>,
    pub timeout: f64,
    pub detect_ttl: i64,
    pub service_ttl: i64,
    pub python: String,
    /// Changes when the file changes; invalidates the probe cache.
    pub stamp: String,
}

impl Catalogue {
    pub fn get(&self, id: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.id == id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    pub fn ids(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.id.clone()).collect()
    }

    /// Declaration order. Fixed, so the registry never reorders (ADR-0006).
    pub fn order(&self) -> Vec<String> {
        self.ids()
    }

    pub fn position(&self, id: &str) -> Option<usize> {
        self.tools.iter().position(|t| t.id == id)
    }
}

struct Source {
    text: String,
    path: String,
    shipped: bool,
    stamp: String,
}

fn source_of(paths: &Paths, name: &str) -> Result<Source> {
    let live = paths.live(name);
    if live.exists() {
        let text = std::fs::read_to_string(&live)
            .map_err(|e| err!("{}: {}", live.display(), io_reason(&e)))?;
        return Ok(Source {
            text,
            path: live.display().to_string(),
            shipped: false,
            stamp: stamp_of(&live),
        });
    }
    let bytes = paths.shipped_bytes(name).map_err(|_| {
        err!("{}: not found -- run `reactor setup` first", live.display())
    })?;
    let text = String::from_utf8(bytes).map_err(|e| err!("{}: {}", paths.shipped_display(name), e))?;
    let stamp = match &paths.shipped {
        crate::paths::Shipped::Dir(root) => stamp_of(&root.join(name)),
        crate::paths::Shipped::Embedded => format!("embedded:{}", text.len()),
    };
    Ok(Source { text, path: paths.shipped_display(name), shipped: true, stamp })
}

fn stamp_of(path: &std::path::Path) -> String {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(path) {
        Ok(m) => format!("{}:{}", m.mtime() as i128 * 1_000_000_000 + m.mtime_nsec() as i128, m.len()),
        Err(_) => "0:0".to_string(),
    }
}

fn parse(src: &Source) -> Result<Table> {
    src.text
        .parse::<Table>()
        .map_err(|e| err!("{}: {}", src.path, e.message()))
}

fn check_version(doc: &Table, path: &str) -> Result<()> {
    match doc.get("version") {
        Some(Value::Integer(1)) => Ok(()),
        Some(other) => Err(err!("{path}: unsupported version {other} (expected 1)")),
        None => Err(err!("{path}: unsupported version None (expected 1)")),
    }
}

fn str_list(raw: Option<&Value>, whence: &str) -> Result<Vec<String>> {
    match raw {
        None => Ok(vec![]),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| err!("{whence}: expected a list of strings")),
        Some(_) => Err(err!("{whence}: expected a list of strings")),
    }
}

fn table_of(v: Option<&Value>) -> Option<&Table> {
    v.and_then(Value::as_table)
}

fn number(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Float(f) => Some(*f),
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

pub fn load_catalogue(paths: &Paths) -> Result<Catalogue> {
    let src = source_of(paths, "tools.toml")?;
    let doc = parse(&src)?;
    let path = src.path.clone();
    check_version(&doc, &path)?;

    let platform = table_of(doc.get("platform"));
    let mut managers = BTreeMap::new();
    if let Some(mgrs) = platform.and_then(|p| table_of(p.get("manager"))) {
        for (mid, spec) in mgrs {
            let spec = spec
                .as_table()
                .ok_or_else(|| err!("{path}: [platform.manager.{mid}] must be a table"))?;
            managers.insert(
                mid.clone(),
                Manager {
                    id: mid.clone(),
                    binary: spec
                        .get("binary")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .unwrap_or(mid)
                        .to_string(),
                    os: spec.get("os").and_then(Value::as_str).map(str::to_string),
                    sudo: spec.get("sudo").and_then(Value::as_bool).unwrap_or(false),
                },
            );
        }
    }
    let prefer = str_list(
        platform.and_then(|p| p.get("prefer")),
        &format!("{path}: [platform].prefer"),
    )?;

    let probe = table_of(doc.get("probe"));
    let mut tools = Vec::new();
    if let Some(entries) = table_of(doc.get("tool")) {
        for (tid, spec) in entries {
            let whence = format!("{path}: [tool.{tid}]");
            let spec = spec.as_table().ok_or_else(|| err!("{whence} must be a table"))?;

            let detect = table_of(spec.get("detect"))
                .filter(|d| d.len() == 1)
                .ok_or_else(|| err!("{whence}.detect: expected exactly one of binary, python_module"))?;
            let (kind_name, value) = detect.iter().next().expect("length checked");
            let kind = match kind_name.as_str() {
                "binary" => DetectKind::Binary,
                "python_module" => DetectKind::PythonModule,
                other => return Err(err!("{whence}.detect: unknown kind '{other}'")),
            };
            let detect_value = value
                .as_str()
                .ok_or_else(|| err!("{whence}.detect.{kind_name}: must be a string"))?
                .to_string();

            let required = |field: &str| -> Result<String> {
                spec.get(field)
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| err!("{whence}.{field}: required, must be a string"))
            };
            let name = required("name")?;
            let desc = required("desc")?;
            let invoke = required("invoke")?;

            let mut install = OrderedMap::default();
            if let Some(recipes) = table_of(spec.get("install")) {
                for (k, v) in recipes {
                    let cmd = v
                        .as_str()
                        .ok_or_else(|| err!("{whence}.install: every recipe must be a string"))?;
                    install.0.push((k.clone(), cmd.to_string()));
                }
            }

            let source = match spec.get("source") {
                None => None,
                Some(v) => {
                    // ADR-0028: provenance is https or nothing -- the field
                    // exists so a manual install can be checked against where
                    // the tool really comes from, and an insecure URL defeats
                    // that purpose.
                    match v.as_str() {
                        Some(s) if s.starts_with("https://") => Some(s.to_string()),
                        _ => {
                            return Err(err!(
                                "{whence}.source: must be an https:// URL (the tool's true \
                                 upstream) -- see ADR-0028"
                            ));
                        }
                    }
                }
            };

            let service = table_of(spec.get("service"));
            let service_probe = str_list(
                service.and_then(|s| s.get("probe")),
                &format!("{whence}.service.probe"),
            )?;
            let version_argv = str_list(spec.get("version"), &format!("{whence}.version"))?;

            tools.push(Tool {
                id: tid.clone(),
                name,
                desc,
                invoke,
                detect_kind: kind,
                detect_value,
                tags: str_list(spec.get("tags"), &format!("{whence}.tags"))?,
                version_argv: (!version_argv.is_empty()).then_some(version_argv),
                source,
                service_probe: (!service_probe.is_empty()).then_some(service_probe),
                service_label: service
                    .and_then(|s| s.get("label"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                service_count: table_of(service.and_then(|s| s.get("count"))).map(|c| ServiceCount {
                    pattern: c.get("pattern").and_then(Value::as_str).map(str::to_string),
                    noun: c.get("noun").and_then(Value::as_str).map(str::to_string),
                }),
                skill: table_of(spec.get("skill")).map(|s| SkillSpec {
                    source: s.get("source").and_then(Value::as_str).map(str::to_string),
                    path: s.get("path").and_then(Value::as_str).map(str::to_string),
                    git_ref: s.get("ref").and_then(Value::as_str).map(str::to_string),
                }),
                install,
            });
        }
    }

    Ok(Catalogue {
        path,
        shipped: src.shipped,
        tools,
        managers,
        prefer,
        timeout: number(probe.and_then(|p| p.get("timeout"))).unwrap_or(DEFAULT_TIMEOUT),
        detect_ttl: number(probe.and_then(|p| p.get("detect_ttl")))
            .map(|n| n as i64)
            .unwrap_or(DEFAULT_DETECT_TTL),
        service_ttl: number(probe.and_then(|p| p.get("service_ttl")))
            .map(|n| n as i64)
            .unwrap_or(DEFAULT_SERVICE_TTL),
        python: probe
            .and_then(|p| p.get("python"))
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_PYTHON)
            .to_string(),
        stamp: src.stamp,
    })
}

pub fn load_toolsets(paths: &Paths) -> Result<Vec<Toolset>> {
    let src = source_of(paths, "toolsets.toml")?;
    let doc = parse(&src)?;
    check_version(&doc, &src.path)?;
    let mut out = Vec::new();
    if let Some(sets) = table_of(doc.get("toolset")) {
        for (sid, spec) in sets {
            let whence = format!("{}: [toolset.{sid}]", src.path);
            let spec = spec.as_table().ok_or_else(|| err!("{whence} must be a table"))?;
            out.push(Toolset {
                id: sid.clone(),
                desc: spec.get("desc").and_then(Value::as_str).unwrap_or("").to_string(),
                tools: str_list(spec.get("tools"), &format!("{whence}.tools"))?,
                tags: str_list(spec.get("tags"), &format!("{whence}.tags"))?,
                everything: spec.get("all").and_then(Value::as_bool).unwrap_or(false),
            });
        }
    }
    Ok(out)
}

/// Lookup helper for the `Vec<Toolset>` the loader returns.
pub fn find_toolset<'a>(sets: &'a [Toolset], id: &str) -> Option<&'a Toolset> {
    sets.iter().find(|t| t.id == id)
}
