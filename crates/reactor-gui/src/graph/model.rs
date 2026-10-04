//! From the stored graph to what is drawn
//! ([ADR-0047](../../../../docs/adr/0047-the-analysis-graph-is-the-case-s-model.md)).
//!
//! The stored structure is honest and verbose: every relation is a node with two role
//! edges. A person should not have to read that. So the view **collapses a binary
//! claim into one labeled line** between the two things it relates, and shows a claim
//! as a node only where the structure matters — it has more than two participants, or
//! something is said about it (an annotation, another claim). A node in the middle of
//! a line means "this relation is being discussed".
//!
//! Sources are provenance, not participants: externals and `source` edges are not
//! drawn. Everything here is a pure function of the snapshot, so it is tested without
//! a window.

use std::collections::{BTreeMap, HashMap, HashSet};

use reactor_db::graph::{Edge, Element, Kind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Entity,
    /// An event or observation that could not be drawn as a line.
    Claim,
    Annotation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    pub type_: String,
    /// What is written on it: an entity's value, an annotation's text, a claim's type.
    pub label: String,
    pub status: String,
    pub confidence: Option<String>,
    /// Written by the agent rather than a person.
    pub by_agent: bool,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// A collapsed binary claim.
    Relation { directed: bool },
    /// A claim node to one of its participants.
    Role,
    /// An annotation to what it is about, supports, refutes or groups.
    Annotation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Indices into [`Model::nodes`].
    pub from: usize,
    pub to: usize,
    pub label: String,
    pub kind: LineKind,
    /// The claims this line stands for: one, or several of the same type between the
    /// same two things, bundled. Empty for a role or annotation line.
    pub claims: Vec<String>,
    /// The worst status among them, so a disputed relation shows as one.
    pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    pub nodes: Vec<Node>,
    pub lines: Vec<Line>,
}

/// The roles a relation is drawn *from*. The stored vocabulary (`known_role`) is the
/// source of this; these are its seeded values, for when it cannot be read.
pub const FROM_ROLES: [&str; 6] = ["src", "actor", "parent", "subject", "query", "cause"];

fn first_line(s: &str, n: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    if line.chars().count() <= n {
        line.to_string()
    } else {
        format!(
            "{}…",
            line.chars().take(n.saturating_sub(1)).collect::<String>()
        )
    }
}

fn worse(a: &str, b: &str) -> String {
    let rank = |s: &str| match s {
        "conflicting" => 2,
        "soft-deleted" => 1,
        _ => 0,
    };
    if rank(b) > rank(a) { b } else { a }.to_string()
}

/// What makes two collapsed claims the same line: its ends, its type, and whether it
/// has a direction.
type BundleKey = (usize, usize, String, bool);

