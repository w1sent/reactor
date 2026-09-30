//! Detection and service probing, with a cache the CLI, agent and GUI share (ADR-0014).
//!
//! Results carry no timing information: rendering must be byte-stable across
//! turns when nothing about the machine changed (ADR-0006). A probe that times
//! out is *unknown*, never *absent*.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};

use crate::catalogue::{Catalogue, DetectKind, Tool};
use crate::json::write_json_atomic;
use crate::paths::Paths;
use crate::util::{first_line, now_secs_f64, par_map, run, which};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Present,
    Absent,
    Unknown,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Present => "present",
            Status::Absent => "absent",
            Status::Unknown => "unknown",
        }
    }
    fn parse(s: &str) -> Status {
        match s {
            "present" => Status::Present,
            "absent" => Status::Absent,
            _ => Status::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceState {
    Up,
    Down,
    Unknown,
}

impl ServiceState {
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceState::Up => "up",
            ServiceState::Down => "down",
            ServiceState::Unknown => "unknown",
        }
    }
    fn parse(s: &str) -> ServiceState {
        match s {
            "up" => ServiceState::Up,
            "down" => ServiceState::Down,
            _ => ServiceState::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceInfo {
    pub label: Option<String>,
    pub state: ServiceState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub status: Status,
    pub path: Option<String>,
    pub version: Option<String>,
    pub service: Option<ServiceInfo>,
}

impl ProbeResult {
    pub fn unknown() -> Self {
        Self { status: Status::Unknown, path: None, version: None, service: None }
    }
}

/// One subprocess answers every python_module probe at once, and find_spec is
/// used rather than import so that probing angr does not pay for loading angr.
/// This is a *question put to the machine's Python* about which libraries it
/// has — REactor itself does not run on, or need, Python. With no interpreter
/// the answer is `unknown`, not `absent`.
const FIND_SPEC: &str = "\
import importlib.util as u, json, sys
out = {}
for m in sys.argv[1:]:
    try:
        out[m] = u.find_spec(m) is not None
    except Exception:
        out[m] = False
print(json.dumps(out))
";

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

/// Probe results on disk. Best-effort: an unwritable cache is not an error.
pub struct Cache {
    path: PathBuf,
    config_dir: PathBuf,
    stamp: String,
    detect: BTreeMap<String, Value>,
    service: BTreeMap<String, Value>,
}

impl Cache {
    pub fn load(paths: &Paths, cat: &Catalogue) -> Self {
        let mut cache = Cache {
            path: paths.cache_file(),
            config_dir: paths.config_dir.clone(),
            stamp: cat.stamp.clone(),
            detect: BTreeMap::new(),
            service: BTreeMap::new(),
        };
        let Ok(text) = std::fs::read_to_string(&cache.path) else { return cache };
        let Ok(doc) = serde_json::from_str::<Value>(&text) else { return cache };
        if doc.get("version") == Some(&json!(1)) && doc.get("stamp") == Some(&json!(cat.stamp)) {
            let bucket = |name: &str| -> BTreeMap<String, Value> {
                doc.get(name)
                    .and_then(Value::as_object)
                    .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default()
            };
            cache.detect = bucket("detect");
            cache.service = bucket("service");
        }
        cache
    }

    fn bucket(&self, name: &str) -> &BTreeMap<String, Value> {
        if name == "detect" { &self.detect } else { &self.service }
    }

    fn get(&self, bucket: &str, key: &str, ttl: i64, now: f64) -> Option<&Value> {
        let entry = self.bucket(bucket).get(key).filter(|e| e.as_object().is_some_and(|o| !o.is_empty()))?;
        let ts = entry.get("ts").and_then(Value::as_f64).unwrap_or(0.0);
        if ttl >= 0 && now - ts > ttl as f64 {
            return None;
        }
        Some(entry)
    }

    fn stale(&self, bucket: &str, key: &str) -> Option<&Value> {
        self.bucket(bucket).get(key)
    }

    fn put(&mut self, bucket: &str, key: &str, mut value: Value, now: f64) {
        value["ts"] = json!(now);
        let map = if bucket == "detect" { &mut self.detect } else { &mut self.service };
        map.insert(key.to_string(), value);
    }

    pub fn clear(&mut self) {
        self.detect.clear();
        self.service.clear();
    }

    pub fn save(&self) {
        // Not before setup: reading the shipped fallback should not conjure the
        // config dir out of a plain `reactor doctor`.
        if !self.config_dir.is_dir() {
            return;
        }
        let doc = json!({
            "version": 1,
            "stamp": self.stamp,
            "detect": self.detect,
            "service": self.service,
        });
        let _ = write_json_atomic(&self.path, &doc);
    }
}

// ---------------------------------------------------------------------------
// Probing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct ProbeOpts {
    /// Ignore the cache and probe everything.
    pub refresh: bool,
    /// Never probe; report cached values, unknown where absent.
    pub cached_only: bool,
    /// Also probe service state for tools that declare one.
    pub services: bool,
}

impl ProbeOpts {
    pub fn new() -> Self {
        Self { services: true, ..Self::default() }
    }
}

/// Detection (+ version, + service) for `ids`, cache-aware.
pub fn probe(paths: &Paths, cat: &Catalogue, ids: &[String], opts: ProbeOpts) -> HashMap<String, ProbeResult> {
    let now = now_secs_f64();
    let mut cache = Cache::load(paths, cat);
    if opts.refresh {
        cache.clear();
    }

    let tools: Vec<&Tool> = ids.iter().filter_map(|i| cat.get(i)).collect();
    let mut results: HashMap<String, ProbeResult> = HashMap::new();
    let mut detect_todo: Vec<&Tool> = Vec::new();

    for t in &tools {
        let mut hit = if opts.refresh { None } else { cache.get("detect", &t.id, cat.detect_ttl, now) };
        if hit.is_none() && opts.cached_only {
            hit = cache.stale("detect", &t.id);
        }
        if let Some(hit) = hit {
            let text = |k: &str| hit.get(k).and_then(Value::as_str).map(str::to_string);
            results.insert(
                t.id.clone(),
                ProbeResult {
                    status: hit.get("status").and_then(Value::as_str).map(Status::parse).unwrap_or(Status::Unknown),
                    path: text("path"),
                    version: text("version"),
                    service: None,
                },
            );
        } else if opts.cached_only {
            results.insert(t.id.clone(), ProbeResult::unknown());
        } else {
            detect_todo.push(t);
        }
    }

    if !detect_todo.is_empty() {
        probe_detect(cat, &detect_todo, &mut results, &mut cache, now);
    }

    let mut service_todo: Vec<&Tool> = Vec::new();
    for t in &tools {
        let Some(_) = t.service_probe.as_ref().filter(|_| opts.services) else { continue };
        let unknown = || ServiceInfo { label: t.service_label.clone(), state: ServiceState::Unknown, detail: None };
        if results[&t.id].status != Status::Present {
            results.get_mut(&t.id).unwrap().service = Some(unknown());
            continue;
        }
        let mut hit = if opts.refresh { None } else { cache.get("service", &t.id, cat.service_ttl, now) };
        if hit.is_none() && opts.cached_only {
            hit = cache.stale("service", &t.id);
        }
        if let Some(hit) = hit {
            let info = ServiceInfo {
                label: t.service_label.clone(),
                state: hit.get("state").and_then(Value::as_str).map(ServiceState::parse).unwrap_or(ServiceState::Unknown),
                detail: hit.get("detail").and_then(Value::as_str).map(str::to_string),
            };
            results.get_mut(&t.id).unwrap().service = Some(info);
        } else if opts.cached_only {
            results.get_mut(&t.id).unwrap().service = Some(unknown());
        } else {
            service_todo.push(t);
        }
    }

    if !service_todo.is_empty() {
        probe_services(cat, &service_todo, &mut results, &mut cache, now);
    }

    cache.save();
    results
}

fn timeout_of(cat: &Catalogue) -> Duration {
    Duration::from_secs_f64(cat.timeout.max(0.0))
}

fn probe_detect(
    cat: &Catalogue,
    tools: &[&Tool],
    results: &mut HashMap<String, ProbeResult>,
    cache: &mut Cache,
    now: f64,
) {
    let modules: Vec<&&Tool> = tools.iter().filter(|t| t.detect_kind == DetectKind::PythonModule).collect();
    let mut found: HashMap<String, bool> = HashMap::new();
    if !modules.is_empty() {
        let mut argv = vec![cat.python.clone(), "-c".to_string(), FIND_SPEC.to_string()];
        argv.extend(modules.iter().map(|t| t.detect_value.clone()));
        let r = run(&argv, timeout_of(cat));
        if r.ok()
            && let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&first_line(&r.output))
        {
            found = m.into_iter().map(|(k, v)| (k, v.as_bool().unwrap_or(false))).collect();
        }
        // A timeout leaves `found` empty → unknown below.
    }
    let module_probe_failed = !modules.is_empty() && found.is_empty();

    for t in tools {
        let entry = match t.detect_kind {
            DetectKind::Binary => {
                let where_ = which(&t.detect_value).map(|p| p.display().to_string());
                ProbeResult {
                    status: if where_.is_some() { Status::Present } else { Status::Absent },
                    path: where_,
                    version: None,
                    service: None,
                }
            }
            DetectKind::PythonModule => {
                if module_probe_failed {
                    ProbeResult::unknown()
                } else {
                    let here = found.get(&t.detect_value).copied().unwrap_or(false);
                    ProbeResult {
                        status: if here { Status::Present } else { Status::Absent },
                        path: None,
                        version: None,
                        service: None,
                    }
                }
            }
        };
        results.insert(t.id.clone(), entry);
    }

    // Versions only for what is actually here, in parallel: a dozen `--version`
    // calls in series is a visible pause at session start.
    let versioned: Vec<&&Tool> = tools
        .iter()
        .filter(|t| t.version_argv.is_some() && results[&t.id].status == Status::Present)
        .collect();
    if !versioned.is_empty() {
        let outs = par_map(&versioned, 8, |t| run(t.version_argv.as_ref().unwrap(), timeout_of(cat)));
        for (t, r) in versioned.iter().zip(outs) {
            if r.ok() {
                results.get_mut(&t.id).unwrap().version = version_of(&first_line(&r.output));
            }
        }
    }

    for t in tools {
        let r = &results[&t.id];
        cache.put(
            "detect",
            &t.id,
            json!({"status": r.status.as_str(), "path": r.path, "version": r.version}),
            now,
        );
    }
}

fn version_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b\d+(?:\.\d+)+(?:[-+.\w]*)?").unwrap())
}

