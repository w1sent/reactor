//! Large groups of tools are loaded on demand
//! ([ADR-0055](../../../docs/adr/0055-large-tool-groups-are-loaded-on-demand.md)).
//!
//! A *group* is the tools that arrive together: the case graph's five, or one MCP server's.
//! A group whose definitions together cost more than the threshold is not advertised. What
//! the model gets instead is one tool, `tool_search`, whose description lists the group's
//! tools by name and first sentence; it loads what it needs, and a loaded tool is advertised
//! from the next request on, on that branch of the session.
//!
//! The plan is a pure function of the candidates, the loaded set and the threshold: the same
//! inputs give the same request, so the provider's prompt cache holds until a load changes it.

use std::collections::{BTreeMap, BTreeSet};

use crate::context::estimate_tokens;
use crate::inspect::Origin;
use crate::llm::ToolSpec;

/// The name of the loading tool.
pub const TOOL: &str = "tool_search";

/// A definition that could be advertised, and the group it comes with.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub origin: Origin,
    pub group: Option<String>,
    pub spec: ToolSpec,
}

/// What a request carries, and what it leaves for `tool_search`.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// The definitions to send, in order.
    pub advertised: Vec<(Origin, ToolSpec)>,
    /// Every tool of every deferred group, loaded or not: what `tool_search` can be asked
    /// about. Empty when nothing is deferred, and then `tool_search` is not advertised.
    pub deferred: Vec<ToolSpec>,
}

/// What a definition costs every request while it is advertised.
pub fn cost(spec: &ToolSpec) -> u64 {
    estimate_tokens(&spec.name)
        + estimate_tokens(&spec.description)
        + estimate_tokens(&spec.parameters.to_string())
}

/// Decide which groups are deferred: those that cost more than `threshold` tokens.
pub fn plan(candidates: Vec<Candidate>, loaded: &BTreeSet<String>, threshold: usize) -> Plan {
    let mut cost_of: BTreeMap<&str, u64> = BTreeMap::new();
    for c in &candidates {
        if let Some(g) = &c.group {
            *cost_of.entry(g).or_default() += cost(&c.spec);
        }
    }
    let deferred_group = |c: &Candidate| {
        c.group
            .as_deref()
            .is_some_and(|g| cost_of[g] > threshold as u64)
    };
    let mut plan = Plan::default();
    for c in &candidates {
        if deferred_group(c) {
            plan.deferred.push(c.spec.clone());
            if !loaded.contains(&c.spec.name) {
                continue;
            }
        }
        plan.advertised.push((c.origin, c.spec.clone()));
    }
    if !plan.deferred.is_empty() {
        plan.advertised
            .push((Origin::ToolDefinition, spec(&plan.deferred)));
    }
    plan
}

/// The first sentence of a description, short enough for a list.
fn gist(description: &str) -> String {
    let first = description.lines().next().unwrap_or("").trim();
    let end = first
        .find(". ")
        .map(|i| i + 1)
        .unwrap_or(first.len())
        .min(110);
    let mut end = end;
    while !first.is_char_boundary(end) {
        end -= 1;
    }
    first[..end].trim_end_matches(['.', ' ']).to_string()
}

/// The loading tool's definition: it names what can be loaded, so a model that has never
/// seen those tools knows they exist, what each is for and what to ask for.
pub fn spec(deferred: &[ToolSpec]) -> ToolSpec {
    let list: Vec<String> = deferred
        .iter()
        .map(|t| format!("- {}: {}", t.name, gist(&t.description)))
        .collect();
    ToolSpec {
        name: TOOL.into(),
        description: format!(
            "Load tools that are not in your tool list. A loaded tool is in your tool list from your next step.\n\nNot loaded:\n{}",
            list.join("\n")
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "`select:name1,name2` loads those tools; any other text is searched for in their names and descriptions." },
                "max_results": { "type": "integer", "description": "Most tools to load for a text query (default 5)." }
            },
            "required": ["query"]
        }),
    }
}

/// Words that say nothing about which tool is meant.
const STOP: &[&str] = &[
    "an", "and", "are", "as", "at", "be", "by", "for", "from", "if", "in", "is", "it", "of", "on",
    "or", "the", "to", "with",
];

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1 && !STOP.contains(w))
        .map(|w| match w.strip_suffix('s') {
            // `hosts` finds `host`; a word like `class` or `dns` is left alone.
            Some(stem) if w.len() > 3 && !stem.ends_with('s') => stem.to_string(),
            _ => w.to_string(),
        })
        .collect()
}

/// The tools a query asks for: the named ones for `select:`, else the best matches of the
/// words in it, a hit in a name counting for more than one in a description.
pub fn find<'a>(deferred: &'a [ToolSpec], query: &str, max: usize) -> Vec<&'a ToolSpec> {
    let query = query.trim();
    if let Some(names) = query.strip_prefix("select:") {
        return names
            .split(',')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .filter_map(|n| deferred.iter().find(|t| t.name.eq_ignore_ascii_case(n)))
            .collect();
    }
    let wanted = words(query);
    if wanted.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(u32, &ToolSpec)> = deferred
        .iter()
        .filter_map(|t| {
            let (name, text) = (words(&t.name), words(&t.description));
            let score: u32 = wanted
                .iter()
                .map(|w| {
                    3 * name.iter().filter(|n| *n == w).count() as u32
                        + text.iter().filter(|x| *x == w).count().min(2) as u32
                })
                .sum();
            (score > 0).then_some((score, t))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
    scored.into_iter().take(max).map(|(_, t)| t).collect()
}
