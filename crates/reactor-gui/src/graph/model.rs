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

/// How big a node is for laying things out, in pixels at zoom 1: the widest and tallest the
/// panel draws, so that no two boxes touch.
pub(super) const NODE_W: f32 = 170.0;
pub(super) const NODE_H: f32 = 44.0;
/// The space between two groups of nodes that are not connected.
pub(super) const GAP: f32 = 40.0;

/// Force-directed layout (Fruchterman–Reingold) of one connected group: connected nodes
/// pull together, every pair pushes apart. `edges` and `pos` are indices into the group.
fn spring(
    pos: &mut [(f32, f32)],
    fixed: &[bool],
    edges: &[(usize, usize)],
    ids: &[&str],
    spread: f32,
) {
    let n = pos.len();
    if fixed.iter().all(|f| *f) {
        return;
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
                    let (sx, sy) = seed(&format!("{}{}", ids[a], ids[b]));
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
        for &(from, to) in edges {
            let (dx, dy) = (pos[from].0 - pos[to].0, pos[from].1 - pos[to].1);
            let d = (dx * dx + dy * dy).sqrt().max(0.01);
            let f = d * d / SPACING;
            let (fx, fy) = (dx / d * f, dy / d * f);
            push[from].0 -= fx;
            push[from].1 -= fy;
            push[to].0 += fx;
            push[to].1 += fy;
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
}

/// Lay boxes of these sizes out in rows, close together, wider than tall: the offset of each
/// one's top-left corner. Biggest first, so the small ones fill in behind them.
pub(super) fn pack(sizes: &[(f32, f32)], gap: f32) -> Vec<(f32, f32)> {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|&a, &b| {
        (sizes[b].0 * sizes[b].1)
            .total_cmp(&(sizes[a].0 * sizes[a].1))
            .then(a.cmp(&b))
    });
    let area: f32 = sizes.iter().map(|(w, h)| w * h).sum();
    let widest = sizes.iter().map(|s| s.0).fold(0.0f32, f32::max);
    let row_width = (area.sqrt() * 1.6).max(widest);
    let mut out = vec![(0.0, 0.0); sizes.len()];
    let (mut x, mut y, mut row_height) = (0.0f32, 0.0f32, 0.0f32);
    for i in order {
        let (w, h) = sizes[i];
        if x > 0.0 && x + w > row_width {
            y += row_height + gap;
            x = 0.0;
            row_height = 0.0;
        }
        out[i] = (x, y);
        x += w + gap;
        row_height = row_height.max(h);
    }
    out
}

/// A group laid out alone, waiting to be packed: its nodes, where they are relative to one
/// another, and the top-left corner of those positions.
pub(super) type Loose = (Vec<usize>, Vec<(f32, f32)>, (f32, f32));

/// Where each node goes.
///
/// **Each connected group is laid out on its own** (by [`spring`]) **and the groups are then
/// packed close together**, not left to push one another apart: a node that is connected to
/// nothing, or a small cluster, sits beside the big one and not at the other end of the plane.
///
/// **Nodes that already have a position keep it.** Only new ones are placed, starting
/// near what they connect to, so the picture a person has learned does not rearrange
/// itself when the agent adds something. A new group connected to nothing placed goes in
/// rows below what is there.
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
    let mut pos: Vec<(f32, f32)> = model
        .nodes
        .iter()
        .map(|node| known.get(&node.id).copied().unwrap_or((0.0, 0.0)))
        .collect();
    if fixed.iter().all(|f| *f) {
        return pos;
    }

    let (groups, group_of, neighbours) = connected_groups(model);

    // Groups that hold something placed already are settled in place; the others are laid
    // out alone, to be packed.
    let mut loose: Vec<Loose> = Vec::new();
    // Where the settled groups are, for putting the loose ones beside them.
    let mut settled: Vec<(f32, f32)> = Vec::new();
    for members in &groups {
        let m = members.len();
        let local = |global: usize| members.binary_search(&global).unwrap();
        let edges: Vec<(usize, usize)> = model
            .lines
            .iter()
            .filter(|l| l.from != l.to && group_of[l.from] == group_of[members[0]])
            .map(|l| (local(l.from), local(l.to)))
            .collect();
        let ids: Vec<&str> = members
            .iter()
            .map(|&i| model.nodes[i].id.as_str())
            .collect();
        let fixed_here: Vec<bool> = members.iter().map(|&i| fixed[i]).collect();
        let spread = SPACING * (m as f32).sqrt();
        let any_fixed = fixed_here.iter().any(|f| *f);
        let mut local_pos: Vec<(f32, f32)> = members
            .iter()
            .map(|&i| {
                if fixed[i] {
                    return pos[i];
                }
                let (sx, sy) = seed(&model.nodes[i].id);
                // Start beside a neighbour that is already placed, else on a disc.
                match neighbours[i].iter().find(|&&j| fixed[j]) {
                    Some(&j) => (pos[j].0 + sx * SPACING, pos[j].1 + sy * SPACING),
                    None => (sx * spread * 0.5, sy * spread * 0.5),
                }
            })
            .collect();
        spring(&mut local_pos, &fixed_here, &edges, &ids, spread);
        if any_fixed {
            for (k, &i) in members.iter().enumerate() {
                pos[i] = local_pos[k];
            }
            settled.extend(local_pos);
        } else {
            let mut lo = (f32::MAX, f32::MAX);
            for p in &local_pos {
                lo = (lo.0.min(p.0), lo.1.min(p.1));
            }
            loose.push((members.clone(), local_pos, lo));
        }
    }
    place_loose(&loose, &settled, &mut pos, (NODE_W, NODE_H), GAP);
    pos
}