/// Build what is drawn from a snapshot of the graph.
pub fn build(elements: &[Element], edges: &[Edge], from_roles: &[String]) -> Model {
    let by_id: HashMap<&str, &Element> = elements.iter().map(|e| (e.id.as_str(), e)).collect();
    let drawn = |id: &str| by_id.get(id).is_some_and(|e| e.kind != Kind::External);
    // What each element points at, sources and externals aside.
    let mut out: HashMap<&str, Vec<&Edge>> = HashMap::new();
    let mut pointed_at: HashSet<&str> = HashSet::new();
    for g in edges {
        if g.role == "source" || !drawn(&g.from_id) || !drawn(&g.to_id) {
            continue;
        }
        out.entry(g.from_id.as_str()).or_default().push(g);
        pointed_at.insert(g.to_id.as_str());
    }

    // A claim is a line when it relates exactly two things and nothing refers to it.
    let collapses = |e: &Element| {
        e.kind.is_claim()
            && !pointed_at.contains(e.id.as_str())
            && out.get(e.id.as_str()).is_some_and(|p| {
                p.len() == 2
                    && p.iter()
                        .all(|g| by_id[g.to_id.as_str()].kind == Kind::Entity)
            })
    };

    let mut model = Model::default();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for e in elements {
        let kind = match e.kind {
            Kind::External => continue,
            Kind::Entity => NodeKind::Entity,
            Kind::Annotation => NodeKind::Annotation,
            Kind::Event | Kind::Observation if collapses(e) => continue,
            Kind::Event | Kind::Observation => NodeKind::Claim,
        };
        let label = match (&e.content.value, kind) {
            (Some(v), NodeKind::Annotation) => first_line(v, 60),
            (Some(v), _) => first_line(v, 40),
            (None, _) => e.type_.clone(),
        };
        index.insert(e.id.as_str(), model.nodes.len());
        model.nodes.push(Node {
            id: e.id.clone(),
            kind,
            type_: e.type_.clone(),
            label,
            status: e.content.status.clone(),
            confidence: e.content.confidence.clone(),
            by_agent: e.author.starts_with("agent:"),
            tags: e.content.tags.clone(),
        });
    }

    // Collapsed claims, bundled by (from, to, type).
    let mut bundles: BTreeMap<BundleKey, (Vec<String>, String)> = BTreeMap::new();
    for e in elements.iter().filter(|e| collapses(e)) {
        let parts = &out[e.id.as_str()];
        let (a, b) = (parts[0], parts[1]);
        let is_from = |g: &Edge| from_roles.contains(&g.role);
        let (first, second, directed) = match (is_from(a), is_from(b)) {
            (true, false) => (a, b, true),
            (false, true) => (b, a, true),
            // No known direction: a stable order, drawn without an arrow.
            _ if a.to_id <= b.to_id => (a, b, false),
            _ => (b, a, false),
        };
        let (Some(&from), Some(&to)) = (
            index.get(first.to_id.as_str()),
            index.get(second.to_id.as_str()),
        ) else {
            continue;
        };
        let entry = bundles
            .entry((from, to, e.type_.clone(), directed))
            .or_insert_with(|| (Vec::new(), "active".into()));
        entry.0.push(e.id.clone());
        entry.1 = worse(&entry.1, &e.content.status);
    }
    for ((from, to, type_, directed), (claims, status)) in bundles {
        let label = if claims.len() > 1 {
            format!("{type_} ×{}", claims.len())
        } else {
            type_
        };
        model.lines.push(Line {
            from,
            to,
            label,
            kind: LineKind::Relation { directed },
            claims,
            status,
        });
    }

    // Everything that stayed a node: its own edges, labeled by role.
    for node in 0..model.nodes.len() {
        let id = model.nodes[node].id.clone();
        let kind = match model.nodes[node].kind {
            NodeKind::Entity => continue,
            NodeKind::Claim => LineKind::Role,
            NodeKind::Annotation => LineKind::Annotation,
        };
        let mut links: Vec<&&Edge> = out.get(id.as_str()).into_iter().flatten().collect();
        links.sort_by(|a, b| (&a.role, &a.to_id).cmp(&(&b.role, &b.to_id)));
        for g in links {
            // The other end may itself be a collapsed claim: then the line cannot be
            // drawn to a node, and by construction this does not happen (a claim that
            // is pointed at never collapses).
            let Some(&to) = index.get(g.to_id.as_str()) else {
                continue;
            };
            model.lines.push(Line {
                from: node,
                to,
                label: g.role.clone(),
                kind,
                claims: Vec::new(),
                status: model.nodes[node].status.clone(),
            });
        }
    }
    model
}

// -- layout -------------------------------------------------------------------------------

/// The ideal distance between two connected nodes, in pixels at zoom 1.
pub const SPACING: f32 = 190.0;

fn seed(id: &str) -> (f32, f32) {
    // FNV-1a: a spread that depends only on the id, so a layout is reproducible.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let angle = (h & 0xffff) as f32 / 65_535.0 * std::f32::consts::TAU;
    let radius = 0.35 + ((h >> 16) & 0xffff) as f32 / 65_535.0 * 0.65;
    (angle.cos() * radius, angle.sin() * radius)
}

