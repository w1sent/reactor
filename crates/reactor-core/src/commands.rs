//! The read/query/activation half of the CLI surface, as library calls.
//!
//! Each function does what its subcommand does and returns the report —
//! payload plus human text — leaving printing to the caller. The GUI links
//! these directly; the binary prints them.

use std::collections::HashMap;

use serde::Serialize;

use crate::catalogue::{Catalogue, Toolset, load_catalogue, load_toolsets};
use crate::config::{ConfigDrift, config_drift};
use crate::err;
use crate::error::Result;
use crate::model::{ToolEntry, describe};
use crate::paths::{CONFIG_FILES, Paths};
use crate::probe::{ProbeOpts, ProbeResult, ServiceState, Status, probe};
use crate::recipes::{available_managers, sys_platform};
use crate::render::{render_registry, render_tool_list, render_tool_show, service_glyph, table};
use crate::report::{Done, Report};
use crate::skills::{FetchResult, SkillStatus, fetch_skill, skill_remote_head, skill_status};
use crate::state::{Scope, State, StateDoc, active_ids, load_state, toggle, toolset_members};

/// Ids in catalogue order; unknown ones are an error. Empty means "all".
pub fn select(cat: &Catalogue, ids: &[String]) -> Result<Vec<String>> {
    if ids.is_empty() {
        return Ok(cat.order());
    }
    let unknown: Vec<&str> = ids.iter().filter(|i| !cat.contains(i)).map(String::as_str).collect();
    if !unknown.is_empty() {
        return Err(err!("unknown tool(s): {}", unknown.join(", ")));
    }
    Ok(cat.order().into_iter().filter(|i| ids.contains(i)).collect())
}

struct Loaded {
    cat: Catalogue,
    toolsets: Vec<Toolset>,
    state: State,
}

fn load(paths: &Paths) -> Result<Loaded> {
    Ok(Loaded { cat: load_catalogue(paths)?, toolsets: load_toolsets(paths)?, state: load_state(paths)? })
}

