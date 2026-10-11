//! The ways the graph can be arranged on the plane, to be chosen from.
//!
//! * **Force-directed** (the default, `model::layout`): connected nodes pull together, every
//!   pair pushes apart. Reads as clusters.
//! * **Hierarchical**, in the manner of Sugiyama: nodes in layers from the sources down,
//!   in an order that keeps lines from crossing where it can. Reads as flow.
//! * **Radial**: the best-connected node in the middle, what it touches on a ring around it,
//!   what that touches on the next. Reads as distance from a centre.
//! * **Orthogonal**: nodes on a grid, and lines that run only up, down and across.
//!
//! All of them lay each connected group out on its own and pack the groups close together,
//! as the default does, and all are reproducible: the same graph gives the same picture.
//! They arrange *everything afresh*, which is what choosing one asks for; what is added later
//! is placed by the default's rules, beside what it connects to.

use std::collections::HashMap;

use super::model::{Loose, Model, NODE_H, connected_groups, layout, place_loose};

/// The widest box a node is drawn in (a note). These arrangements leave this much room for
/// every node, whatever it is, and well over it between neighbours: a line's label and its
/// arrowhead sit between two nodes too, and the default's tighter spacing let boxes touch.
const WIDE: f32 = 220.0;
/// The space between two nodes side by side, and between layers.
const AIR: f32 = 90.0;
/// The space between groups that are not connected.
const SPACE: f32 = 100.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutKind {
    #[default]
    Force,
    Hierarchical,
    Radial,
    Orthogonal,
}

impl LayoutKind {
    pub const ALL: [LayoutKind; 4] = [
        LayoutKind::Force,
        LayoutKind::Hierarchical,
        LayoutKind::Radial,
        LayoutKind::Orthogonal,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LayoutKind::Force => "Force-directed",
            LayoutKind::Hierarchical => "Hierarchical",
            LayoutKind::Radial => "Circular / radial",
            LayoutKind::Orthogonal => "Orthogonal",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            LayoutKind::Force => "force",
            LayoutKind::Hierarchical => "hierarchical",
            LayoutKind::Radial => "radial",
            LayoutKind::Orthogonal => "orthogonal",
        }
    }

    pub fn parse(s: &str) -> Option<LayoutKind> {
        LayoutKind::ALL
            .into_iter()
            .find(|k| k.slug() == s.trim().to_lowercase())
    }

    /// What it is for, in a sentence (a tooltip).
    pub fn tip(self) -> &'static str {
        match self {
            LayoutKind::Force => {
                "Connected nodes pull together: clusters show as clusters (the default)"
            }
            LayoutKind::Hierarchical => {
                "Layers from the sources down, ordered to keep lines from crossing: flow shows as flow"
            }
            LayoutKind::Radial => {
                "The best-connected node in the middle, what it touches on rings around it"
            }
            LayoutKind::Orthogonal => "Nodes on a grid, lines that run only up, down and across",
        }
    }

    /// Whether lines are drawn as right-angled connectors.
    pub fn right_angled(self) -> bool {
        self == LayoutKind::Orthogonal
    }
}

/// Where each node goes, arranged afresh in the given way.
pub fn layout_as(model: &Model, kind: LayoutKind) -> Vec<(f32, f32)> {
    if kind == LayoutKind::Force {
        return layout(model, &HashMap::new());
    }
    let n = model.nodes.len();
    let mut pos = vec![(0.0, 0.0); n];
    let (groups, group_of, _) = connected_groups(model);
    let mut loose: Vec<Loose> = Vec::new();
    for members in &groups {
        let local = |global: usize| members.binary_search(&global).unwrap();
        // The lines, as ends of nodes in this group, `from` to `to` as drawn.
        let edges: Vec<(usize, usize)> = model
            .lines
            .iter()
            .filter(|l| l.from != l.to && group_of[l.from] == group_of[members[0]])
            .map(|l| (local(l.from), local(l.to)))
            .collect();
        let local_pos = match kind {
            LayoutKind::Hierarchical => layered(members.len(), &edges),
            LayoutKind::Radial => radial(members.len(), &edges),
            LayoutKind::Orthogonal => on_grid(members.len(), &edges),
            LayoutKind::Force => unreachable!(),
        };
        let mut lo = (f32::MAX, f32::MAX);
        for p in &local_pos {
            lo = (lo.0.min(p.0), lo.1.min(p.1));
        }
        loose.push((members.clone(), local_pos, lo));
    }
    place_loose(&loose, &[], &mut pos, (WIDE, NODE_H), SPACE);
    pos
}