/// The connected groups of the model, as lists of node indices in order; which group each
/// node is in; and each node's neighbours.
pub(super) fn connected_groups(model: &Model) -> (Vec<Vec<usize>>, Vec<usize>, Vec<Vec<usize>>) {
    let n = model.nodes.len();
    let mut neighbours: Vec<Vec<usize>> = vec![Vec::new(); n];
    for l in &model.lines {
        if l.from != l.to {
            neighbours[l.from].push(l.to);
            neighbours[l.to].push(l.from);
        }
    }
    let mut group_of = vec![usize::MAX; n];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for start in 0..n {
        if group_of[start] != usize::MAX {
            continue;
        }
        let g = groups.len();
        let mut members = Vec::new();
        let mut stack = vec![start];
        group_of[start] = g;
        while let Some(i) = stack.pop() {
            members.push(i);
            for &j in &neighbours[i] {
                if group_of[j] == usize::MAX {
                    group_of[j] = g;
                    stack.push(j);
                }
            }
        }
        members.sort_unstable();
        groups.push(members);
    }
    (groups, group_of, neighbours)
}

/// Pack groups that were laid out alone: side by side, close, wider than tall. The block goes
/// centred on the origin if nothing is `settled`, else in rows below what is.
///
/// `node` is the room one node takes and `gap` the space between groups.
pub(super) fn place_loose(
    loose: &[Loose],
    settled: &[(f32, f32)],
    pos: &mut [(f32, f32)],
    node: (f32, f32),
    gap: f32,
) {
    let (node_w, node_h) = node;
    if loose.is_empty() {
        return;
    }
    // Pack the loose groups: side by side, close, wider than tall.
    let sizes: Vec<(f32, f32)> = loose
        .iter()
        .map(|(_, local_pos, lo)| {
            let mut hi = (f32::MIN, f32::MIN);
            for p in local_pos {
                hi = (hi.0.max(p.0), hi.1.max(p.1));
            }
            (hi.0 - lo.0 + node_w, hi.1 - lo.1 + node_h)
        })
        .collect();
    let offsets = pack(&sizes, gap);
    // Where the packed block goes: centred on the origin if nothing is placed, else in
    // rows below what is.
    let (block_w, block_h) = offsets
        .iter()
        .zip(&sizes)
        .fold((0.0f32, 0.0f32), |(w, h), (o, s)| {
            (w.max(o.0 + s.0), h.max(o.1 + s.1))
        });
    let origin = if settled.is_empty() {
        (-block_w / 2.0, -block_h / 2.0)
    } else {
        let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
        for p in settled {
            lo = (lo.0.min(p.0), lo.1.min(p.1));
            hi = (hi.0.max(p.0), hi.1.max(p.1));
        }
        (lo.0 - node_w / 2.0, hi.1 + node_h / 2.0 + gap)
    };
    for ((members, local_pos, lo), (off, _size)) in loose.iter().zip(offsets.iter().zip(&sizes)) {
        for (k, &i) in members.iter().enumerate() {
            pos[i] = (
                origin.0 + off.0 + node_w / 2.0 + (local_pos[k].0 - lo.0),
                origin.1 + off.1 + node_h / 2.0 + (local_pos[k].1 - lo.1),
            );
        }
    }
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

// -- filtering ---------------------------------------------------------------------------------

/// What one word of a filter asks.
#[derive(Debug, Clone, PartialEq)]
enum Ask {
    /// In the label or the type.
    Text(String),
    /// `type:net.` — the type starts with it, or a part of it does (`ipv4` finds `net.ipv4`).
    Type(String),
    /// `tag:phase.` — a tag that is or starts with it.
    Tag(String),
    /// `by:agent` or `by:person`.
    By { agent: bool },
    /// `status:conflicting`.
    Status(String),
    /// `kind:entity`, `kind:claim` or `kind:note`.
    Kind(NodeKind),
}

#[derive(Debug, Clone, PartialEq)]
struct Term {
    /// A leading `-`: the node must not match.
    not: bool,
    ask: Ask,
}

/// Which nodes to show, as typed: words that must all hold. A bare word is looked for in
/// the label and the type; `type:`, `tag:`, `by:`, `status:` and `kind:` ask about those;
/// a leading `-` negates. Case is ignored.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    terms: Vec<Term>,
}