/// Where each node goes. Force-directed (Fruchterman–Reingold): connected nodes pull
/// together, every pair pushes apart.
///
/// **Nodes that already have a position keep it.** Only new ones are placed, starting
/// near what they connect to, so the picture a person has learned does not rearrange
/// itself when the agent adds something.
pub fn layout(model: &Model, known: &HashMap<String, (f32, f32)>) -> Vec<(f32, f32)> {
    let n = model.nodes.len();
    if n == 0 {
        return Vec::new();
    }
    let fixed: Vec<bool> = model
        .nodes
        .iter()
        .map(|node| known.contains_key(&node.id))
        .collect();
    let mut neighbours: Vec<Vec<usize>> = vec![Vec::new(); n];
    for l in &model.lines {
        if l.from != l.to {
            neighbours[l.from].push(l.to);
            neighbours[l.to].push(l.from);
        }
    }
    let spread = SPACING * (n as f32).sqrt();
    let mut pos: Vec<(f32, f32)> = (0..n)
        .map(|i| {
            if let Some(p) = known.get(&model.nodes[i].id) {
                return *p;
            }
            let (sx, sy) = seed(&model.nodes[i].id);
            // Start beside a neighbour that is already placed, else on the disc.
            match neighbours[i]
                .iter()
                .find_map(|&j| known.get(&model.nodes[j].id))
            {
                Some((x, y)) => (x + sx * SPACING, y + sy * SPACING),
                None => (sx * spread * 0.5, sy * spread * 0.5),
            }
        })
        .collect();
    if fixed.iter().all(|f| *f) {
        return pos;
    }

    let rounds = 220;
    let mut heat = spread * 0.25;
    for _ in 0..rounds {
        let mut push = vec![(0.0f32, 0.0f32); n];
        for a in 0..n {
            for b in (a + 1)..n {
                let (mut dx, mut dy) = (pos[a].0 - pos[b].0, pos[a].1 - pos[b].1);
                let mut d = (dx * dx + dy * dy).sqrt();
                if d < 0.01 {
                    // Exactly on top of each other: part them along a direction that
                    // depends only on which two they are.
                    let (sx, sy) = seed(&format!("{}{}", model.nodes[a].id, model.nodes[b].id));
                    (dx, dy, d) = (sx, sy, (sx * sx + sy * sy).sqrt().max(0.01));
                }
                let f = SPACING * SPACING / d;
                let (fx, fy) = (dx / d * f, dy / d * f);
                push[a].0 += fx;
                push[a].1 += fy;
                push[b].0 -= fx;
                push[b].1 -= fy;
            }
        }
        for l in &model.lines {
            if l.from == l.to {
                continue;
            }
            let (dx, dy) = (pos[l.from].0 - pos[l.to].0, pos[l.from].1 - pos[l.to].1);
            let d = (dx * dx + dy * dy).sqrt().max(0.01);
            let f = d * d / SPACING;
            let (fx, fy) = (dx / d * f, dy / d * f);
            push[l.from].0 -= fx;
            push[l.from].1 -= fy;
            push[l.to].0 += fx;
            push[l.to].1 += fy;
        }
        for i in 0..n {
            if fixed[i] {
                continue;
            }
            let (fx, fy) = push[i];
            let len = (fx * fx + fy * fy).sqrt().max(0.01);
            let step = len.min(heat);
            pos[i].0 += fx / len * step;
            pos[i].1 += fy / len * step;
        }
        heat *= 0.975;
    }
    // A fresh layout is centred on the origin; one that extends an old one stays put.
    if !fixed.iter().any(|f| *f) {
        let (cx, cy) = pos.iter().fold((0.0, 0.0), |(x, y), p| {
            (x + p.0 / n as f32, y + p.1 / n as f32)
        });
        for p in &mut pos {
            *p = (p.0 - cx, p.1 - cy);
        }
    }
    pos
}

/// Somewhere near `want` that no node in `taken` is within `clear` of: `want` itself if it
/// is free, else the nearest free place on widening rings around it. For putting what a
/// person adds where they can see it, and not on top of something.
pub fn free_spot(taken: &[(f32, f32)], want: (f32, f32), clear: f32) -> (f32, f32) {
    let free = |p: (f32, f32)| taken.iter().all(|t| (t.0 - p.0).hypot(t.1 - p.1) >= clear);
    if free(want) {
        return want;
    }
    for ring in 1..=40 {
        let r = clear * ring as f32 * 0.6;
        let steps = (6 * ring).max(8);
        for i in 0..steps {
            let a = std::f32::consts::TAU * i as f32 / steps as f32;
            let p = (want.0 + a.cos() * r, want.1 + a.sin() * r);
            if free(p) {
                return p;
            }
        }
    }
    want
}

#[cfg(test)]
mod tests {
    use super::*;
    use reactor_db::Case;
    use reactor_db::graph::{self, Batch, Change, Content, Link, Put, Target, query};
    use reactor_db::manifest::Author;

    fn roles() -> Vec<String> {
        FROM_ROLES.iter().map(|r| r.to_string()).collect()
    }

    fn claim(kind: Kind, type_: &str, links: Vec<Link>) -> Put {
        Put {
            links,
            ..Put::new(kind, type_)
        }
    }