/// Pull a version number out of a --version line, so the registry stays short
/// and, more importantly, stable: full banners carry build dates.
pub fn version_of(line: &str) -> Option<String> {
    if line.is_empty() {
        return None;
    }
    match version_re().find(line) {
        Some(m) => Some(m.as_str().to_string()),
        None => Some(line.chars().take(32).collect()),
    }
}

fn probe_services(
    cat: &Catalogue,
    tools: &[&Tool],
    results: &mut HashMap<String, ProbeResult>,
    cache: &mut Cache,
    now: f64,
) {
    let outs = par_map(tools, 8, |t| run(t.service_probe.as_ref().unwrap(), timeout_of(cat)));
    for (t, r) in tools.iter().zip(outs) {
        let (state, detail) = if !r.completed {
            (ServiceState::Unknown, None)
        } else if r.code == Some(0) {
            (ServiceState::Up, service_detail(t, &r.output))
        } else {
            (ServiceState::Down, None)
        };
        cache.put("service", &t.id, json!({"state": state.as_str(), "detail": detail}), now);
        results.get_mut(&t.id).unwrap().service =
            Some(ServiceInfo { label: t.service_label.clone(), state, detail });
    }
}

/// Deliberately coarse: a count, or nothing. Anything finer would rewrite the
/// system prompt every time a service's output jittered (ADR-0006).
pub fn service_detail(tool: &Tool, out: &str) -> Option<String> {
    let spec = tool.service_count.as_ref()?;
    let rx = Regex::new(spec.pattern.as_ref()?).ok()?;
    let n = out.lines().filter(|l| rx.is_match(l)).count();
    let noun = spec.noun.as_deref().unwrap_or("item");
    Some(if n == 1 { format!("{n} {noun}") } else { format!("{n} {noun}s") })
}
