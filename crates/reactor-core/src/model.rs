//! The entry model: what a tool looks like once catalogue, probe, activation
//! and install candidates are joined. This is the `--format json` contract for
//! everything that lists tools, so the field order below *is* the byte order.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::catalogue::{Catalogue, DetectKind, OrderedMap, Toolset};
use crate::paths::Paths;
use crate::probe::{ProbeResult, ServiceInfo, Status};
use crate::recipes::{Recipe, available_managers, rank_recipes};
use crate::skills::{SkillStatus, skill_status};
use crate::state::{State, active_ids};

#[derive(Debug, Clone, Serialize)]
pub struct DetectSpec {
    pub kind: DetectKind,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallInfo {
    pub recommended: Option<Recipe>,
    pub candidates: Vec<Recipe>,
    pub notes: OrderedMap<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolEntry {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub source: Option<String>,
    pub invoke: String,
    pub tags: Vec<String>,
    pub detect: DetectSpec,
    pub status: Status,
    pub path: Option<String>,
    pub version: Option<String>,
    pub active: bool,
    /// What state.json says about this tool alone. `active` is derived from the
    /// toolsets too, so the two together are what a UI needs to explain itself
    /// (ADR-0011).
    #[serde(rename = "override")]
    pub override_: Option<&'static str>,
    pub service: Option<ServiceInfo>,
    pub skill: Option<SkillStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install: Option<InstallInfo>,
}

pub fn describe(
    paths: &Paths,
    cat: &Catalogue,
    toolsets: &[Toolset],
    state: &State,
    ids: &[String],
    results: &HashMap<String, ProbeResult>,
    with_install: bool,
) -> Vec<ToolEntry> {
    let managers = with_install.then(|| available_managers(cat));
    describe_using(paths, cat, toolsets, state, ids, results, managers.as_ref())
}

/// [`describe`] with the package managers supplied rather than looked up:
/// `Some` adds each entry's install candidates, ranked against exactly those.
pub fn describe_using(
    paths: &Paths,
    cat: &Catalogue,
    toolsets: &[Toolset],
    state: &State,
    ids: &[String],
    results: &HashMap<String, ProbeResult>,
    managers: Option<&BTreeMap<String, String>>,
) -> Vec<ToolEntry> {
    let active = active_ids(cat, toolsets, state);
    ids.iter()
        .filter_map(|tid| cat.get(tid))
        .map(|t| {
            let unknown = ProbeResult::unknown();
            let r = results.get(&t.id).unwrap_or(&unknown);
            let install = managers.map(|managers| {
                let (candidates, notes) = rank_recipes(t, cat, managers);
                InstallInfo {
                    recommended: candidates.first().cloned(),
                    candidates,
                    notes,
                }
            });
            ToolEntry {
                id: t.id.clone(),
                name: t.name.clone(),
                desc: t.desc.clone(),
                source: t.source.clone(),
                invoke: t.invoke.clone(),
                tags: t.tags.clone(),
                detect: DetectSpec {
                    kind: t.detect_kind,
                    value: t.detect_value.clone(),
                },
                status: r.status,
                path: r.path.clone(),
                version: r.version.clone(),
                active: active.contains(&t.id),
                override_: state.override_of(&t.id),
                service: r.service.clone(),
                skill: skill_status(paths, t),
                install,
            }
        })
        .collect()
}