// -- hierarchical -------------------------------------------------------------------------------

/// The room a node takes along its layer, and between layers.
const STEP: f32 = WIDE + AIR;
const LAYER: f32 = NODE_H + 110.0;

/// Layers: the lines made to run one way (a cycle is broken where the walk finds it), each
/// node on the layer after the last one that points at it, then the order along each layer
/// swept to the average place of its neighbours, and the places along it pulled towards the
/// same.
fn layered(m: usize, edges: &[(usize, usize)]) -> Vec<(f32, f32)> {
    if m == 0 {
        return Vec::new();
    }
    // Out-lines, without repeats, in index order.
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); m];
    for &(a, b) in edges {
        if a != b && !out[a].contains(&b) {
            out[a].push(b);
        }
    }
    for list in &mut out {
        list.sort_unstable();
    }
    // Break cycles: a line to a node still being walked goes the other way.
    let mut state = vec![0u8; m]; // 0 new, 1 being walked, 2 done
    let mut dag: Vec<(usize, usize)> = Vec::new();
    for root in 0..m {
        if state[root] != 0 {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
        state[root] = 1;
        while let Some(&mut (v, ref mut next)) = stack.last_mut() {
            if *next < out[v].len() {
                let w = out[v][*next];
                *next += 1;
                match state[w] {
                    0 => {
                        dag.push((v, w));
                        state[w] = 1;
                        stack.push((w, 0));
                    }
                    1 => dag.push((w, v)),
                    _ => dag.push((v, w)),
                }
            } else {
                state[v] = 2;
                stack.pop();
            }
        }
    }
    dag.sort_unstable();
    dag.dedup();
    // A line that is also its own opposite (a 2-cycle) is one line now: keep one direction.
    let dag: Vec<(usize, usize)> = dag
        .iter()
        .copied()
        .filter(|&(a, b)| a < b || !dag.contains(&(b, a)))
        .collect();

    // Longest path: a node's layer is one past the deepest of those pointing at it.
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); m];
    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); m];
    for &(a, b) in &dag {
        preds[b].push(a);
        succs[a].push(b);
    }
    let mut indegree: Vec<usize> = preds.iter().map(Vec::len).collect();
    let mut layer = vec![0usize; m];
    let mut ready: Vec<usize> = (0..m).filter(|&i| indegree[i] == 0).collect();
    let mut head = 0;
    while head < ready.len() {
        let v = ready[head];
        head += 1;
        for &w in &succs[v] {
            layer[w] = layer[w].max(layer[v] + 1);
            indegree[w] -= 1;
            if indegree[w] == 0 {
                ready.push(w);
            }
        }
    }
    let depth = layer.iter().copied().max().unwrap_or(0) + 1;
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); depth];
    for i in 0..m {
        rows[layer[i]].push(i);
    }

    // Order along each layer: sweep down and up, each node at the average place of its
    // neighbours on the layer it faces; those with none keep their place.
    let mut place = vec![0.0f32; m];
    let renumber = |rows: &Vec<Vec<usize>>, place: &mut Vec<f32>| {
        for row in rows {
            for (k, &v) in row.iter().enumerate() {
                place[v] = k as f32;
            }
        }
    };
    renumber(&rows, &mut place);
    for sweep in 0..12 {
        let down = sweep % 2 == 0;
        let order: Vec<usize> = if down {
            (1..depth).collect()
        } else {
            (0..depth.saturating_sub(1)).rev().collect()
        };
        for r in order {
            let facing = if down { &preds } else { &succs };
            let mut keyed: Vec<(f32, usize, usize)> = rows[r]
                .iter()
                .map(|&v| {
                    let near: Vec<f32> = facing[v].iter().map(|&u| place[u]).collect();
                    let key = if near.is_empty() {
                        place[v]
                    } else {
                        near.iter().sum::<f32>() / near.len() as f32
                    };
                    (key, place[v] as usize, v)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
            rows[r] = keyed.into_iter().map(|k| k.2).collect();
            for (k, &v) in rows[r].iter().enumerate() {
                place[v] = k as f32;
            }
        }
    }

    // Places along the layer: a row's nodes at least a step apart, pulled towards where their
    // neighbours are, down and up a few times.
    let mut x = vec![0.0f32; m];
    for row in &rows {
        let width = (row.len().saturating_sub(1)) as f32 * STEP;
        for (k, &v) in row.iter().enumerate() {
            x[v] = k as f32 * STEP - width / 2.0;
        }
    }
    for sweep in 0..6 {
        let down = sweep % 2 == 0;
        let order: Vec<usize> = if down {
            (0..depth).collect()
        } else {
            (0..depth).rev().collect()
        };
        for r in order {
            let row = &rows[r];
            if row.is_empty() {
                continue;
            }
            let wanted: Vec<f32> = row
                .iter()
                .map(|&v| {
                    let near: Vec<f32> = preds[v].iter().chain(&succs[v]).map(|&u| x[u]).collect();
                    if near.is_empty() {
                        x[v]
                    } else {
                        near.iter().sum::<f32>() / near.len() as f32
                    }
                })
                .collect();
            // Forwards and backwards, each keeping a step between neighbours; the middle of
            // the two keeps it too.
            let mut fwd = wanted.clone();
            for k in 1..fwd.len() {
                fwd[k] = fwd[k].max(fwd[k - 1] + STEP);
            }
            let mut bwd = wanted;
            for k in (0..bwd.len().saturating_sub(1)).rev() {
                bwd[k] = bwd[k].min(bwd[k + 1] - STEP);
            }
            for (k, &v) in row.iter().enumerate() {
                x[v] = (fwd[k] + bwd[k]) / 2.0;
            }
        }
    }
    (0..m).map(|i| (x[i], layer[i] as f32 * LAYER)).collect()
}

// -- radial -------------------------------------------------------------------------------------

/// The distance between rings at least, and the room a node takes along a ring.
const RING: f32 = WIDE + 60.0;
const ARC: f32 = WIDE + 50.0;

/// The most connected node in the middle; a walk outwards from it gives each node its ring;
/// each node gets a slice of the circle as wide as the nodes under it need, and sits in the
/// middle of it. A ring is as far out as its crowding makes it.
fn radial(m: usize, edges: &[(usize, usize)]) -> Vec<(f32, f32)> {
    if m <= 1 {
        return vec![(0.0, 0.0); m];
    }
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); m];
    for &(a, b) in edges {
        if a != b {
            if !adj[a].contains(&b) {
                adj[a].push(b);
            }
            if !adj[b].contains(&a) {
                adj[b].push(a);
            }
        }
    }
    for list in &mut adj {
        list.sort_unstable();
    }
    let root = (0..m)
        .max_by(|&a, &b| adj[a].len().cmp(&adj[b].len()).then(b.cmp(&a)))
        .unwrap_or(0);

    // The walk: each node's ring, and its parent.
    let mut ring = vec![usize::MAX; m];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); m];
    let mut order = vec![root];
    ring[root] = 0;
    let mut head = 0;
    while head < order.len() {
        let v = order[head];
        head += 1;
        for &w in &adj[v] {
            if ring[w] == usize::MAX {
                ring[w] = ring[v] + 1;
                children[v].push(w);
                order.push(w);
            }
        }
    }
    // How many leaves are under each node: what its slice of the circle is proportional to.
    let mut weight = vec![1.0f32; m];
    for &v in order.iter().rev() {
        if !children[v].is_empty() {
            weight[v] = children[v].iter().map(|&c| weight[c]).sum();
        }
    }
    let mut angle = vec![0.0f32; m];
    let mut span = vec![(0.0f32, std::f32::consts::TAU); m];
    for &v in &order {
        let (lo, hi) = span[v];
        angle[v] = (lo + hi) / 2.0;
        let mut at = lo;
        for &c in &children[v] {
            let width = (hi - lo) * weight[c] / weight[v];
            span[c] = (at, at + width);
            at += width;
        }
    }
    // Ring radii: a step out at least, and far enough that the nearest two on it fit.
    let rings = ring.iter().copied().max().unwrap_or(0);
    let mut radius = vec![0.0f32; rings + 1];
    for k in 1..=rings {
        let mut on: Vec<f32> = (0..m).filter(|&v| ring[v] == k).map(|v| angle[v]).collect();
        on.sort_by(f32::total_cmp);
        let mut least_gap = std::f32::consts::TAU;
        for w in on.windows(2) {
            least_gap = least_gap.min(w[1] - w[0]);
        }
        if on.len() > 1 {
            least_gap = least_gap.min(std::f32::consts::TAU - (on[on.len() - 1] - on[0]));
        }
        let crowded = if on.len() > 1 {
            ARC / least_gap.max(1e-3)
        } else {
            0.0
        };
        radius[k] = (radius[k - 1] + RING).max(crowded);
    }
    (0..m)
        .map(|v| {
            if ring[v] == usize::MAX {
                (0.0, 0.0)
            } else {
                let r = radius[ring[v]];
                (r * angle[v].cos(), r * angle[v].sin())
            }
        })
        .collect()
}