/// Read a filter from what is typed. A word that asks about something unknown (`foo:bar`)
/// is looked for as text, as typed.
pub fn parse_filter(text: &str) -> Filter {
    let terms = text
        .split_whitespace()
        .filter_map(|word| {
            let (not, word) = match word.strip_prefix('-') {
                Some(rest) if !rest.is_empty() => (true, rest),
                _ => (false, word),
            };
            let lower = word.to_lowercase();
            let ask = match lower.split_once(':') {
                Some(("type", v)) if !v.is_empty() => Ask::Type(v.to_string()),
                Some(("tag", v)) if !v.is_empty() => Ask::Tag(v.to_string()),
                Some(("status", v)) if !v.is_empty() => Ask::Status(v.to_string()),
                Some(("by", "agent")) => Ask::By { agent: true },
                Some(("by", "person" | "analyst" | "user")) => Ask::By { agent: false },
                Some(("kind", "entity")) => Ask::Kind(NodeKind::Entity),
                Some(("kind", "claim")) => Ask::Kind(NodeKind::Claim),
                Some(("kind", "note" | "annotation")) => Ask::Kind(NodeKind::Annotation),
                _ if lower.is_empty() => return None,
                _ => Ask::Text(lower),
            };
            Some(Term { not, ask })
        })
        .collect();
    Filter { terms }
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Whether a node is to be shown.
    pub fn matches(&self, node: &Node) -> bool {
        let type_ = node.type_.to_lowercase();
        self.terms.iter().all(|t| {
            let hit = match &t.ask {
                Ask::Text(w) => node.label.to_lowercase().contains(w) || type_.contains(w),
                Ask::Type(p) => type_.starts_with(p) || type_.contains(&format!(".{p}")),
                Ask::Tag(p) => node.tags.iter().any(|g| g.to_lowercase().starts_with(p)),
                Ask::By { agent } => node.by_agent == *agent,
                Ask::Status(s) => node.status.to_lowercase() == *s,
                Ask::Kind(k) => node.kind == *k,
            };
            hit != t.not
        })
    }
}

/// Which nodes of the model the filter lets through. A line is drawn only between two that
/// are shown.
pub fn visible(model: &Model, filter: &Filter) -> Vec<bool> {
    model.nodes.iter().map(|n| filter.matches(n)).collect()
}

// -- groups ------------------------------------------------------------------------------------