    /// The graph concept's example, plus a second resolution of the same pair.
    fn incident(case: &Case) -> Vec<String> {
        let batch = Batch::new(Author::analyst())
            .put(claim(
                Kind::Event,
                "auth.logon",
                vec![
                    Link::entity("target", "host.name", "ws-042"),
                    Link::entity("actor", "account.user", "jdoe"),
                    Link::source("csv:vpn.csv"),
                ],
            ))
            .put(claim(
                Kind::Event,
                "net.connection",
                vec![
                    Link::entity("src", "host.name", "ws-042"),
                    Link::entity("dst", "net.ipv4", "198.51.100.7"),
                    Link::source("csv:fw.csv"),
                ],
            ))
            .put(claim(
                Kind::Observation,
                "net.resolution",
                vec![
                    Link::entity("query", "net.domain", "cdn-sync.example"),
                    Link::entity("answer", "net.ipv4", "198.51.100.7"),
                    Link::source("csv:pdns.csv").at("row=1", None),
                ],
            ))
            .put(claim(
                Kind::Observation,
                "net.resolution",
                vec![
                    Link::entity("query", "net.domain", "cdn-sync.example"),
                    Link::entity("answer", "net.ipv4", "198.51.100.7"),
                    Link::source("csv:pdns.csv").at("row=2", None),
                ],
            ));
        graph::apply(case.db(), batch).unwrap().ids
    }

    fn model(case: &Case) -> Model {
        let (elements, edges) = query::snapshot(case.db(), false).unwrap();
        build(&elements, &edges, &roles())
    }

