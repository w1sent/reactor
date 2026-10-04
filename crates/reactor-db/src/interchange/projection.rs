//! The entity-to-entity graph that centrality, community detection and shortest paths need.
//!
//! The stored graph has claims as nodes, so two entities are not joined by an edge but by a
//! claim they both take part in. A projection joins every two entities of a claim, weighted
//! by how many claims they share. **A claim with k participants becomes k(k−1)/2 edges**, so
//! the large ones are skipped (`max_participants`), and an entity that takes part in a great
//! many claims — `127.0.0.1`, `svchost.exe` — joins everything to everything, so it can be
//! left out (`max_prevalence`). `candidate` roles ("one of these three hosts") are uncertain
//! and left out unless asked for. Annotations and sources are never part of it.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;

use crate::db::Db;
use crate::error::Result;
use crate::graph::{Kind, query};

#[derive(Debug, Clone)]
pub struct Options {
    pub include_deleted: bool,
    pub include_candidates: bool,
    /// Claims with more participants than this are skipped.
    pub max_participants: usize,
    /// Entities in more claims than this are left out.
    pub max_prevalence: Option<usize>,
    /// Only claims whose type starts with this.
    pub claim_prefix: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            include_deleted: false,
            include_candidates: false,
            max_participants: 12,
            max_prevalence: None,
            claim_prefix: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Node {
    pub id: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub value: String,
    /// Claims it takes part in, before any were skipped.
    pub claims: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Edge {
    pub a: String,
    pub b: String,
    /// How many claims join these two.
    pub weight: usize,
    /// By claim type.
    pub types: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Projection {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Claims too large to project.
    pub skipped_large: usize,
    /// Entities left out for taking part in too many claims.
    pub skipped_prevalent: Vec<String>,
}

pub fn project(db: &Db, opts: &Options) -> Result<Projection> {
    let (elements, edges) = query::snapshot(db, opts.include_deleted)?;
    let by_id: HashMap<&str, &query::Element> =
        elements.iter().map(|e| (e.id.as_str(), e)).collect();
    // The entities each claim involves.
    let mut participants: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for g in &edges {
        if g.role == "source" || (g.role == "candidate" && !opts.include_candidates) {
            continue;
        }
        let (Some(from), Some(to)) = (by_id.get(g.from_id.as_str()), by_id.get(g.to_id.as_str()))
        else {
            continue;
        };
        if from.kind.is_claim()
            && to.kind == Kind::Entity
            && opts
                .claim_prefix
                .as_ref()
                .is_none_or(|p| from.type_.starts_with(p.as_str()))
        {
            participants
                .entry(from.id.as_str())
                .or_default()
                .insert(to.id.as_str());
        }
    }
    let mut prevalence: HashMap<&str, usize> = HashMap::new();
    for set in participants.values() {
        for id in set {
            *prevalence.entry(id).or_default() += 1;
        }
    }
    let prevalent: BTreeSet<&str> = opts
        .max_prevalence
        .map(|max| {
            prevalence
                .iter()
                .filter(|(_, n)| **n > max)
                .map(|(id, _)| *id)
                .collect()
        })
        .unwrap_or_default();

    let mut joined: BTreeMap<(&str, &str), BTreeMap<String, usize>> = BTreeMap::new();
    let mut skipped_large = 0;
    for (claim, set) in &participants {
        if set.len() > opts.max_participants {
            skipped_large += 1;
            continue;
        }
        let ids: Vec<&str> = set
            .iter()
            .copied()
            .filter(|id| !prevalent.contains(id))
            .collect();
        let claim_type = &by_id[claim].type_;
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                *joined
                    .entry((a, b))
                    .or_default()
                    .entry(claim_type.clone())
                    .or_default() += 1;
            }
        }
    }
    let used: BTreeSet<&str> = joined.keys().flat_map(|(a, b)| [*a, *b]).collect();
    Ok(Projection {
        nodes: used
            .iter()
            .map(|id| {
                let e = by_id[id];
                Node {
                    id: e.id.clone(),
                    type_: e.type_.clone(),
                    value: e.content.value.clone().unwrap_or_default(),
                    claims: prevalence.get(id).copied().unwrap_or(0),
                }
            })
            .collect(),
        edges: joined
            .into_iter()
            .map(|((a, b), types)| Edge {
                a: a.to_string(),
                b: b.to_string(),
                weight: types.values().sum(),
                types,
            })
            .collect(),
        skipped_large,
        skipped_prevalent: prevalent.iter().map(|s| s.to_string()).collect(),
    })
}

/// `a,b,weight,types` — the shape NetworkX's `read_edgelist` and igraph take.
pub fn edge_list_csv(p: &Projection) -> String {
    let mut w = csv::Writer::from_writer(Vec::new());
    let _ = w.write_record(["source", "target", "weight", "claim_types"]);
    for e in &p.edges {
        let types = e
            .types
            .iter()
            .map(|(t, n)| format!("{t}:{n}"))
            .collect::<Vec<_>>()
            .join(";");
        let _ = w.write_record([&e.a, &e.b, &e.weight.to_string(), &types]);
    }
    String::from_utf8(w.into_inner().unwrap_or_default()).unwrap_or_default()
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// GraphML, which Gephi, NetworkX and igraph read.
pub fn graphml(p: &Projection) -> String {
    let mut out = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<graphml xmlns="http://graphml.graphdrawing.org/xmlns">
  <key id="type" for="node" attr.name="type" attr.type="string"/>
  <key id="value" for="node" attr.name="value" attr.type="string"/>
  <key id="claims" for="node" attr.name="claims" attr.type="int"/>
  <key id="weight" for="edge" attr.name="weight" attr.type="int"/>
  <key id="claim_types" for="edge" attr.name="claim_types" attr.type="string"/>
  <graph id="case" edgedefault="undirected">
"#,
    );
    for n in &p.nodes {
        out.push_str(&format!(
            "    <node id=\"{}\"><data key=\"type\">{}</data><data key=\"value\">{}</data><data key=\"claims\">{}</data></node>\n",
            xml(&n.id), xml(&n.type_), xml(&n.value), n.claims
        ));
    }
    for e in &p.edges {
        let types = e
            .types
            .iter()
            .map(|(t, n)| format!("{t}:{n}"))
            .collect::<Vec<_>>()
            .join(";");
        out.push_str(&format!(
            "    <edge source=\"{}\" target=\"{}\"><data key=\"weight\">{}</data><data key=\"claim_types\">{}</data></edge>\n",
            xml(&e.a), xml(&e.b), e.weight, xml(&types)
        ));
    }
    out.push_str("  </graph>\n</graphml>\n");
    out
}
