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
    /// What it holds in full: an entity's whole value, an annotation's whole text. Empty
    /// for a claim that has no value.
    pub text: String,
    /// What the element is in the graph: an event and an observation are both claims here.
    pub element: Kind,
    /// Its properties, as written.
    pub props: Vec<(String, String)>,
    /// A hypothesis's standing: open, supported or rejected.
    pub hypothesis: Option<String>,
    /// When it happened or began to hold, as recorded.
    pub when: Option<String>,
    /// How many things say they support it, and refute it.
    pub supports: usize,
    pub refutes: usize,
    /// How many things it points at by role (a claim's participants, what a note is about).
    pub roles: usize,
    /// How many lines are drawn to or from it.
    pub links: usize,
}

impl Node {
    /// A node with nothing known about it but what it is: for tests and for building up.
    pub fn bare(id: &str, kind: NodeKind, type_: &str, label: &str) -> Node {
        Node {
            id: id.into(),
            kind,
            type_: type_.into(),
            label: label.into(),
            status: "active".into(),
            confidence: None,
            by_agent: false,
            tags: Vec::new(),
            text: label.into(),
            element: match kind {
                NodeKind::Entity => Kind::Entity,
                NodeKind::Claim => Kind::Event,
                NodeKind::Annotation => Kind::Annotation,
            },
            props: Vec::new(),
            hypothesis: None,
            when: None,
            supports: 0,
            refutes: 0,
            roles: 0,
            links: 0,
        }
    }
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
    /// How sure the relation is, where its claims agree on it.
    pub confidence: Option<String>,
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

/// What a bundle gathers: its claims, the worst status among them, and their confidences.
type Bundle = (Vec<String>, String, Vec<Option<String>>);

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
        let told = |role: &str| {
            edges
                .iter()
                .filter(|g| g.role == role && g.to_id == e.id && drawn(&g.from_id))
                .count()
        };
        let mut hypothesis = None;
        let mut props = Vec::new();
        for (key, value) in &e.content.props {
            let value = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if key == "hypothesis_status" {
                hypothesis = Some(value);
            } else {
                props.push((key.clone(), value));
            }
        }
        let time = &e.content.time;
        model.nodes.push(Node {
            id: e.id.clone(),
            kind,
            type_: e.type_.clone(),
            label,
            status: e.content.status.clone(),
            confidence: e.content.confidence.clone(),
            by_agent: e.author.starts_with("agent:"),
            tags: e.content.tags.clone(),
            text: e.content.value.clone().unwrap_or_default(),
            element: e.kind,
            props,
            hypothesis,
            when: time.occurred_at.clone().or_else(|| time.valid_from.clone()),
            supports: told("supports"),
            refutes: told("refutes"),
            roles: out.get(e.id.as_str()).map_or(0, Vec::len),
            links: 0,
        });
    }

    // Collapsed claims, bundled by (from, to, type).
    let mut bundles: BTreeMap<BundleKey, Bundle> = BTreeMap::new();
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
            .or_insert_with(|| (Vec::new(), "active".into(), Vec::new()));
        entry.0.push(e.id.clone());
        entry.1 = worse(&entry.1, &e.content.status);
        entry.2.push(e.content.confidence.clone());
    }
    for ((from, to, type_, directed), (claims, status, sure)) in bundles {
        // One confidence for the line only where every claim of it says the same.
        let confidence = match sure.split_first() {
            Some((first, rest)) if rest.iter().all(|c| c == first) => first.clone(),
            _ => None,
        };
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
            confidence,
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
                confidence: None,
            });
        }
    }
    for i in 0..model.lines.len() {
        let (from, to) = (model.lines[i].from, model.lines[i].to);
        model.nodes[from].links += 1;
        if to != from {
            model.nodes[to].links += 1;
        }
    }
    model
}

// -- layout -------------------------------------------------------------------------------

/// The space kept between the edges of two nodes joined by a line, in pixels at zoom 1: a
/// line's label and its arrowhead sit in it. Nodes are different sizes, so it is the edges
/// that are this far apart, not the centres.
pub const CLEAR: f32 = 100.0;

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

/// The usual size of a node, in pixels at zoom 1, for tests: layouts are given the size each
/// node is drawn at.
#[cfg(test)]
pub(super) const NODE_W: f32 = 170.0;
#[cfg(test)]
pub(super) const NODE_H: f32 = 44.0;
/// The space between two groups of nodes that are not connected.
pub(super) const GAP: f32 = 90.0;

/// How far the edge of a box is from its centre, going out along `u` (a unit vector).
pub(super) fn reach(size: (f32, f32), u: (f32, f32)) -> f32 {
    let along = |half: f32, c: f32| {
        if c.abs() > 1e-6 {
            half / c.abs()
        } else {
            f32::MAX
        }
    };
    along(size.0 / 2.0, u.0).min(along(size.1 / 2.0, u.1))
}