    fn case() -> (tempfile::TempDir, Case) {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), None).unwrap();
        (dir, case)
    }

    fn label(m: &Model, i: usize) -> &str {
        &m.nodes[i].label
    }

    #[test]
    fn binary_claims_are_lines_between_entities_and_sources_are_not_drawn() {
        let (_d, case) = case();
        incident(&case);
        let m = model(&case);
        // Four entities; no claim nodes, no externals.
        assert_eq!(m.nodes.len(), 4);
        assert!(m.nodes.iter().all(|n| n.kind == NodeKind::Entity));
        let lines: Vec<(&str, &str, &str, LineKind)> = m
            .lines
            .iter()
            .map(|l| (label(&m, l.from), label(&m, l.to), l.label.as_str(), l.kind))
            .collect();
        let directed = LineKind::Relation { directed: true };
        assert_eq!(lines.len(), 3);
        // Drawn from the actor to the target, whatever order the roles were given in.
        assert!(lines.contains(&("jdoe", "ws-042", "auth.logon", directed)));
        assert!(lines.contains(&("ws-042", "198.51.100.7", "net.connection", directed)));
        // Two resolutions of the same pair are one line that says so.
        assert!(lines.contains(&(
            "cdn-sync.example",
            "198.51.100.7",
            "net.resolution ×2",
            directed
        )));
        assert_eq!(m.lines.iter().map(|l| l.claims.len()).sum::<usize>(), 4);
    }

    #[test]
    fn a_claim_that_is_discussed_or_has_three_participants_stays_a_node() {
        let (_d, case) = case();
        let ids = incident(&case);
        let db = case.db();
        graph::apply(
            db,
            Batch::new(Author::agent("m"))
                .put(Put {
                    content: Content::value("Resolution overlaps the beacon window"),
                    links: vec![Link::new("about", Target::Id(ids[2].clone())).pinned()],
                    ..Put::new(Kind::Annotation, "analysis.judgement")
                })
                .put(claim(
                    Kind::Event,
                    "process.start",
                    vec![
                        Link::entity("parent", "process.name", "winword.exe"),
                        Link::entity("child", "process.name", "powershell.exe"),
                        Link::entity("actor", "account.user", "jdoe"),
                        Link::source("jsonl:edr.jsonl"),
                    ],
                )),
        )
        .unwrap();
        let m = model(&case);
        let claims: Vec<&Node> = m
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Claim)
            .collect();
        assert_eq!(
            claims.len(),
            2,
            "the judged resolution, and the three-way process start"
        );
        let notes: Vec<&Node> = m
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Annotation)
            .collect();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].by_agent);
        // The judged resolution is now a node with a role line to each end; its twin,
        // which nobody discusses, is still a plain line.
        let roles: Vec<&str> = m
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Role && m.nodes[l.from].type_ == "net.resolution")
            .map(|l| l.label.as_str())
            .collect();
        assert_eq!(roles, ["answer", "query"]);
        assert!(
            m.lines
                .iter()
                .any(|l| l.label == "net.resolution" && l.claims.len() == 1)
        );
        assert!(
            m.lines
                .iter()
                .any(|l| l.kind == LineKind::Annotation && l.label == "about")
        );
        assert_eq!(
            m.lines
                .iter()
                .filter(|l| m.nodes[l.from].type_ == "process.start")
                .count(),
            3
        );
    }

    #[test]
    fn a_disputed_relation_shows_as_one_and_a_soft_deleted_one_is_gone() {
        let (_d, case) = case();
        let ids = incident(&case);
        let set = |id: &str, status: &str| {
            graph::apply(
                case.db(),
                Batch::new(Author::analyst()).change(Change {
                    id: id.to_string(),
                    status: Some(status.into()),
                    ..Change::default()
                }),
            )
            .unwrap();
        };
        set(&ids[3], "conflicting");
        set(&ids[0], "soft-deleted");
        let m = model(&case);
        assert!(!m.lines.iter().any(|l| l.label == "auth.logon"));
        let res = m
            .lines
            .iter()
            .find(|l| l.label.starts_with("net.resolution"))
            .unwrap();
        assert_eq!(res.status, "conflicting", "one of the two is disputed");
        // jdoe is still there, now with nothing attached.
        assert!(m.nodes.iter().any(|n| n.label == "jdoe"));
    }

    #[test]
    fn a_relation_with_no_known_direction_is_drawn_without_an_arrow() {
        let (_d, case) = case();
        graph::apply(
            case.db(),
            Batch::new(Author::analyst()).put(claim(
                Kind::Observation,
                "x.linked",
                vec![
                    Link::entity("left", "net.domain", "a.example"),
                    Link::entity("right", "net.domain", "b.example"),
                    Link::source("notes.csv"),
                ],
            )),
        )
        .unwrap();
        let m = model(&case);
        assert_eq!(m.lines[0].kind, LineKind::Relation { directed: false });
    }

    #[test]
    fn a_layout_is_reproducible_spread_out_and_keeps_what_was_already_placed() {
        let (_d, case) = case();
        incident(&case);
        let m = model(&case);
        let first = layout(&m, &HashMap::new());
        assert_eq!(
            first,
            layout(&m, &HashMap::new()),
            "the same graph, the same picture"
        );
        assert!(first.iter().all(|(x, y)| x.is_finite() && y.is_finite()));
        for a in 0..first.len() {
            for b in (a + 1)..first.len() {
                let d =
                    ((first[a].0 - first[b].0).powi(2) + (first[a].1 - first[b].1).powi(2)).sqrt();
                assert!(
                    d > SPACING * 0.4,
                    "{} and {} are {d} apart",
                    label(&m, a),
                    label(&m, b)
                );
            }
        }
        // Connected things end up nearer than unconnected ones.
        let at = |name: &str| first[m.nodes.iter().position(|n| n.label == name).unwrap()];
        let dist =
            |a: (f32, f32), b: (f32, f32)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
        assert!(dist(at("jdoe"), at("ws-042")) < dist(at("jdoe"), at("cdn-sync.example")));

        // Something new arrives: what was placed does not move, and the newcomer lands
        // near what it connects to.
        let known: HashMap<String, (f32, f32)> = m
            .nodes
            .iter()
            .zip(&first)
            .map(|(n, p)| (n.id.clone(), *p))
            .collect();
        graph::apply(
            case.db(),
            Batch::new(Author::analyst()).put(claim(
                Kind::Event,
                "auth.logon",
                vec![
                    Link::entity("actor", "account.user", "asmith"),
                    Link::entity("target", "host.name", "ws-042"),
                    Link::source("csv:vpn.csv").at("row=9", None),
                ],
            )),
        )
        .unwrap();
        let grown = model(&case);
        let second = layout(&grown, &known);
        for (node, p) in grown.nodes.iter().zip(&second) {
            if let Some(old) = known.get(&node.id) {
                assert_eq!(p, old, "{} moved", node.label);
            }
        }
        let new = second[grown
            .nodes
            .iter()
            .position(|n| n.label == "asmith")
            .unwrap()];
        assert!(
            dist(new, at("ws-042")) < SPACING * 2.5,
            "{}",
            dist(new, at("ws-042"))
        );
        assert!(layout(&Model::default(), &HashMap::new()).is_empty());
    }

    #[test]
    fn a_spot_is_the_one_asked_for_or_the_nearest_free_one() {
        let taken = [(0.0, 0.0), (100.0, 0.0), (0.0, 100.0)];
        assert_eq!(free_spot(&taken, (500.0, 500.0), 90.0), (500.0, 500.0));
        let p = free_spot(&taken, (0.0, 0.0), 90.0);
        assert!(
            taken.iter().all(|t| (t.0 - p.0).hypot(t.1 - p.1) >= 90.0),
            "{p:?}"
        );
        assert!(p.0.hypot(p.1) < 200.0, "near what was asked for: {p:?}");
        assert_eq!(free_spot(&[], (3.0, 4.0), 90.0), (3.0, 4.0));
    }
}