/// The nodes that share a tag, as the box they sit in.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub tag: String,
    pub members: usize,
    /// The corners of the box around the members' centres, in world units.
    pub min: (f32, f32),
    pub max: (f32, f32),
}

/// A box for each tag that two or more of the shown nodes have, the most shared first, at
/// most `limit` of them. Boxes may overlap: a node with several tags is in several.
pub fn tag_groups(
    model: &Model,
    positions: &HashMap<String, (f32, f32)>,
    shown: &[bool],
    limit: usize,
) -> Vec<Group> {
    let mut by_tag: BTreeMap<&str, Vec<(f32, f32)>> = BTreeMap::new();
    for (node, show) in model.nodes.iter().zip(shown) {
        if !show {
            continue;
        }
        let Some(p) = positions.get(&node.id) else {
            continue;
        };
        for tag in &node.tags {
            by_tag.entry(tag.as_str()).or_default().push(*p);
        }
    }
    let mut groups: Vec<Group> = by_tag
        .into_iter()
        .filter(|(_, points)| points.len() >= 2)
        .map(|(tag, points)| {
            let (mut min, mut max) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
            for p in &points {
                min = (min.0.min(p.0), min.1.min(p.1));
                max = (max.0.max(p.0), max.1.max(p.1));
            }
            Group {
                tag: tag.to_string(),
                members: points.len(),
                min,
                max,
            }
        })
        .collect();
    groups.sort_by(|a, b| b.members.cmp(&a.members).then(a.tag.cmp(&b.tag)));
    groups.truncate(limit);
    groups
}

// -- sources -----------------------------------------------------------------------------------

/// Where "find source" goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceTarget {
    /// A page: opened in the browser.
    Url(String),
    /// What was said in a session of the case: that session's transcript.
    Session { id: String, entry: Option<u64> },
    /// A file of the case, at a line if the locator says one.
    File { path: String, line: Option<u32> },
}

/// A line number in a locator: `line=42`, `line 42`, `L42` or `:42`.
fn line_in(locator: &str) -> Option<u32> {
    let lower = locator.to_lowercase();
    for key in ["line=", "line ", "l", ":"] {
        if let Some(rest) = lower.strip_prefix(key)
            && let digits = rest
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
            && let Ok(n) = digits.parse::<u32>()
        {
            return Some(n);
        }
    }
    None
}