// ---------------------------------------------------------------------------
// registry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct ProbeFlags {
    pub refresh: bool,
    pub cached: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistrySummary {
    pub present: usize,
    pub absent: usize,
    pub unknown: usize,
    pub catalogued: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistryReport {
    pub block: String,
    pub tools: Vec<ToolEntry>,
    pub summary: RegistrySummary,
    #[serde(rename = "skillPaths")]
    pub skill_paths: Vec<String>,
}

impl Report for RegistryReport {
    fn human(&self) -> String {
        self.block.clone()
    }
}

pub fn registry(paths: &Paths, flags: ProbeFlags) -> Result<Done<RegistryReport>> {
    let l = load(paths)?;
    let active = active_ids(&l.cat, &l.toolsets, &l.state);
    let ids: Vec<String> = l.cat.order().into_iter().filter(|i| active.contains(i)).collect();
    let results = probe(paths, &l.cat, &ids, ProbeOpts { refresh: flags.refresh, cached_only: flags.cached, services: true });
    let entries = describe(paths, &l.cat, &l.toolsets, &l.state, &ids, &results, false);
    let count = |s: Status| entries.iter().filter(|e| e.status == s).count();
    let mut skill_paths: Vec<String> = entries
        .iter()
        .filter(|e| e.active && e.status == Status::Present && e.skill.as_ref().is_some_and(|s| s.fetched))
        .map(|e| paths.skills_dir().join(&e.id).display().to_string())
        .collect();
    skill_paths.sort();
    Ok(Done::ok(RegistryReport {
        block: render_registry(&entries),
        summary: RegistrySummary {
            present: count(Status::Present),
            absent: count(Status::Absent),
            unknown: count(Status::Unknown),
            catalogued: l.cat.tools.len(),
        },
        tools: entries,
        skill_paths,
    }))
}

// ---------------------------------------------------------------------------
// doctor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct DoctorPlatform {
    pub sys_platform: &'static str,
    /// REactor's own version. (This slot held the CLI's Python version before
    /// the port; nothing read it, and there is no interpreter to report now.)
    pub reactor: &'static str,
    pub managers: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorConfig {
    pub dir: String,
    pub catalogue: String,
    pub shipped_fallback: bool,
    pub state_scope: Scope,
    pub active_toolsets: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Problem {
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toolset: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StaleSkill {
    pub tool: String,
    pub have: String,
    pub remote: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub platform: DoctorPlatform,
    pub config: DoctorConfig,
    pub tools: Vec<ToolEntry>,
    pub skills_stale: Vec<StaleSkill>,
    pub problems: Vec<Problem>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DoctorFlags {
    pub cached: bool,
    pub check_skills: bool,
}

pub fn doctor(paths: &Paths, flags: DoctorFlags) -> Result<Done<DoctorReport>> {
    let l = load(paths)?;
    let ids = l.cat.order();
    let results = probe(
        paths,
        &l.cat,
        &ids,
        ProbeOpts { refresh: !flags.cached, cached_only: flags.cached, services: true },
    );
    let entries = describe(paths, &l.cat, &l.toolsets, &l.state, &ids, &results, true);
    let managers: Vec<String> = available_managers(&l.cat).into_keys().collect();

    let mut problems: Vec<Problem> = Vec::new();
    if l.cat.shipped {
        problems.push(Problem {
            kind: "not-installed",
            message: format!(
                "reading the shipped catalogue at {}; run `reactor setup` to seed {}",
                l.cat.path,
                paths.config_dir.display()
            ),
            ..Default::default()
        });
    }
    for name in CONFIG_FILES {
        if config_drift(paths, name) == ConfigDrift::Differs {
            problems.push(Problem {
                kind: "config-drift",
                file: Some(name.to_string()),
                message: format!("{name} differs from the shipped copy -- `reactor diff-config`"),
                ..Default::default()
            });
        }
    }
    for e in &entries {
        if e.skill.as_ref().is_some_and(|s| !s.fetched) {
            problems.push(Problem {
                kind: "skill-missing",
                tool: Some(e.id.clone()),
                message: format!("{}: configured skill not fetched -- `reactor skills fetch {}`", e.id, e.id),
                ..Default::default()
            });
        }
    }
    // Tags intersect (ADR-0013), so an over-specified list selects nothing at
    // all. Activating such a toolset looks like activating nothing, which is a
    // confusing way to find out you have a typo.
    for ts in &l.toolsets {
        if toolset_members(ts, &l.cat).is_empty() {
            problems.push(Problem {
                kind: "toolset-empty",
                toolset: Some(ts.id.clone()),
                message: format!("toolset {}: selects no tool -- `reactor toolsets show {}`", ts.id, ts.id),
                ..Default::default()
            });
        }
    }

    let mut stale = Vec::new();
    if flags.check_skills {
        for e in &entries {
            let Some(sk) = e.skill.as_ref() else { continue };
            let Some(have) = sk.commit.as_deref().filter(|_| sk.fetched) else { continue };
            let Some(head) = l.cat.get(&e.id).and_then(skill_remote_head) else { continue };
            if head != have {
                let cut = |s: &str| s.chars().take(12).collect::<String>();
                stale.push(StaleSkill { tool: e.id.clone(), have: cut(have), remote: cut(&head) });
                problems.push(Problem {
                    kind: "skill-stale",
                    tool: Some(e.id.clone()),
                    message: format!(
                        "{}: fetched skill is behind {}",
                        e.id,
                        sk.git_ref.as_deref().unwrap_or("HEAD")
                    ),
                    ..Default::default()
                });
            }
        }
    }

    Ok(Done::ok(DoctorReport {
        platform: DoctorPlatform { sys_platform: sys_platform(), reactor: env!("CARGO_PKG_VERSION"), managers },
        config: DoctorConfig {
            dir: paths.config_dir.display().to_string(),
            catalogue: l.cat.path.clone(),
            shipped_fallback: l.cat.shipped,
            state_scope: l.state.scope,
            active_toolsets: if l.state.toolsets.is_empty() { vec!["(all)".into()] } else { l.state.toolsets.clone() },
        },
        tools: entries,
        skills_stale: stale,
        problems,
    }))
}

impl Report for DoctorReport {
    fn human(&self) -> String {
        let p = &self.platform;
        let c = &self.config;
        let mut out = vec![
            format!("platform   {}", p.sys_platform),
            format!(
                "managers   {}",
                if p.managers.is_empty() { "none detected".to_string() } else { p.managers.join(", ") }
            ),
            format!("config     {}{}", c.catalogue, if c.shipped_fallback { "  (SHIPPED FALLBACK)" } else { "" }),
            format!("toolsets   {}  [{}]", c.active_toolsets.join(", "), c.state_scope.as_str()),
            String::new(),
        ];
        let of = |s: Status| self.tools.iter().filter(move |e| e.status == s).collect::<Vec<_>>();
        let (present, absent, unknown) = (of(Status::Present), of(Status::Absent), of(Status::Unknown));

        out.push(format!("present ({})", present.len()));
        let rows: Vec<Vec<String>> = present
            .iter()
            .map(|e| vec![format!("  {}", e.id), e.version.clone().unwrap_or_default(), e.desc.clone()])
            .collect();
        out.push(if rows.is_empty() { "  none".to_string() } else { table(&rows) });
        if !unknown.is_empty() {
            out.push(String::new());
            out.push(format!("unknown ({}) -- probe timed out; not reported as absent", unknown.len()));
            out.push(table(&unknown.iter().map(|e| vec![format!("  {}", e.id), e.desc.clone()]).collect::<Vec<_>>()));
        }
        if !absent.is_empty() {
            out.push(String::new());
            out.push(format!("missing ({})", absent.len()));
            let rows: Vec<Vec<String>> = absent
                .iter()
                .map(|e| {
                    let inst = e.install.as_ref().expect("doctor describes with install");
                    let hint = match &inst.recommended {
                        Some(rec) => rec.text(),
                        // No runnable recipe here. Show a note, keyed, so it
                        // reads as "this is what works elsewhere" rather than
                        // as a command to run.
                        None => match inst.notes.iter().next() {
                            Some((k, v)) => format!("({k}) {v}"),
                            None => "no recipe for this machine".to_string(),
                        },
                    };
                    vec![format!("  {}", e.id), hint]
                })
                .collect();
            out.push(table(&rows));
        }
        if !self.problems.is_empty() {
            out.push(String::new());
            out.push("problems".to_string());
            out.extend(self.problems.iter().map(|pr| format!("  ! {}", pr.message)));
        }
        out.join("\n")
    }
}

// ---------------------------------------------------------------------------
// tools list / show, toggles
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ToolsListFlags {
    pub tags: Vec<String>,
    pub active: bool,
    pub present: bool,
    pub missing: bool,
    pub probe: ProbeFlags,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolsReport {
    pub tools: Vec<ToolEntry>,
}

impl Report for ToolsReport {
    fn human(&self) -> String {
        render_tool_list(&self.tools)
    }
}

pub fn tools_list(paths: &Paths, flags: &ToolsListFlags) -> Result<Done<ToolsReport>> {
    let l = load(paths)?;
    let mut ids = l.cat.order();
    if !flags.tags.is_empty() {
        // Repeating --tag narrows, the same way a toolset's tag list does
        // (ADR-0013). One spelling of "a list of tags", one meaning.
        ids.retain(|i| flags.tags.iter().all(|w| l.cat.get(i).unwrap().tags.contains(w)));
    }
    if flags.active {
        let active = active_ids(&l.cat, &l.toolsets, &l.state);
        ids.retain(|i| active.contains(i));
    }
    let results = probe(
        paths,
        &l.cat,
        &ids,
        ProbeOpts { refresh: flags.probe.refresh, cached_only: flags.probe.cached, services: false },
    );
    let mut entries = describe(paths, &l.cat, &l.toolsets, &l.state, &ids, &results, false);
    if flags.present {
        entries.retain(|e| e.status == Status::Present);
    } else if flags.missing {
        entries.retain(|e| e.status == Status::Absent);
    }
    Ok(Done::ok(ToolsReport { tools: entries }))
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolReport {
    pub tool: ToolEntry,
}

impl Report for ToolReport {
    fn human(&self) -> String {
        render_tool_show(&self.tool)
    }
}

pub fn tools_show(paths: &Paths, id: &str, flags: ProbeFlags) -> Result<Done<ToolReport>> {
    let l = load(paths)?;
    let ids = select(&l.cat, &[id.to_string()])?;
    let results = probe(
        paths,
        &l.cat,
        &ids,
        ProbeOpts { refresh: flags.refresh, cached_only: flags.cached, services: true },
    );
    let entry = describe(paths, &l.cat, &l.toolsets, &l.state, &ids, &results, true).remove(0);
    Ok(Done::ok(ToolReport { tool: entry }))
}

#[derive(Debug, Clone, Serialize)]
pub struct ToggleReport {
    pub state: StateDoc,
    pub active: Vec<String>,
    pub path: String,
    #[serde(skip)]
    verb: &'static str,
    #[serde(skip)]
    ids: Vec<String>,
}

impl Report for ToggleReport {
    fn human(&self) -> String {
        format!("{}: {}\n{} tool(s) active -- {}", self.verb, self.ids.join(", "), self.active.len(), self.path)
    }
}

pub fn set_activation(paths: &Paths, ids: &[String], tools: bool, on: Option<bool>) -> Result<Done<ToggleReport>> {
    let mut l = load(paths)?;
    toggle(&l.cat, &l.toolsets, &mut l.state, ids, tools, on)?;
    if let Some(session) = &paths.session_state {
        // Edits in a session go to the session, never to the machine: the override is
        // seeded from whatever state applied, then written here.
        l.state.path = session.clone();
        l.state.scope = Scope::Session;
    } else if l.state.scope == Scope::Default {
        l.state.path = paths.state_file();
    }
    l.state.save()?;
    let active: Vec<String> = active_ids(&l.cat, &l.toolsets, &l.state).into_iter().collect();
    let verb = match on {
        Some(true) => "enabled",
        Some(false) => "disabled",
        None => "reset",
    };
    Ok(Done::ok(ToggleReport {
        state: l.state.as_doc(),
        active,
        path: l.state.path.display().to_string(),
        verb,
        ids: ids.to_vec(),
    }))
}

// ---------------------------------------------------------------------------
// toolsets, state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ToolsetRow {
    pub id: String,
    pub desc: String,
    pub active: bool,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolsetsReport {
    pub toolsets: Vec<ToolsetRow>,
}

impl Report for ToolsetsReport {
    fn human(&self) -> String {
        table(
            &self
                .toolsets
                .iter()
                .map(|t| {
                    vec![
                        if t.active { "[x]".to_string() } else { "[ ]".to_string() },
                        t.id.clone(),
                        t.tools.len().to_string(),
                        t.desc.clone(),
                    ]
                })
                .collect::<Vec<_>>(),
        )
    }
}

pub fn toolsets_list(paths: &Paths) -> Result<Done<ToolsetsReport>> {
    let l = load(paths)?;
    let rows = l
        .toolsets
        .iter()
        .map(|ts| ToolsetRow {
            id: ts.id.clone(),
            desc: ts.desc.clone(),
            active: l.state.toolsets.contains(&ts.id),
            tools: toolset_members(ts, &l.cat),
        })
        .collect();
    Ok(Done::ok(ToolsetsReport { toolsets: rows }))
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolsetDetail {
    pub id: String,
    pub desc: String,
    pub tools: Vec<String>,
    pub tags: Vec<String>,
    pub active: bool,
    pub members: Vec<ToolEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolsetReport {
    pub toolset: ToolsetDetail,
}

impl Report for ToolsetReport {
    fn human(&self) -> String {
        format!("{} -- {}\n\n{}", self.toolset.id, self.toolset.desc, render_tool_list(&self.toolset.members))
    }
}

pub fn toolsets_show(paths: &Paths, id: &str) -> Result<Done<ToolsetReport>> {
    let l = load(paths)?;
    let ts = l.toolsets.iter().find(|t| t.id == id).ok_or_else(|| err!("unknown toolset: {id}"))?;
    let members = toolset_members(ts, &l.cat);
    let results = probe(paths, &l.cat, &members, ProbeOpts { cached_only: true, ..Default::default() });
    Ok(Done::ok(ToolsetReport {
        toolset: ToolsetDetail {
            id: ts.id.clone(),
            desc: ts.desc.clone(),
            tools: ts.tools.clone(),
            tags: ts.tags.clone(),
            active: l.state.toolsets.contains(&ts.id),
            members: describe(paths, &l.cat, &l.toolsets, &l.state, &members, &results, false),
        },
    }))
}

#[derive(Debug, Clone, Serialize)]
pub struct StateReport {
    pub state: StateDoc,
    pub scope: Scope,
    pub path: String,
    pub active: Vec<String>,
    #[serde(skip)]
    raw: State,
}

impl Report for StateReport {
    fn human(&self) -> String {
        let join = |v: &[String], empty: &str| if v.is_empty() { empty.to_string() } else { v.join(", ") };
        format!(
            "scope      {} ({})\ntoolsets   {}\nenabled    {}\ndisabled   {}\nactive     {} tool(s)",
            self.scope.as_str(),
            self.path,
            join(&self.raw.toolsets, "(none -- everything is active)"),
            join(&self.raw.enabled, "-"),
            join(&self.raw.disabled, "-"),
            self.active.len()
        )
    }
}

pub fn state(paths: &Paths) -> Result<Done<StateReport>> {
    let l = load(paths)?;
    let active = active_ids(&l.cat, &l.toolsets, &l.state).into_iter().collect();
    Ok(Done::ok(StateReport {
        state: l.state.as_doc(),
        scope: l.state.scope,
        path: l.state.path.display().to_string(),
        active,
        raw: l.state,
    }))
}

// ---------------------------------------------------------------------------
// skills
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct SkillRow {
    pub tool: String,
    #[serde(flatten)]
    pub status: SkillStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillsListReport {
    pub skills: Vec<SkillRow>,
}

impl Report for SkillsListReport {
    fn human(&self) -> String {
        let rows: Vec<Vec<String>> = self
            .skills
            .iter()
            .map(|s| {
                vec![
                    if s.status.fetched { "[+]".to_string() } else { "[-]".to_string() },
                    s.tool.clone(),
                    s.status.commit.as_deref().unwrap_or("").chars().take(12).collect(),
                    s.status.source.clone().unwrap_or_default(),
                ]
            })
            .collect();
        if rows.is_empty() { "no upstream skills configured".to_string() } else { table(&rows) }
    }
}

pub fn skills_list(paths: &Paths) -> Result<Done<SkillsListReport>> {
    let cat = load_catalogue(paths)?;
    let skills = cat
        .tools
        .iter()
        .filter_map(|t| skill_status(paths, t).map(|status| SkillRow { tool: t.id.clone(), status }))
        .collect();
    Ok(Done::ok(SkillsListReport { skills }))
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillBody {
    pub tool: String,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillShowReport {
    pub skill: SkillBody,
}

impl Report for SkillShowReport {
    fn human(&self) -> String {
        self.skill.content.clone()
    }
}

pub fn skills_show(paths: &Paths, id: &str) -> Result<Done<SkillShowReport>> {
    let cat = load_catalogue(paths)?;
    if !cat.contains(id) {
        return Err(err!("unknown tool: {id}"));
    }
    let path = paths.skills_dir().join(id).join("SKILL.md");
    if !path.is_file() {
        return Err(err!("{id}: no fetched skill -- `reactor skills fetch {id}`"));
    }
    let content = String::from_utf8_lossy(&std::fs::read(&path).map_err(|e| err!("{}: {}", path.display(), e))?).into_owned();
    Ok(Done::ok(SkillShowReport { skill: SkillBody { tool: id.to_string(), path: path.display().to_string(), content } }))
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillsFetchReport {
    pub fetched: Vec<FetchResult>,
    pub failed: usize,
}

impl Report for SkillsFetchReport {
    fn human(&self) -> String {
        self.fetched
            .iter()
            .map(|r| format!("{}{}: {}", if r.ok { "  " } else { "! " }, r.tool, r.message))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn skills_fetch(paths: &Paths, ids: &[String]) -> Result<Done<SkillsFetchReport>> {
    let cat = load_catalogue(paths)?;
    let selected = select(&cat, ids)?;
    let targets: Vec<_> = selected.iter().filter_map(|i| cat.get(i)).filter(|t| t.skill.is_some()).collect();
    if targets.is_empty() {
        return Err(err!("no configured upstream skills among the selected tools"));
    }
    let fetched: Vec<FetchResult> = targets.iter().map(|t| fetch_skill(paths, t)).collect();
    let failed = fetched.iter().filter(|r| !r.ok).count();
    Ok(Done::code(SkillsFetchReport { fetched, failed }, if failed > 0 { 1 } else { 0 }))
}

// ---------------------------------------------------------------------------
// services, refresh, completion ids
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ServiceRow {
    pub id: String,
    pub name: String,
    pub label: String,
    pub state: ServiceState,
    pub detail: Option<String>,
    pub status: Status,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceSummary {
    pub up: usize,
    pub down: usize,
    pub unknown: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServicesReport {
    pub services: Vec<ServiceRow>,
    pub summary: ServiceSummary,
    #[serde(skip)]
    rows: Vec<Vec<String>>,
}

impl Report for ServicesReport {
    fn human(&self) -> String {
        if self.rows.is_empty() {
            "no catalogued tool declares a service probe".to_string()
        } else {
            table(&self.rows)
        }
    }
}

/// State of the catalogue entries that are backed by something running.
///
/// A separate command rather than a filter over `tools list` because the
/// question is different: not "what is installed" but "what is up right now".
/// Probing only the service-capable entries is also what makes it cheap enough
/// for a status line to ask on every turn (ADR-0014).
pub fn services(paths: &Paths, flags: ProbeFlags) -> Result<Done<ServicesReport>> {
    let l = load(paths)?;
    let ids: Vec<String> = l.cat.tools.iter().filter(|t| t.service_probe.is_some()).map(|t| t.id.clone()).collect();
    let results = probe(
        paths,
        &l.cat,
        &ids,
        ProbeOpts { refresh: flags.refresh, cached_only: flags.cached, services: true },
    );
    let entries = describe(paths, &l.cat, &l.toolsets, &l.state, &ids, &results, false);

    let (mut up, mut down, mut unknown) = (0, 0, 0);
    let (mut services, mut rows) = (Vec::new(), Vec::new());
    for e in &entries {
        let svc = e.service.as_ref();
        // A tool that is not installed has no service state to report, and
        // "down" would be a claim about something that does not exist here.
        let st = if e.status == Status::Present {
            svc.map(|s| s.state).unwrap_or(ServiceState::Unknown)
        } else {
            ServiceState::Unknown
        };
        match st {
            ServiceState::Up => up += 1,
            ServiceState::Down => down += 1,
            ServiceState::Unknown => unknown += 1,
        }
        let label = svc.and_then(|s| s.label.clone()).filter(|l| !l.is_empty());
        let detail = if st == ServiceState::Up { svc.and_then(|s| s.detail.clone()) } else { None };
        rows.push(vec![
            service_glyph(st).to_string(),
            e.id.clone(),
            label.clone().unwrap_or_default(),
            svc.and_then(|s| s.detail.clone()).filter(|d| !d.is_empty()).unwrap_or_else(|| st.as_str().to_string()),
        ]);
        services.push(ServiceRow {
            id: e.id.clone(),
            name: e.name.clone(),
            label: label.unwrap_or_else(|| e.id.clone()),
            state: st,
            detail,
            status: e.status,
            active: e.active,
        });
    }
    Ok(Done::ok(ServicesReport { services, summary: ServiceSummary { up, down, unknown }, rows }))
}

#[derive(Debug, Clone, Serialize)]
pub struct RefreshReport {
    pub probed: usize,
    pub present: usize,
}

impl Report for RefreshReport {
    fn human(&self) -> String {
        format!("probed {} tool(s); {} present", self.probed, self.present)
    }
}

pub fn refresh(paths: &Paths) -> Result<Done<RefreshReport>> {
    let cat = load_catalogue(paths)?;
    let mut cache = crate::probe::Cache::load(paths, &cat);
    cache.clear();
    cache.save();
    let ids = cat.order();
    let results: HashMap<String, ProbeResult> =
        probe(paths, &cat, &ids, ProbeOpts { refresh: true, cached_only: false, services: true });
    let present = results.values().filter(|r| r.status == Status::Present).count();
    Ok(Done::ok(RefreshReport { probed: ids.len(), present }))
}

/// `reactor __complete tools|toolsets`: bare ids, no probing — a <TAB> press
/// costs a TOML parse and nothing else (ADR-0015).
pub fn complete_ids(paths: &Paths, kind: &str) -> Result<Vec<String>> {
    Ok(if kind == "tools" {
        load_catalogue(paths)?.order()
    } else {
        load_toolsets(paths)?.into_iter().map(|t| t.id).collect()
    })
}

// ---------------------------------------------------------------------------
// session scope
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct PromoteReport {
    /// Where the session's activation was written: the machine-wide default.
    pub path: String,
    pub state: StateDoc,
}

impl Report for PromoteReport {
    fn human(&self) -> String {
        format!("made this session's activation the default -- {}", self.path)
    }
}

/// "Make this the default": copy the session's activation into the machine-wide
/// `state.json` (ADR-0038). The session keeps its own override; the two now agree.
pub fn make_session_state_default(paths: &Paths) -> Result<Done<PromoteReport>> {
    let l = load(paths)?;
    if l.state.scope != Scope::Session {
        return Err(err!("this session has no activation of its own to promote"));
    }
    let doc = l.state.as_doc();
    let target = paths.state_file();
    crate::json::write_json_atomic(&target, &doc)?;
    Ok(Done::ok(PromoteReport { path: target.display().to_string(), state: doc }))
}

/// Drop the session's override: it inherits the project or machine state again.
pub fn clear_session_state(paths: &Paths) -> Result<()> {
    if let Some(p) = &paths.session_state {
        match std::fs::remove_file(p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(err!("{}: {}", p.display(), crate::json::io_reason(&e))),
        }
    }
    Ok(())
}