// -- orthogonal ---------------------------------------------------------------------------------

/// The layered arrangement, snapped onto a grid: columns a step apart, rows a layer apart,
/// a node that would share a cell moving to the next free one on its row. Lines are then
/// drawn right-angled ([`orthogonal_route`]).
fn on_grid(m: usize, edges: &[(usize, usize)]) -> Vec<(f32, f32)> {
    let flow = layered(m, edges);
    let mut by_row: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, p) in flow.iter().enumerate() {
        by_row
            .entry((p.1 / LAYER).round() as i64)
            .or_default()
            .push(i);
    }
    let mut out = vec![(0.0f32, 0.0f32); m];
    for (row, mut nodes) in by_row {
        nodes.sort_by(|&a, &b| flow[a].0.total_cmp(&flow[b].0).then(a.cmp(&b)));
        let mut taken: i64 = i64::MIN;
        for v in nodes {
            let wanted = (flow[v].0 / STEP).round() as i64;
            let col = wanted.max(taken.saturating_add(1));
            taken = col;
            out[v] = (col as f32 * STEP, row as f32 * LAYER);
        }
    }
    out
}

/// A right-angled connector between two boxes: where it leaves, its two bends, and where it
/// arrives. Between boxes in different rows it goes out of the bottom (or top), across at
/// mid-height and into the top (or bottom); between two in a row, up over them.
pub fn orthogonal_route(
    a: (f32, f32),
    a_half: (f32, f32),
    b: (f32, f32),
    b_half: (f32, f32),
) -> [(f32, f32); 4] {
    let gap = (b.1 - a.1).abs() - (a_half.1 + b_half.1);
    if gap > 1.0 {
        let down = b.1 > a.1;
        let (start, end) = if down {
            (a.1 + a_half.1, b.1 - b_half.1)
        } else {
            (a.1 - a_half.1, b.1 + b_half.1)
        };
        let mid = (start + end) / 2.0;
        [(a.0, start), (a.0, mid), (b.0, mid), (b.0, end)]
    } else {
        let top = (a.1 - a_half.1).min(b.1 - b_half.1) - 28.0 * (a_half.1 / 10.0).max(0.3);
        [
            (a.0, a.1 - a_half.1),
            (a.0, top),
            (b.0, top),
            (b.0, b.1 - b_half.1),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::model::{Line, LineKind, Node, NodeKind};

    fn graph(count: usize, links: &[(usize, usize)]) -> Model {
        Model {
            nodes: (0..count)
                .map(|i| {
                    Node::bare(
                        &format!("n{i}"),
                        NodeKind::Entity,
                        "host.name",
                        &format!("h{i}"),
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

    /// Whether two boxes touch. The default leaves room for the usual node (170 wide); the
    /// others for the widest, a note (220), which they have to, since they are not as loose.
    fn overlap(a: (f32, f32), b: (f32, f32), wide: f32) -> bool {
        (a.0 - b.0).abs() < wide && (a.1 - b.1).abs() < NODE_H
    }

    fn sane(m: &Model, kind: LayoutKind) -> Vec<(f32, f32)> {
        let pos = layout_as(m, kind);
        assert_eq!(pos.len(), m.nodes.len());
        assert!(
            pos.iter().all(|(x, y)| x.is_finite() && y.is_finite()),
            "{kind:?}"
        );
        for a in 0..pos.len() {
            for b in (a + 1)..pos.len() {
                let wide = if kind == LayoutKind::Force {
                    170.0
                } else {
                    WIDE
                };
                assert!(
                    !overlap(pos[a], pos[b], wide),
                    "{kind:?}: {a} and {b} overlap"
                );
            }
        }
        assert_eq!(pos, layout_as(m, kind), "{kind:?} is not reproducible");
        pos
    }

    /// A small incident: a tree, a cycle, a diamond, and some loose nodes.
    fn incident() -> Model {
        let mut links = vec![(0, 1), (0, 2), (1, 3), (1, 4), (2, 5)]; // a tree
        links.extend([(6, 7), (7, 8), (8, 6)]); // a cycle
        links.extend([(9, 10), (9, 11), (10, 12), (11, 12)]); // a diamond
        graph(18, &links) // 13..18 have no line
    }

    #[test]
    fn every_layout_gives_each_node_a_place_without_overlap_and_the_same_every_time() {
        let m = incident();
        for kind in LayoutKind::ALL {
            sane(&m, kind);
        }
        for kind in LayoutKind::ALL {
            assert!(layout_as(&Model::default(), kind).is_empty());
            assert_eq!(sane(&graph(1, &[]), kind).len(), 1);
        }
        // Names round trip.
        for kind in LayoutKind::ALL {
            assert_eq!(LayoutKind::parse(kind.slug()), Some(kind));
        }
        assert_eq!(LayoutKind::default(), LayoutKind::Force);
    }

    #[test]
    fn hierarchical_puts_what_points_at_a_node_above_it_and_survives_a_cycle() {
        let m = graph(5, &[(0, 1), (1, 2), (0, 3), (3, 2), (2, 4)]);
        let pos = layered(5, &[(0, 1), (1, 2), (0, 3), (3, 2), (2, 4)]);
        let y = |i: usize| pos[i].1;
        assert!(y(0) < y(1) && y(1) < y(2) && y(0) < y(3) && y(3) < y(2) && y(2) < y(4));
        assert_eq!(y(1), y(3), "the two middles share a layer");
        // The diamond is symmetric: the top and bottom sit over the middle of the two.
        assert!(
            (pos[0].0 - (pos[1].0 + pos[3].0) / 2.0).abs() < 1.0,
            "{pos:?}"
        );
        // A cycle is broken once and so is a chain: three layers.
        let mut ys: Vec<f32> = layered(3, &[(0, 1), (1, 2), (2, 0)])
            .iter()
            .map(|p| p.1)
            .collect();
        ys.sort_by(f32::total_cmp);
        ys.dedup();
        assert_eq!(ys.len(), 3);
        let _ = m;
    }

    #[test]
    fn radial_puts_the_best_connected_node_in_the_middle_and_rings_grow_outwards() {
        // A hub with three spokes, one of which has two more.
        let edges = [(0, 1), (0, 2), (0, 3), (3, 4), (3, 5)];
        let pos = radial(6, &edges);
        assert_eq!(pos[0], (0.0, 0.0), "the hub is in the middle");
        let r = |i: usize| pos[i].0.hypot(pos[i].1);
        assert!(r(1) > 0.0 && (r(1) - r(2)).abs() < 1.0 && (r(1) - r(3)).abs() < 1.0);
        assert!(
            r(4) > r(3) && (r(4) - r(5)).abs() < 1.0,
            "the next ring is further out"
        );
        // Many spokes push the ring out until they fit.
        let many: Vec<(usize, usize)> = (1..40).map(|i| (0, i)).collect();
        let wide = radial(40, &many);
        let ring = wide[1].0.hypot(wide[1].1);
        assert!(ring > 40.0 * ARC / std::f32::consts::TAU * 0.9, "{ring}");
    }

    #[test]
    fn orthogonal_nodes_sit_on_a_grid_and_a_route_has_only_right_angles() {
        let pos = on_grid(6, &[(0, 1), (0, 2), (1, 3), (2, 3), (3, 4), (3, 5)]);
        for p in &pos {
            assert!(
                ((p.0 / STEP) - (p.0 / STEP).round()).abs() < 1e-3,
                "column {p:?}"
            );
            assert!(
                ((p.1 / LAYER) - (p.1 / LAYER).round()).abs() < 1e-3,
                "row {p:?}"
            );
        }
        let half = (WIDE / 2.0, NODE_H / 2.0);
        // Different rows: out of the bottom, across, into the top.
        let route = orthogonal_route((0.0, 0.0), half, (300.0, 200.0), half);
        for w in route.windows(2) {
            assert!(w[0].0 == w[1].0 || w[0].1 == w[1].1, "{route:?}");
        }
        assert_eq!(route[0], (0.0, NODE_H / 2.0));
        assert_eq!(route[3], (300.0, 200.0 - NODE_H / 2.0));
        // The same row: up over them, and still right-angled.
        let over = orthogonal_route((0.0, 0.0), half, (400.0, 0.0), half);
        for w in over.windows(2) {
            assert!(w[0].0 == w[1].0 || w[0].1 == w[1].1, "{over:?}");
        }
        assert!(over[1].1 < -NODE_H / 2.0, "goes over the top");
    }
}