/// What a source's URI and locator point at.
pub fn source_target(uri: &str, locator: Option<&str>) -> SourceTarget {
    if uri.starts_with("http://") || uri.starts_with("https://") {
        return SourceTarget::Url(uri.to_string());
    }
    if let Some(rest) = uri.strip_prefix("session:") {
        let (id, fragment) = rest.split_once('#').unwrap_or((rest, ""));
        let entry = fragment
            .strip_prefix("entry=")
            .and_then(|n| n.parse::<u64>().ok());
        return SourceTarget::Session {
            id: id.to_string(),
            entry,
        };
    }
    let path = uri.split(['#', '?']).next().unwrap_or(uri).to_string();
    SourceTarget::File {
        path,
        line: locator.and_then(line_in),
    }
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

    fn node(
        id: &str,
        kind: NodeKind,
        type_: &str,
        label: &str,
        tags: &[&str],
        agent: bool,
    ) -> Node {
        Node {
            id: id.into(),
            kind,
            type_: type_.into(),
            label: label.into(),
            status: "active".into(),
            confidence: None,
            by_agent: agent,
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    fn sample() -> Model {
        Model {
            nodes: vec![
                node(
                    "a",
                    NodeKind::Entity,
                    "net.ipv4",
                    "10.0.0.5",
                    &["phase.recon"],
                    false,
                ),
                node(
                    "b",
                    NodeKind::Entity,
                    "net.domain",
                    "Evil.example",
                    &["phase.recon", "c2"],
                    true,
                ),
                node(
                    "c",
                    NodeKind::Annotation,
                    "analysis.hypothesis",
                    "jdoe is compromised",
                    &[],
                    false,
                ),
                node(
                    "d",
                    NodeKind::Claim,
                    "net.connection",
                    "net.connection",
                    &["c2"],
                    true,
                ),
            ],
            lines: Vec::new(),
        }
    }

    fn shown(model: &Model, text: &str) -> Vec<String> {
        let f = parse_filter(text);
        model
            .nodes
            .iter()
            .zip(visible(model, &f))
            .filter(|(_, v)| *v)
            .map(|(n, _)| n.id.clone())
            .collect()
    }

    #[test]
    fn a_filter_is_words_that_must_all_hold_and_can_ask_about_type_tag_author_status_and_kind() {
        let m = sample();
        assert_eq!(shown(&m, ""), ["a", "b", "c", "d"]);
        assert_eq!(
            shown(&m, "evil"),
            ["b"],
            "case is ignored, label is looked in"
        );
        assert_eq!(
            shown(&m, "net."),
            ["a", "b", "d"],
            "a bare word is in the type too"
        );
        assert_eq!(shown(&m, "type:net.ip"), ["a"]);
        assert_eq!(shown(&m, "type:ipv4"), ["a"], "a part of a dotted type");
        assert_eq!(shown(&m, "tag:phase"), ["a", "b"]);
        assert_eq!(shown(&m, "tag:c2 by:agent"), ["b", "d"]);
        assert_eq!(shown(&m, "by:person"), ["a", "c"]);
        assert_eq!(shown(&m, "kind:note"), ["c"]);
        assert_eq!(shown(&m, "kind:claim"), ["d"]);
        assert_eq!(shown(&m, "-tag:c2 net"), ["a"]);
        assert_eq!(shown(&m, "status:conflicting"), Vec::<String>::new());
        // An unknown key is just text, colon and all.
        assert_eq!(shown(&m, "foo:bar"), Vec::<String>::new());
        assert!(parse_filter("   ").is_empty());
    }

    #[test]
    fn a_box_is_made_for_each_tag_two_shown_nodes_share_the_most_shared_first() {
        let m = sample();
        let pos: HashMap<String, (f32, f32)> = [
            ("a", (0., 0.)),
            ("b", (100., 50.)),
            ("c", (500., 500.)),
            ("d", (-20., 80.)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let all = vec![true; 4];
        let groups = tag_groups(&m, &pos, &all, 10);
        let tags: Vec<&str> = groups.iter().map(|g| g.tag.as_str()).collect();
        // Two nodes each; ties by name.
        assert_eq!(tags, ["c2", "phase.recon"]);
        let recon = groups.iter().find(|g| g.tag == "phase.recon").unwrap();
        assert_eq!(
            (recon.min, recon.max, recon.members),
            ((0., 0.), (100., 50.), 2)
        );
        let c2 = groups.iter().find(|g| g.tag == "c2").unwrap();
        assert_eq!((c2.min, c2.max), ((-20., 50.), (100., 80.)));
        // A hidden node is not in a box, and a tag shared by one is no group.
        let hide_b = vec![true, false, true, true];
        assert!(tag_groups(&m, &pos, &hide_b, 10).is_empty());
        assert_eq!(tag_groups(&m, &pos, &all, 1).len(), 1);
    }

    #[test]
    fn a_source_is_a_page_a_session_entry_or_a_file_at_a_line() {
        use SourceTarget::*;
        assert_eq!(
            source_target("https://vendor.example/a", None),
            Url("https://vendor.example/a".into())
        );
        assert_eq!(
            source_target("session:3#entry=7", None),
            Session {
                id: "3".into(),
                entry: Some(7)
            }
        );
        assert_eq!(
            source_target("session:3", None),
            Session {
                id: "3".into(),
                entry: None
            }
        );
        assert_eq!(
            source_target("logs/auth.log", Some("line=42")),
            File {
                path: "logs/auth.log".into(),
                line: Some(42)
            }
        );
        assert_eq!(
            source_target("logs/auth.log#t", Some("L7")),
            File {
                path: "logs/auth.log".into(),
                line: Some(7)
            }
        );
        assert_eq!(
            source_target("data/c2.sqlite", Some("row=abc")),
            File {
                path: "data/c2.sqlite".into(),
                line: None
            }
        );
        assert_eq!(
            source_target("a.txt", Some(":12")),
            File {
                path: "a.txt".into(),
                line: Some(12)
            }
        );
    }

    fn bare(count: usize, links: &[(usize, usize)]) -> Model {
        Model {
            nodes: (0..count)
                .map(|i| Node {
                    id: format!("n{i}"),
                    kind: NodeKind::Entity,
                    type_: "host.name".into(),
                    label: format!("host-{i}"),
                    status: "active".into(),
                    confidence: None,
                    by_agent: false,
                    tags: Vec::new(),
                })
                .collect(),
            lines: links
                .iter()
                .map(|&(from, to)| Line {
                    from,
                    to,
                    label: "x".into(),
                    kind: LineKind::Relation { directed: true },
                    claims: Vec::new(),
                    status: "active".into(),
                })
                .collect(),
        }
    }

    fn extent(points: &[(f32, f32)]) -> ((f32, f32), (f32, f32)) {
        let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
        for p in points {
            lo = (lo.0.min(p.0), lo.1.min(p.1));
            hi = (hi.0.max(p.0), hi.1.max(p.1));
        }
        (lo, hi)
    }

    #[test]
    fn clusters_and_unconnected_nodes_are_laid_close_together_and_do_not_overlap() {
        // Two clusters (a chain of six, a star of four) and twelve nodes with no line.
        let mut links: Vec<(usize, usize)> = (0..5).map(|i| (i, i + 1)).collect();
        links.extend([(6, 7), (6, 8), (6, 9)]);
        let m = bare(22, &links);
        let pos = layout(&m, &HashMap::new());
        assert_eq!(pos.len(), 22);
        assert!(pos.iter().all(|(x, y)| x.is_finite() && y.is_finite()));
        let (lo, hi) = extent(&pos);
        let (w, h) = (hi.0 - lo.0 + NODE_W, hi.1 - lo.1 + NODE_H);
        // The whole picture is not much bigger than the nodes in it: nothing flies off.
        let boxes = 22.0 * NODE_W * NODE_H;
        assert!(w * h < boxes * 12.0, "{w} x {h} for {boxes} of nodes");
        assert!(w.max(h) < 1800.0, "{w} x {h}");
        // Nodes of different groups never overlap.
        let group = |i: usize| match i {
            0..=5 => 0,
            6..=9 => 1,
            _ => 2 + i,
        };
        for a in 0..22 {
            for b in (a + 1)..22 {
                if group(a) == group(b) {
                    continue;
                }
                let overlap =
                    (pos[a].0 - pos[b].0).abs() < NODE_W && (pos[a].1 - pos[b].1).abs() < NODE_H;
                assert!(!overlap, "{a} and {b} overlap");
            }
        }
        // And it is the same every time.
        assert_eq!(pos, layout(&m, &HashMap::new()));
    }

    #[test]
    fn a_new_node_with_no_line_is_put_beside_what_is_there_not_far_off() {
        let m = bare(5, &[(0, 1), (1, 2), (2, 3)]);
        let first = layout(&m, &HashMap::new());
        let known: HashMap<String, (f32, f32)> = m
            .nodes
            .iter()
            .zip(&first)
            .map(|(n, p)| (n.id.clone(), *p))
            .collect();
        // Node 4 is not connected to anything and has no place yet.
        let mut known = known;
        known.remove("n4");
        let grown = layout(&m, &known);
        for (i, node) in m.nodes.iter().enumerate() {
            if let Some(old) = known.get(&node.id) {
                assert_eq!(grown[i], *old, "{} moved", node.label);
            }
        }
        let placed: Vec<(f32, f32)> = grown[..4].to_vec();
        let (lo, hi) = extent(&placed);
        let new = grown[4];
        assert!(new.1 > hi.1, "below what is there");
        assert!(
            new.1 - hi.1 < NODE_H + GAP + 20.0,
            "close: {} below",
            new.1 - hi.1
        );
        assert!(
            new.0 >= lo.0 - NODE_W && new.0 <= hi.0 + NODE_W,
            "under it, not off to a side"
        );
    }
}