/// Force-directed layout (Fruchterman–Reingold) of one connected group: connected nodes
/// pull together, every pair pushes apart. `edges`, `pos` and `sizes` are indices into the
/// group. The forces act on the space between the *edges* of two boxes, so that a wide node
/// and a tall one keep as much air round them as two small ones do.
fn spring(
    pos: &mut [(f32, f32)],
    sizes: &[(f32, f32)],
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
                let u = (dx / d, dy / d);
                let space = d - reach(sizes[a], u) - reach(sizes[b], u);
                let f = CLEAR * CLEAR / space.max(1.0);
                let (fx, fy) = (u.0 * f, u.1 * f);
                push[a].0 += fx;
                push[a].1 += fy;
                push[b].0 -= fx;
                push[b].1 -= fy;
            }
        }
        for &(from, to) in edges {
            let (dx, dy) = (pos[from].0 - pos[to].0, pos[from].1 - pos[to].1);
            let d = (dx * dx + dy * dy).sqrt().max(0.01);
            let u = (dx / d, dy / d);
            let space = (d - reach(sizes[from], u) - reach(sizes[to], u)).max(0.0);
            let f = space * space / CLEAR;
            let (fx, fy) = (u.0 * f, u.1 * f);
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

/// A group laid out alone, waiting to be packed: its nodes and where their centres are
/// relative to one another.
pub(super) type Loose = (Vec<usize>, Vec<(f32, f32)>);

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
///
/// `sizes` are the whole widths and heights the nodes are drawn at, in the order of the
/// model's nodes: the room each takes is its own.
pub fn layout(
    model: &Model,
    known: &HashMap<String, (f32, f32)>,
    sizes: &[(f32, f32)],
) -> Vec<(f32, f32)> {
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
    let mut settled: Vec<usize> = Vec::new();
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
        let here: Vec<(f32, f32)> = members.iter().map(|&i| sizes[i]).collect();
        // One node's share of the plane: its own size and the air round it.
        let pitch = CLEAR + here.iter().map(|s| (s.0 * s.1).sqrt()).sum::<f32>() / m as f32;
        let spread = pitch * (m as f32).sqrt();
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
                    Some(&j) => (pos[j].0 + sx * pitch, pos[j].1 + sy * pitch),
                    None => (sx * spread * 0.5, sy * spread * 0.5),
                }
            })
            .collect();
        spring(&mut local_pos, &here, &fixed_here, &edges, &ids, spread);
        if any_fixed {
            for (k, &i) in members.iter().enumerate() {
                pos[i] = local_pos[k];
            }
            settled.extend(members.iter().copied());
        } else {
            loose.push((members.clone(), local_pos));
        }
    }
    place_loose(&loose, &settled, &mut pos, sizes, GAP);
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
/// `sizes` are the whole sizes of the nodes (all of them, by index; `settled` are indices
/// whose place is already in `pos`) and `gap` the space between groups.
pub(super) fn place_loose(
    loose: &[Loose],
    settled: &[usize],
    pos: &mut [(f32, f32)],
    sizes: &[(f32, f32)],
    gap: f32,
) {
    if loose.is_empty() {
        return;
    }
    // The box round a group of boxes: its top-left and bottom-right corners.
    let hull = |nodes: &[usize], at: &dyn Fn(usize) -> (f32, f32)| {
        let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
        for (k, &i) in nodes.iter().enumerate() {
            let (p, s) = (at(k), sizes[i]);
            lo = (lo.0.min(p.0 - s.0 / 2.0), lo.1.min(p.1 - s.1 / 2.0));
            hi = (hi.0.max(p.0 + s.0 / 2.0), hi.1.max(p.1 + s.1 / 2.0));
        }
        (lo, hi)
    };
    let hulls: Vec<((f32, f32), (f32, f32))> = loose
        .iter()
        .map(|(members, local_pos)| hull(members, &|k| local_pos[k]))
        .collect();
    // Pack the loose groups: side by side, close, wider than tall.
    let boxes: Vec<(f32, f32)> = hulls
        .iter()
        .map(|(lo, hi)| (hi.0 - lo.0, hi.1 - lo.1))
        .collect();
    let offsets = pack(&boxes, gap);
    // Where the packed block goes: centred on the origin if nothing is placed, else in
    // rows below what is.
    let (block_w, block_h) = offsets
        .iter()
        .zip(&boxes)
        .fold((0.0f32, 0.0f32), |(w, h), (o, s)| {
            (w.max(o.0 + s.0), h.max(o.1 + s.1))
        });
    let origin = if settled.is_empty() {
        (-block_w / 2.0, -block_h / 2.0)
    } else {
        let (lo, hi) = hull(settled, &|k| pos[settled[k]]);
        (lo.0, hi.1 + gap)
    };
    for (((members, local_pos), (lo, _)), off) in loose.iter().zip(&hulls).zip(&offsets) {
        for (k, &i) in members.iter().enumerate() {
            pos[i] = (
                origin.0 + off.0 + (local_pos[k].0 - lo.0),
                origin.1 + off.1 + (local_pos[k].1 - lo.1),
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

/// The confidence scale as seeded, weakest first: what `conf:` is read against when the
/// case's own scale is not at hand.
pub const CONFIDENCE: [&str; 7] = [
    "almost no chance",
    "very unlikely",
    "unlikely",
    "roughly even",
    "likely",
    "very likely",
    "almost certain",
];

/// How a confidence is compared with the one asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cmp {
    Lt,
    Le,
    Eq,
    Ge,
    Gt,
}

/// What one word of a filter asks. Where a word gives alternatives (`tag:a|b`), any of them
/// will do.
#[derive(Debug, Clone, PartialEq)]
enum Ask {
    /// In the label, the type or the whole text.
    Text(String),
    /// `type:net.` — the type starts with it, or a part of it does (`ipv4` finds `net.ipv4`).
    Type(Vec<String>),
    /// `tag:phase.` — a tag that is or starts with it.
    Tag(Vec<String>),
    /// `by:agent` or `by:person`.
    By { agent: bool },
    /// `status:conflicting` (or `disputed`), `status:soft-deleted` (or `deleted`).
    Status(Vec<String>),
    /// `kind:entity`, `kind:claim` or `kind:note`.
    Kind(NodeKind),
    /// `conf:>=likely`, `conf:<5`, `conf:likely`; `conf:none` is what has no confidence. The
    /// number is the place on the scale, from 1.
    Conf(Option<(Cmp, usize)>),
    /// `hyp:open`, `hyp:supported`, `hyp:rejected`.
    Hypothesis(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
struct Term {
    /// A leading `-`: the node must not match.
    not: bool,
    ask: Ask,
}

/// Which nodes to show, as typed: words that must all hold. A bare word is looked for in
/// the label, the type and the text; `type:`, `tag:`, `by:`, `status:`, `kind:`, `conf:` and
/// `hyp:` ask about those; `a|b` after one of them is either; a leading `-` negates. Case is
/// ignored.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    terms: Vec<Term>,
    /// The confidence scale `conf:` was read against, weakest first.
    scale: Vec<String>,
}

fn alternatives(v: &str) -> Vec<String> {
    v.split('|')
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .collect()
}

/// A place on the scale, from 1: its number, or its name with `-` or `_` for the spaces.
fn place(scale: &[String], v: &str) -> Option<usize> {
    if let Ok(n) = v.parse::<usize>() {
        return (1..=scale.len()).contains(&n).then_some(n);
    }
    let name = v.replace(['-', '_'], " ");
    scale
        .iter()
        .position(|s| s.to_lowercase() == name)
        .map(|i| i + 1)
}

fn confidence(scale: &[String], v: &str) -> Option<Ask> {
    if v == "none" {
        return Some(Ask::Conf(None));
    }
    let (cmp, rest) = [
        (">=", Cmp::Ge),
        ("<=", Cmp::Le),
        (">", Cmp::Gt),
        ("<", Cmp::Lt),
        ("=", Cmp::Eq),
    ]
    .into_iter()
    .find_map(|(op, cmp)| v.strip_prefix(op).map(|rest| (cmp, rest)))
    .unwrap_or((Cmp::Eq, v));
    Some(Ask::Conf(Some((cmp, place(scale, rest)?))))
}

/// Read a filter from what is typed. A word that asks about something unknown (`foo:bar`)
/// is looked for as text, as typed.
pub fn parse_filter(text: &str) -> Filter {
    let scale: Vec<String> = CONFIDENCE.iter().map(|s| s.to_string()).collect();
    parse_filter_on(text, &scale)
}

/// [`parse_filter`], reading `conf:` against the case's own scale.
pub fn parse_filter_on(text: &str, scale: &[String]) -> Filter {
    let terms = text
        .split_whitespace()
        .filter_map(|word| {
            let (not, word) = match word.strip_prefix('-') {
                Some(rest) if !rest.is_empty() => (true, rest),
                _ => (false, word),
            };
            let lower = word.to_lowercase();
            let status = |v: &str| {
                alternatives(v)
                    .into_iter()
                    .map(|s| match s.as_str() {
                        "disputed" => "conflicting".to_string(),
                        "deleted" => "soft-deleted".to_string(),
                        _ => s,
                    })
                    .collect()
            };
            let ask = match lower.split_once(':') {
                Some(("type", v)) if !v.is_empty() => Ask::Type(alternatives(v)),
                Some(("tag", v)) if !v.is_empty() => Ask::Tag(alternatives(v)),
                Some(("status", v)) if !v.is_empty() => Ask::Status(status(v)),
                Some(("hyp" | "hypothesis", v)) if !v.is_empty() => {
                    Ask::Hypothesis(alternatives(v))
                }
                Some(("conf" | "confidence", v)) if !v.is_empty() => match confidence(scale, v) {
                    Some(ask) => ask,
                    None => Ask::Text(lower),
                },
                Some(("by", "agent")) => Ask::By { agent: true },
                Some(("by", "person" | "analyst" | "user" | "me")) => Ask::By { agent: false },
                Some(("kind", "entity")) => Ask::Kind(NodeKind::Entity),
                Some(("kind", "claim" | "event" | "observation")) => Ask::Kind(NodeKind::Claim),
                Some(("kind", "note" | "annotation")) => Ask::Kind(NodeKind::Annotation),
                _ if lower.is_empty() => return None,
                _ => Ask::Text(lower),
            };
            Some(Term { not, ask })
        })
        .collect();
    Filter {
        terms,
        scale: scale.to_vec(),
    }
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
                Ask::Text(w) => {
                    node.label.to_lowercase().contains(w)
                        || type_.contains(w)
                        || node.text.to_lowercase().contains(w)
                }
                Ask::Type(any) => any
                    .iter()
                    .any(|p| type_.starts_with(p) || type_.contains(&format!(".{p}"))),
                Ask::Tag(any) => any
                    .iter()
                    .any(|p| node.tags.iter().any(|g| g.to_lowercase().starts_with(p))),
                Ask::By { agent } => node.by_agent == *agent,
                Ask::Status(any) => any.contains(&node.status.to_lowercase()),
                Ask::Kind(k) => node.kind == *k,
                Ask::Hypothesis(any) => node
                    .hypothesis
                    .as_ref()
                    .is_some_and(|h| any.contains(&h.to_lowercase())),
                Ask::Conf(asked) => {
                    let has = node.confidence.as_ref().and_then(|c| {
                        let c = c.to_lowercase();
                        self.scale.iter().position(|s| s.to_lowercase() == c)
                    });
                    match (asked, has) {
                        (None, has) => has.is_none() && node.confidence.is_none(),
                        (Some(_), None) => false,
                        (Some((cmp, place)), Some(i)) => {
                            let at = i + 1;
                            match cmp {
                                Cmp::Lt => at < *place,
                                Cmp::Le => at <= *place,
                                Cmp::Eq => at == *place,
                                Cmp::Ge => at >= *place,
                                Cmp::Gt => at > *place,
                            }
                        }
                    }
                }
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

// -- room ----------------------------------------------------------------------------------------

/// Move boxes apart until none overlaps another, moving each as little as that takes: where
/// two overlap they part along the side they overlap least on. `pos` are centres and `sizes`
/// whole widths and heights, in the same units; `gap` is kept between any two. What is
/// `fixed` stays where it is and the other gives way. Nodes are drawn at the size of what is
/// written on them, which a layout that thinks of points cannot know.
pub fn separate(pos: &mut [(f32, f32)], sizes: &[(f32, f32)], fixed: &[bool], gap: f32) {
    let n = pos.len();
    if n < 2 || fixed.iter().all(|f| *f) {
        return;
    }
    // Boxes exactly on top of one another have no side to part along: each after the first
    // steps a little way off, round a spiral, so that the same pile always opens the same way.
    let mut seen: HashMap<(i64, i64), usize> = HashMap::new();
    for i in 0..n {
        let key = ((pos[i].0 * 10.0) as i64, (pos[i].1 * 10.0) as i64);
        let k = seen.entry(key).or_insert(0);
        if *k > 0 && !fixed[i] {
            let angle = *k as f32 * 2.399_963;
            let r = 4.0 * (*k as f32).sqrt();
            pos[i] = (pos[i].0 + angle.cos() * r, pos[i].1 + angle.sin() * r);
        }
        *k += 1;
    }
    let mut order: Vec<usize> = (0..n).collect();
    // A pile opens from its ends inward, so it takes rounds in proportion to how many are in it.
    for _ in 0..(60 + 30 * n).min(1500) {
        // Sweep from the left: only boxes whose spans across overlap can touch.
        order.sort_by(|&a, &b| {
            (pos[a].0 - sizes[a].0 / 2.0)
                .total_cmp(&(pos[b].0 - sizes[b].0 / 2.0))
                .then(a.cmp(&b))
        });
        let mut moved = false;
        for i in 0..n {
            let a = order[i];
            let right = pos[a].0 + sizes[a].0 / 2.0 + gap;
            for &b in &order[i + 1..] {
                if pos[b].0 - sizes[b].0 / 2.0 >= right {
                    break;
                }
                if fixed[a] && fixed[b] {
                    continue;
                }
                let (dx, dy) = (pos[b].0 - pos[a].0, pos[b].1 - pos[a].1);
                let over_x = (sizes[a].0 + sizes[b].0) / 2.0 + gap - dx.abs();
                let over_y = (sizes[a].1 + sizes[b].1) / 2.0 + gap - dy.abs();
                if over_x <= 0.01 || over_y <= 0.01 {
                    continue;
                }
                // Along the side they overlap least on; a tie in position parts them by
                // which two they are, so the same graph gives the same picture.
                let (push_x, push_y) = if over_x < over_y {
                    (if dx < 0.0 { -over_x } else { over_x }, 0.0)
                } else {
                    (
                        0.0,
                        if dy < 0.0 || (dy == 0.0 && a > b) {
                            -over_y
                        } else {
                            over_y
                        },
                    )
                };
                let (share_a, share_b) = match (fixed[a], fixed[b]) {
                    (true, _) => (0.0, 1.0),
                    (_, true) => (1.0, 0.0),
                    _ => (0.5, 0.5),
                };
                pos[a].0 -= push_x * share_a;
                pos[a].1 -= push_y * share_a;
                pos[b].0 += push_x * share_b;
                pos[b].1 += push_y * share_b;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
}

// -- progressive disclosure --------------------------------------------------------------------

/// How much of the graph is drawn: the concept's three levels. Each has everything of the one
/// before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Detail {
    /// Entities, labeled lines and notes.
    Basic,
    /// And what an assessment adds: status and confidence, hypotheses and what supports or
    /// refutes them.
    Analysis,
    /// And the structure itself: claims as nodes with a line for each role.
    Full,
}

impl Detail {
    pub fn label(self) -> &'static str {
        match self {
            Detail::Basic => "Basic",
            Detail::Analysis => "Analysis",
            Detail::Full => "Full",
        }
    }

    /// What it shows, in a sentence (a tooltip).
    pub fn tip(self) -> &'static str {
        match self {
            Detail::Basic => "Entities, labeled lines and notes",
            Detail::Analysis => {
                "And status, confidence, hypotheses and what supports or refutes them"
            }
            Detail::Full => "And claims as nodes, with a line for each role",
        }
    }

    /// The level that follows from how far the view is zoomed: out, the overview; in, the
    /// detail. (Used when the person has not picked one.)
    pub fn for_zoom(zoom: f32) -> Detail {
        if zoom < 0.45 {
            Detail::Basic
        } else if zoom < 0.9 {
            Detail::Analysis
        } else {
            Detail::Full
        }
    }
}

/// The lowest level at which a node is drawn.
fn level_of(node: &Node) -> Detail {
    match node.kind {
        NodeKind::Entity => Detail::Basic,
        NodeKind::Annotation if node.type_ == "analysis.note" => Detail::Basic,
        NodeKind::Annotation => Detail::Analysis,
        NodeKind::Claim => Detail::Full,
    }
}

/// What a level draws of a model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Disclosure {
    /// Which nodes are drawn.
    pub shown: Vec<bool>,
    /// Lines to draw that the model does not have, standing in for what is not drawn: a claim
    /// that is a node only because something is said about it becomes the plain line between
    /// the two things it relates, and a note about it attaches to one of them.
    pub extra: Vec<Line>,
    /// Status and confidence are not drawn: the level is the plain picture.
    pub plain: bool,
}

/// What `detail` draws of `model`. A claim that is not drawn as a node still shows as a line if
/// it relates exactly two shown things, so that hiding detail never hides *that two things are
/// related*; one with more participants is detail, and goes with the nodes.
pub fn disclose(model: &Model, detail: Detail) -> Disclosure {
    let shown: Vec<bool> = model.nodes.iter().map(|n| level_of(n) <= detail).collect();
    let mut extra = Vec::new();
    // The participants of each claim that is not drawn.
    let mut standing_in: HashMap<usize, (usize, usize)> = HashMap::new();
    for (i, node) in model.nodes.iter().enumerate() {
        if shown[i] || node.kind != NodeKind::Claim {
            continue;
        }
        let roles: Vec<&Line> = model
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Role && l.from == i)
            .collect();
        let mut ends: Vec<(usize, &str)> = roles.iter().map(|l| (l.to, l.label.as_str())).collect();
        ends.sort_unstable();
        ends.dedup_by_key(|e| e.0);
        let [(a, role_a), (b, role_b)] = ends.as_slice() else {
            continue;
        };
        if !(shown[*a] && shown[*b]) {
            continue;
        }
        // Drawn from the one whose role says so (`src`, `actor`, …), as a collapsed claim is.
        let from_a = FROM_ROLES.contains(role_a);
        let from_b = FROM_ROLES.contains(role_b);
        let (from, to, directed) = match (from_a, from_b) {
            (true, false) => (*a, *b, true),
            (false, true) => (*b, *a, true),
            _ => (*a, *b, false),
        };
        standing_in.insert(i, (from, to));
        extra.push(Line {
            from,
            to,
            label: node.type_.clone(),
            kind: LineKind::Relation { directed },
            claims: vec![node.id.clone()],
            status: node.status.clone(),
            confidence: node.confidence.clone(),
        });
    }
    // A note about such a claim attaches to the thing the line starts from.
    for l in &model.lines {
        if l.kind == LineKind::Annotation
            && shown[l.from]
            && !shown[l.to]
            && let Some(&(from, _)) = standing_in.get(&l.to)
        {
            extra.push(Line {
                to: from,
                ..l.clone()
            });
        }
    }
    Disclosure {
        shown,
        extra,
        plain: detail == Detail::Basic,
    }
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
    /// The members' boxes, each a centre and half its size either way, for the outline that
    /// wraps them ([`padded_hull`]).
    pub boxes: Vec<Frame>,
}

/// A node's box: its centre, and half its width and height.
pub type Frame = ((f32, f32), (f32, f32));

/// A group for each tag that two or more of the shown nodes have, the most shared first, at
/// most `limit` of them. Boxes may overlap: a node with several tags is in several. `sizes`
/// are the whole widths and heights the nodes are drawn at, in the order of the model's nodes.
pub fn tag_groups(
    model: &Model,
    positions: &HashMap<String, (f32, f32)>,
    sizes: &[(f32, f32)],
    shown: &[bool],
    limit: usize,
) -> Vec<Group> {
    let mut by_tag: BTreeMap<&str, Vec<Frame>> = BTreeMap::new();
    for ((node, show), size) in model.nodes.iter().zip(shown).zip(sizes) {
        if !show {
            continue;
        }
        let Some(p) = positions.get(&node.id) else {
            continue;
        };
        for tag in &node.tags {
            by_tag
                .entry(tag.as_str())
                .or_default()
                .push((*p, (size.0 / 2.0, size.1 / 2.0)));
        }
    }
    let mut groups: Vec<Group> = by_tag
        .into_iter()
        .filter(|(_, boxes)| boxes.len() >= 2)
        .map(|(tag, boxes)| {
            let (mut min, mut max) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
            for (p, _) in &boxes {
                min = (min.0.min(p.0), min.1.min(p.1));
                max = (max.0.max(p.0), max.1.max(p.1));
            }
            Group {
                tag: tag.to_string(),
                members: boxes.len(),
                min,
                max,
                boxes,
            }
        })
        .collect();
    groups.sort_by(|a, b| b.members.cmp(&a.members).then(a.tag.cmp(&b.tag)));
    groups.truncate(limit);
    groups
}

/// The convex hull of some points: the corners of the smallest convex shape that holds them,
/// in order round it (Andrew's monotone chain). Fewer than three distinct points are
/// returned as they are.
pub fn convex_hull(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let mut pts: Vec<(f32, f32)> = points
        .iter()
        .copied()
        .filter(|p| p.0.is_finite() && p.1.is_finite())
        .collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: (f32, f32), a: (f32, f32), b: (f32, f32)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let mut lower: Vec<(f32, f32)> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<(f32, f32)> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// The outline of a group: the convex hull of the boxes of its nodes (each a centre and half
/// its own size either way from it), standing `pad` off them all round, with rounded corners.
/// Every corner of every box gets a quarter circle of radius `pad`; the hull of those is the
/// outline, and it is smooth where a plain hull of the centres would have points.
pub fn padded_hull(boxes: &[Frame], pad: f32) -> Vec<(f32, f32)> {
    const STEPS: usize = 6;
    let mut points = Vec::with_capacity(boxes.len() * 4 * (STEPS + 1));
    for &((cx, cy), half) in boxes {
        for (sx, sy, from_angle) in [
            (1.0f32, 1.0f32, 0.0f32),
            (-1.0, 1.0, 90.0),
            (-1.0, -1.0, 180.0),
            (1.0, -1.0, 270.0),
        ] {
            let corner = (cx + sx * half.0, cy + sy * half.1);
            for k in 0..=STEPS {
                let a = (from_angle + 90.0 * k as f32 / STEPS as f32).to_radians();
                points.push((corner.0 + pad * a.cos(), corner.1 + pad * a.sin()));
            }
        }
    }
    convex_hull(&points)
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
    if let Ok(r) = reactor_db::reference::parse(uri) {
        match r.target {
            reactor_db::reference::Target::Session { session, entry } => {
                return SourceTarget::Session { id: session, entry };
            }
            reactor_db::reference::Target::Artifact { path, locator: own } => {
                return SourceTarget::File {
                    path,
                    line: locator.or(own.as_deref()).and_then(line_in),
                };
            }
            _ => {}
        }
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
                    Link::source("artifact:vpn.csv"),
                ],
            ))
            .put(claim(
                Kind::Event,
                "net.connection",
                vec![
                    Link::entity("src", "host.name", "ws-042"),
                    Link::entity("dst", "net.ipv4", "198.51.100.7"),
                    Link::source("artifact:fw.csv"),
                ],
            ))
            .put(claim(
                Kind::Observation,
                "net.resolution",
                vec![
                    Link::entity("query", "net.domain", "cdn-sync.example"),
                    Link::entity("answer", "net.ipv4", "198.51.100.7"),
                    Link::source("artifact:pdns.csv").at("row=1", None),
                ],
            ))
            .put(claim(
                Kind::Observation,
                "net.resolution",
                vec![
                    Link::entity("query", "net.domain", "cdn-sync.example"),
                    Link::entity("answer", "net.ipv4", "198.51.100.7"),
                    Link::source("artifact:pdns.csv").at("row=2", None),
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
                        Link::source("artifact:edr.jsonl"),
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
        let first = layout(&m, &HashMap::new(), &flat(&m));
        assert_eq!(
            first,
            layout(&m, &HashMap::new(), &flat(&m)),
            "the same graph, the same picture"
        );
        assert!(first.iter().all(|(x, y)| x.is_finite() && y.is_finite()));
        for a in 0..first.len() {
            for b in (a + 1)..first.len() {
                let d =
                    ((first[a].0 - first[b].0).powi(2) + (first[a].1 - first[b].1).powi(2)).sqrt();
                assert!(
                    d > CLEAR * 0.4,
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
                    Link::source("artifact:vpn.csv").at("row=9", None),
                ],
            )),
        )
        .unwrap();
        let grown = model(&case);
        let second = layout(&grown, &known, &flat(&grown));
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
            dist(new, at("ws-042")) < (NODE_W + CLEAR) * 2.5,
            "{}",
            dist(new, at("ws-042"))
        );
        assert!(layout(&Model::default(), &HashMap::new(), &[]).is_empty());
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
            by_agent: agent,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            ..Node::bare(id, kind, type_, label)
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
    fn a_filter_reads_the_whole_text_alternatives_confidence_and_a_hypothesis_standing() {
        let mut m = sample();
        m.nodes[2].text = "jdoe is compromised\n\nThe beacon window overlaps the logon.".into();
        m.nodes[2].confidence = Some("likely".into());
        m.nodes[2].hypothesis = Some("open".into());
        m.nodes[3].confidence = Some("very unlikely".into());
        m.nodes[3].status = "conflicting".into();
        // A bare word is looked for in all of the text, not only its first line.
        assert_eq!(shown(&m, "beacon"), ["c"]);
        assert_eq!(shown(&m, "tag:c2|phase"), ["a", "b", "d"]);
        assert_eq!(shown(&m, "type:ipv4|hypothesis"), ["a", "c"]);
        assert_eq!(shown(&m, "conf:>=likely"), ["c"]);
        assert_eq!(shown(&m, "conf:<likely"), ["d"]);
        assert_eq!(shown(&m, "conf:very-unlikely"), ["d"]);
        assert_eq!(shown(&m, "conf:<=5"), ["c", "d"]);
        assert_eq!(shown(&m, "conf:none"), ["a", "b"]);
        assert_eq!(shown(&m, "-conf:none -conf:>4"), ["d"]);
        assert_eq!(shown(&m, "hyp:open|supported"), ["c"]);
        assert_eq!(shown(&m, "hyp:rejected"), Vec::<String>::new());
        assert_eq!(shown(&m, "status:disputed"), ["d"]);
        assert_eq!(shown(&m, "kind:event"), ["d"]);
        // A confidence that is not on the scale is just text.
        assert_eq!(shown(&m, "conf:maybe"), Vec::<String>::new());
        // The case's own scale is the one `conf:` is read against.
        let own: Vec<String> = ["low", "high"].iter().map(|s| s.to_string()).collect();
        m.nodes[0].confidence = Some("high".into());
        let f = parse_filter_on("conf:>low", &own);
        assert_eq!(visible(&m, &f), [true, false, false, false]);
    }

    #[test]
    fn boxes_that_overlap_are_parted_as_little_as_it_takes_and_fixed_ones_stay() {
        let overlap = |pos: &[(f32, f32)], sizes: &[(f32, f32)], gap: f32| {
            (0..pos.len()).any(|a| {
                (a + 1..pos.len()).any(|b| {
                    (pos[a].0 - pos[b].0).abs() < (sizes[a].0 + sizes[b].0) / 2.0 + gap - 0.5
                        && (pos[a].1 - pos[b].1).abs() < (sizes[a].1 + sizes[b].1) / 2.0 + gap - 0.5
                })
            })
        };
        // A tall note on top of two small nodes, and one far away.
        let sizes = [(400.0, 300.0), (150.0, 48.0), (150.0, 48.0), (150.0, 48.0)];
        let mut pos = [(0.0, 0.0), (60.0, 20.0), (-40.0, -160.0), (2000.0, 0.0)];
        let before = pos;
        assert!(overlap(&pos, &sizes, 16.0));
        separate(&mut pos, &sizes, &[true, false, false, false], 16.0);
        assert!(!overlap(&pos, &sizes, 16.0), "{pos:?}");
        assert_eq!(pos[0], before[0], "what is fixed stays");
        assert_eq!(pos[3], before[3], "what touched nothing stays");
        assert!(pos.iter().all(|p| p.0.is_finite() && p.1.is_finite()));
        // The same again changes nothing, and so does a set with nothing free.
        let settled = pos;
        separate(&mut pos, &sizes, &[true, false, false, false], 16.0);
        assert_eq!(pos, settled);
        let mut stuck = before;
        separate(&mut stuck, &sizes, &[true; 4], 16.0);
        assert_eq!(stuck, before);
        // Many on one spot spread out and stay near it.
        let sizes = vec![(160.0, 50.0); 30];
        let mut pos = vec![(10.0, 10.0); 30];
        separate(&mut pos, &sizes, &[false; 30], 12.0);
        assert!(!overlap(&pos, &sizes, 12.0));
        assert!(
            pos.iter().all(|p| p.0.abs() < 2500.0 && p.1.abs() < 2500.0),
            "{pos:?}"
        );
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
        let sizes = vec![(100., 40.); 4];
        let groups = tag_groups(&m, &pos, &sizes, &all, 10);
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
        assert!(tag_groups(&m, &pos, &sizes, &hide_b, 10).is_empty());
        assert_eq!(tag_groups(&m, &pos, &sizes, &all, 1).len(), 1);
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
                .map(|i| {
                    Node::bare(
                        &format!("n{i}"),
                        NodeKind::Entity,
                        "host.name",
                        &format!("host-{i}"),
                    )
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
                    confidence: None,
                })
                .collect(),
        }
    }

    /// Every node at the usual size.
    fn flat(m: &Model) -> Vec<(f32, f32)> {
        vec![(NODE_W, NODE_H); m.nodes.len()]
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
        let pos = layout(&m, &HashMap::new(), &flat(&m));
        assert_eq!(pos.len(), 22);
        assert!(pos.iter().all(|(x, y)| x.is_finite() && y.is_finite()));
        let (lo, hi) = extent(&pos);
        let (w, h) = (hi.0 - lo.0 + NODE_W, hi.1 - lo.1 + NODE_H);
        // The whole picture is not much bigger than the nodes in it: nothing flies off.
        let boxes = 22.0 * NODE_W * NODE_H;
        assert!(w * h < boxes * 14.0, "{w} x {h} for {boxes} of nodes");
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
        assert_eq!(pos, layout(&m, &HashMap::new(), &flat(&m)));
    }

    #[test]
    fn a_new_node_with_no_line_is_put_beside_what_is_there_not_far_off() {
        let m = bare(5, &[(0, 1), (1, 2), (2, 3)]);
        let first = layout(&m, &HashMap::new(), &flat(&m));
        let known: HashMap<String, (f32, f32)> = m
            .nodes
            .iter()
            .zip(&first)
            .map(|(n, p)| (n.id.clone(), *p))
            .collect();
        // Node 4 is not connected to anything and has no place yet.
        let mut known = known;
        known.remove("n4");
        let grown = layout(&m, &known, &flat(&m));
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

    fn role(from: usize, to: usize, label: &str) -> Line {
        Line {
            from,
            to,
            label: label.into(),
            kind: LineKind::Role,
            claims: Vec::new(),
            status: "active".into(),
            confidence: None,
        }
    }

    /// Entities 0 (ip), 1 (domain), 2 (host); a claim 3 relating 0 and 1 that a hypothesis (5)
    /// and a note (6) are about; a claim 4 with three participants; a note 7 on entity 2.
    fn discussed() -> Model {
        let mut m = bare(3, &[]);
        let mut push = |kind, type_: &str, label: &str, id: &str| {
            m.nodes.push(Node::bare(id, kind, type_, label));
        };
        push(NodeKind::Claim, "net.resolution", "net.resolution", "c3");
        push(NodeKind::Claim, "net.connection", "net.connection", "c4");
        push(
            NodeKind::Annotation,
            "analysis.hypothesis",
            "it is c2",
            "h5",
        );
        push(NodeKind::Annotation, "analysis.note", "ask the admin", "n6");
        push(
            NodeKind::Annotation,
            "analysis.note",
            "a note on the host",
            "n7",
        );
        m.lines = vec![
            role(3, 0, "query"),
            role(3, 1, "answer"),
            role(4, 0, "member"),
            role(4, 1, "member"),
            role(4, 2, "member"),
            Line {
                from: 5,
                to: 3,
                kind: LineKind::Annotation,
                label: "about".into(),
                ..role(5, 3, "")
            },
            Line {
                from: 6,
                to: 3,
                kind: LineKind::Annotation,
                label: "about".into(),
                ..role(6, 3, "")
            },
            Line {
                from: 7,
                to: 2,
                kind: LineKind::Annotation,
                label: "about".into(),
                ..role(7, 2, "")
            },
        ];
        m
    }

    #[test]
    fn each_level_draws_what_the_one_before_it_does_and_more() {
        let m = discussed();
        let names = |d: &Disclosure| -> Vec<&str> {
            m.nodes
                .iter()
                .zip(&d.shown)
                .filter(|(_, s)| **s)
                .map(|(n, _)| n.id.as_str())
                .collect()
        };
        let basic = disclose(&m, Detail::Basic);
        assert_eq!(
            names(&basic),
            ["n0", "n1", "n2", "n6", "n7"],
            "entities and notes"
        );
        assert!(basic.plain);
        let analysis = disclose(&m, Detail::Analysis);
        assert_eq!(
            names(&analysis),
            ["n0", "n1", "n2", "h5", "n6", "n7"],
            "and the hypothesis"
        );
        assert!(!analysis.plain);
        let full = disclose(&m, Detail::Full);
        assert_eq!(full.shown, vec![true; 8]);
        assert!(full.extra.is_empty(), "everything is drawn as it is");
        assert!(Detail::Basic < Detail::Analysis && Detail::Analysis < Detail::Full);
    }

    #[test]
    fn a_claim_that_is_not_drawn_still_shows_that_its_two_things_are_related() {
        let m = discussed();
        for level in [Detail::Basic, Detail::Analysis] {
            let d = disclose(&m, level);
            // The relation is a line between the two entities, from the one whose role says so,
            // standing for the claim, so that clicking it selects the claim.
            let rel = d
                .extra
                .iter()
                .find(|l| matches!(l.kind, LineKind::Relation { .. }))
                .unwrap_or_else(|| panic!("{level:?}: no line for the relation"));
            assert_eq!((rel.from, rel.to), (0, 1), "query to answer");
            assert_eq!(rel.kind, LineKind::Relation { directed: true });
            assert_eq!(rel.label, "net.resolution");
            assert_eq!(rel.claims, ["c3"]);
            // Only the binary one: the claim with three participants is detail.
            assert_eq!(
                d.extra
                    .iter()
                    .filter(|l| matches!(l.kind, LineKind::Relation { .. }))
                    .count(),
                1,
                "{level:?}"
            );
        }
        // A note about that claim is attached to where the line starts, and so is a hypothesis
        // once it is drawn.
        let basic = disclose(&m, Detail::Basic);
        let notes: Vec<(usize, usize)> = basic
            .extra
            .iter()
            .filter(|l| l.kind == LineKind::Annotation)
            .map(|l| (l.from, l.to))
            .collect();
        assert_eq!(notes, [(6, 0)]);
        let analysis = disclose(&m, Detail::Analysis);
        let mut got: Vec<(usize, usize)> = analysis
            .extra
            .iter()
            .filter(|l| l.kind == LineKind::Annotation)
            .map(|l| (l.from, l.to))
            .collect();
        got.sort_unstable();
        assert_eq!(got, [(5, 0), (6, 0)]);
        // Nothing stands in once the claim is drawn.
        assert!(disclose(&m, Detail::Full).extra.is_empty());
    }

    #[test]
    fn the_level_follows_the_zoom_when_none_is_picked() {
        assert_eq!(Detail::for_zoom(0.1), Detail::Basic);
        assert_eq!(Detail::for_zoom(0.6), Detail::Analysis);
        assert_eq!(Detail::for_zoom(1.0), Detail::Full);
        assert_eq!(Detail::for_zoom(3.0), Detail::Full);
    }

    #[test]
    fn a_hull_is_the_corners_of_the_smallest_convex_shape_round_the_points() {
        let square = [
            (0., 0.),
            (10., 0.),
            (10., 10.),
            (0., 10.),
            (5., 5.),
            (5., 0.),
        ];
        let hull = convex_hull(&square);
        assert_eq!(hull.len(), 4, "{hull:?}");
        for corner in [(0., 0.), (10., 0.), (10., 10.), (0., 10.)] {
            assert!(hull.contains(&corner), "{corner:?} in {hull:?}");
        }
        // Fewer than three, or all in a line, make no area.
        assert_eq!(convex_hull(&[(1., 1.)]).len(), 1);
        assert_eq!(convex_hull(&[(0., 0.), (5., 0.)]).len(), 2);
        assert_eq!(convex_hull(&[(0., 0.), (5., 0.), (10., 0.)]).len(), 2);
        assert!(convex_hull(&[]).is_empty());
        assert_eq!(
            convex_hull(&[(0., 0.), (f32::NAN, 1.)]).len(),
            1,
            "what cannot be drawn is left out"
        );
    }

    #[test]
    fn a_padded_hull_is_convex_and_stands_off_every_node_by_the_padding() {
        // Boxes of different sizes: the outline stands off each by its own measure.
        let boxes: [Frame; 4] = [
            ((0., 0.), (110., 24.)),
            ((300., 40.), (60., 24.)),
            ((120., 200.), (80., 90.)),
            ((150., 90.), (110., 30.)),
        ];
        let pad = 40.;
        let hull = padded_hull(&boxes, pad);
        assert!(hull.len() > 8, "smooth, not a box: {} corners", hull.len());
        // Convex: the turns are all the same way.
        let n = hull.len();
        let turns: Vec<f32> = (0..n)
            .map(|i| {
                let (a, b, c) = (hull[i], hull[(i + 1) % n], hull[(i + 2) % n]);
                (b.0 - a.0) * (c.1 - b.1) - (b.1 - a.1) * (c.0 - b.0)
            })
            .collect();
        assert!(
            turns.iter().all(|t| *t > -1e-3) || turns.iter().all(|t| *t < 1e-3),
            "not convex"
        );
        // Every corner of every node's box is inside, with the padding to spare: the distance
        // from it to the nearest edge of the hull is at least `pad` (less a little for the
        // straight edges that stand in for the arc between samples).
        let inside_by = |p: (f32, f32)| {
            (0..n)
                .map(|i| {
                    let (a, b) = (hull[i], hull[(i + 1) % n]);
                    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                    let len = dx.hypot(dy).max(1e-6);
                    // Signed distance to the edge's line, positive on the inside (the hull
                    // runs one way round).
                    ((p.0 - a.0) * dy - (p.1 - a.1) * dx) / len
                })
                .fold(f32::MAX, |m, d| m.min(d.abs()))
        };
        for &((cx, cy), half) in &boxes {
            for (sx, sy) in [(1., 1.), (-1., 1.), (-1., -1.), (1., -1.)] {
                let corner = (cx + sx * half.0, cy + sy * half.1);
                assert!(
                    inside_by(corner) >= pad * 0.97 - 1.0,
                    "{corner:?}: {}",
                    inside_by(corner)
                );
            }
        }
        // One node still has an outline round it.
        assert!(padded_hull(&[((0., 0.), (50., 20.))], pad).len() > 8);
        assert!(padded_hull(&[], pad).is_empty());
    }
}
