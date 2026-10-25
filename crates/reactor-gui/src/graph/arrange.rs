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

use super::model::{
    CLEAR, Loose, Model, connected_groups, layout, lines_by_group, place_loose, reach, separate,
};

/// The space between the edges of two nodes side by side. A line's label and its arrowhead sit
/// between two nodes, so the boxes never touch.
const AIR: f32 = CLEAR;
/// The space between the edges of two layers.
const LAYER_AIR: f32 = 120.0;
/// The space between groups that are not connected.
const SPACE: f32 = 110.0;
/// The least space left between any two nodes, whatever the arrangement. Arrangements keep
/// more than this; it is for where one of them could not.
pub const MIN_GAP: f32 = 50.0;

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

/// Where each node goes, arranged afresh in the given way. `sizes` are the whole widths and
/// heights the nodes are drawn at, in the order of the model's nodes: each arrangement leaves
/// the room a node takes, so that big nodes and small ones are as far apart.
pub fn layout_as(model: &Model, kind: LayoutKind, sizes: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let mut pos = arranged(model, kind, sizes);
    separate(&mut pos, sizes, &vec![false; sizes.len()], MIN_GAP);
    pos
}

fn arranged(model: &Model, kind: LayoutKind, sizes: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if kind == LayoutKind::Force {
        return layout(model, &HashMap::new(), sizes);
    }
    let n = model.nodes.len();
    let mut pos = vec![(0.0, 0.0); n];
    let (groups, group_of, _) = connected_groups(model);
    // The lines of each group, as ends of nodes in it, `from` to `to` as drawn.
    let mut group_edges = lines_by_group(model, &group_of, groups.len());
    let mut loose: Vec<Loose> = Vec::new();
    for (g, members) in groups.iter().enumerate() {
        let edges = std::mem::take(&mut group_edges[g]);
        let here: Vec<(f32, f32)> = members.iter().map(|&i| sizes[i]).collect();
        let local_pos = match kind {
            LayoutKind::Hierarchical => layered(&here, &edges).0,
            LayoutKind::Radial => radial(&here, &edges),
            LayoutKind::Orthogonal => on_grid(&here, &edges),
            LayoutKind::Force => unreachable!(),
        };
        loose.push((members.clone(), local_pos));
    }
    place_loose(&loose, &[], &mut pos, sizes, SPACE);
    pos
}

// -- hierarchical -------------------------------------------------------------------------------

/// Layers: the lines made to run one way (a cycle is broken where the walk finds it), each
/// node on the layer after the last one that points at it, then the order along each layer
/// swept to the average place of its neighbours, and the places along it pulled towards the
/// same. Two neighbours on a layer keep [`AIR`] between their edges whatever their widths, and
/// two layers [`LAYER_AIR`] between the tallest of each. The centres, and the layer of each.
fn layered(sizes: &[(f32, f32)], edges: &[(usize, usize)]) -> (Vec<(f32, f32)>, Vec<usize>) {
    let m = sizes.len();
    if m == 0 {
        return (Vec::new(), Vec::new());
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

    // Places along the layer: a row's nodes at least their widths and some air apart, pulled
    // towards where their neighbours are, down and up a few times.
    let step = |a: usize, b: usize| (sizes[a].0 + sizes[b].0) / 2.0 + AIR;
    let mut x = vec![0.0f32; m];
    for row in &rows {
        let mut at = 0.0f32;
        for (k, &v) in row.iter().enumerate() {
            if k > 0 {
                at += step(row[k - 1], v);
            }
            x[v] = at;
        }
        for &v in row {
            x[v] -= at / 2.0;
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
                fwd[k] = fwd[k].max(fwd[k - 1] + step(row[k - 1], row[k]));
            }
            let mut bwd = wanted;
            for k in (0..bwd.len().saturating_sub(1)).rev() {
                bwd[k] = bwd[k].min(bwd[k + 1] - step(row[k], row[k + 1]));
            }
            for (k, &v) in row.iter().enumerate() {
                x[v] = (fwd[k] + bwd[k]) / 2.0;
            }
        }
    }
    // Down the layers: each as tall as its tallest node, with air between.
    let mut y = vec![0.0f32; depth];
    let mut tall_before = 0.0f32;
    for (r, row) in rows.iter().enumerate() {
        let tall = row.iter().map(|&v| sizes[v].1).fold(0.0f32, f32::max);
        y[r] = if r == 0 {
            0.0
        } else {
            y[r - 1] + tall_before / 2.0 + LAYER_AIR + tall / 2.0
        };
        tall_before = tall;
    }
    ((0..m).map(|i| (x[i], y[layer[i]])).collect(), layer)
}

// -- radial -------------------------------------------------------------------------------------

/// The most connected node in the middle; a walk outwards from it gives each node its ring;
/// each node gets a slice of the circle as wide as the nodes under it need, and sits in the
/// middle of it. A ring is as far out as the boxes on it and the one inside it make it:
/// [`AIR`] between their edges, along the line out and along the ring.
fn radial(sizes: &[(f32, f32)], edges: &[(usize, usize)]) -> Vec<(f32, f32)> {
    let m = sizes.len();
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
    // Ring radii: far enough out that the boxes of one ring clear those of the ring inside,
    // and that neighbours on the ring clear each other.
    let rings = ring.iter().copied().max().unwrap_or(0);
    let out = |v: usize| reach(sizes[v], (angle[v].cos(), angle[v].sin()));
    let along = |v: usize| reach(sizes[v], (-angle[v].sin(), angle[v].cos()));
    // How far each ring's boxes reach in towards the middle and out from it; the middle node
    // has no direction of its own, so it counts as far as it reaches towards its children.
    let mut deep = vec![0.0f32; rings + 1];
    for v in 0..m {
        if ring[v] == usize::MAX {
            continue;
        }
        let r = if ring[v] == 0 {
            (0..m)
                .filter(|&c| ring[c] == 1)
                .map(|c| reach(sizes[v], (angle[c].cos(), angle[c].sin())))
                .fold(0.0f32, f32::max)
        } else {
            out(v)
        };
        deep[ring[v]] = deep[ring[v]].max(r);
    }
    let mut radius = vec![0.0f32; rings + 1];
    for k in 1..=rings {
        let mut on: Vec<usize> = (0..m).filter(|&v| ring[v] == k).collect();
        on.sort_by(|&a, &b| angle[a].total_cmp(&angle[b]).then(a.cmp(&b)));
        let mut crowded = 0.0f32;
        for (k, &a) in on.iter().enumerate() {
            let (b, apart) = match on.get(k + 1) {
                Some(&b) => (b, angle[b] - angle[a]),
                None if on.len() > 1 => (on[0], std::f32::consts::TAU - (angle[a] - angle[on[0]])),
                None => continue,
            };
            crowded = crowded.max((along(a) + along(b) + AIR) / apart.max(1e-3));
        }
        radius[k] = (radius[k - 1] + deep[k - 1] + deep[k] + AIR).max(crowded);
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

/// The layered arrangement, snapped onto a grid: columns as far apart as the widest node and
/// some air need, layers as in [`layered`], a node that would share a cell moving to the next
/// free one on its row. Lines are then drawn right-angled ([`orthogonal_route`]).
fn on_grid(sizes: &[(f32, f32)], edges: &[(usize, usize)]) -> Vec<(f32, f32)> {
    let m = sizes.len();
    let (flow, layer) = layered(sizes, edges);
    let column = sizes.iter().map(|s| s.0).fold(0.0f32, f32::max) + AIR;
    let mut by_row: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, &row) in layer.iter().enumerate() {
        by_row.entry(row).or_default().push(i);
    }
    let mut out = vec![(0.0f32, 0.0f32); m];
    for (_, mut nodes) in by_row {
        nodes.sort_by(|&a, &b| flow[a].0.total_cmp(&flow[b].0).then(a.cmp(&b)));
        let mut taken: i64 = i64::MIN;
        for v in nodes {
            let wanted = (flow[v].0 / column).round() as i64;
            let col = wanted.max(taken.saturating_add(1));
            taken = col;
            out[v] = (col as f32 * column, flow[v].1);
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
    use crate::graph::model::NODE_H;

    /// The widest box a node is drawn in (a note).
    const WIDE: f32 = 220.0;
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

    /// Every node at the usual size, and a mix of narrow, wide and tall ones.
    fn usual(m: &Model) -> Vec<(f32, f32)> {
        vec![(WIDE, NODE_H); m.nodes.len()]
    }
    fn mixed(m: &Model) -> Vec<(f32, f32)> {
        (0..m.nodes.len())
            .map(|i| match i % 4 {
                0 => (120.0, 44.0),
                1 => (220.0, 90.0),
                2 => (260.0, 210.0),
                _ => (170.0, 60.0),
            })
            .collect()
    }

    /// The least space between the edges of any two boxes (negative where they overlap).
    fn least_space(pos: &[(f32, f32)], sizes: &[(f32, f32)]) -> f32 {
        let mut least = f32::MAX;
        for a in 0..pos.len() {
            for b in (a + 1)..pos.len() {
                let across = (pos[a].0 - pos[b].0).abs() - (sizes[a].0 + sizes[b].0) / 2.0;
                let down = (pos[a].1 - pos[b].1).abs() - (sizes[a].1 + sizes[b].1) / 2.0;
                least = least.min(across.max(down));
            }
        }
        least
    }

    fn sane(m: &Model, kind: LayoutKind, sizes: &[(f32, f32)]) -> Vec<(f32, f32)> {
        let pos = layout_as(m, kind, sizes);
        assert_eq!(pos.len(), m.nodes.len());
        assert!(
            pos.iter().all(|(x, y)| x.is_finite() && y.is_finite()),
            "{kind:?}"
        );
        let space = least_space(&pos, sizes);
        assert!(
            space >= MIN_GAP - 0.5,
            "{kind:?}: boxes only {space} apart at the least"
        );
        assert_eq!(
            pos,
            layout_as(m, kind, sizes),
            "{kind:?} is not reproducible"
        );
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
    fn every_layout_gives_each_node_a_place_with_air_between_and_the_same_every_time() {
        let m = incident();
        for kind in LayoutKind::ALL {
            sane(&m, kind, &usual(&m));
            sane(&m, kind, &mixed(&m));
        }
        for kind in LayoutKind::ALL {
            assert!(layout_as(&Model::default(), kind, &[]).is_empty());
            let one = graph(1, &[]);
            assert_eq!(sane(&one, kind, &usual(&one)).len(), 1);
        }
        // Names round trip.
        for kind in LayoutKind::ALL {
            assert_eq!(LayoutKind::parse(kind.slug()), Some(kind));
        }
        assert_eq!(LayoutKind::default(), LayoutKind::Force);
    }

    #[test]
    fn big_nodes_get_the_air_small_ones_do() {
        let m = incident();
        let sizes = mixed(&m);
        let space = |a: usize, b: usize, pos: &[(f32, f32)]| {
            let across = (pos[a].0 - pos[b].0).abs() - (sizes[a].0 + sizes[b].0) / 2.0;
            let down = (pos[a].1 - pos[b].1).abs() - (sizes[a].1 + sizes[b].1) / 2.0;
            across.max(down)
        };
        for kind in [
            LayoutKind::Force,
            LayoutKind::Hierarchical,
            LayoutKind::Orthogonal,
        ] {
            let pos = layout_as(&m, kind, &sizes);
            for l in m.lines.iter().filter(|l| l.from != l.to) {
                let s = space(l.from, l.to, &pos);
                assert!(
                    (MIN_GAP - 0.5..4.0 * AIR).contains(&s),
                    "{kind:?}: {} and {} are {s} apart",
                    l.from,
                    l.to
                );
            }
        }
    }

    #[test]
    fn hierarchical_puts_what_points_at_a_node_above_it_and_survives_a_cycle() {
        let m = graph(5, &[(0, 1), (1, 2), (0, 3), (3, 2), (2, 4)]);
        let sizes = vec![(WIDE, NODE_H); 5];
        let (pos, _) = layered(&sizes, &[(0, 1), (1, 2), (0, 3), (3, 2), (2, 4)]);
        let y = |i: usize| pos[i].1;
        assert!(y(0) < y(1) && y(1) < y(2) && y(0) < y(3) && y(3) < y(2) && y(2) < y(4));
        assert_eq!(y(1), y(3), "the two middles share a layer");
        // The diamond is symmetric: the top and bottom sit over the middle of the two.
        assert!(
            (pos[0].0 - (pos[1].0 + pos[3].0) / 2.0).abs() < 1.0,
            "{pos:?}"
        );
        // A cycle is broken once and so is a chain: three layers.
        let mut ys: Vec<f32> = layered(&sizes[..3], &[(0, 1), (1, 2), (2, 0)])
            .0
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
        let pos = radial(&[(WIDE, NODE_H); 6], &edges);
        assert_eq!(pos[0], (0.0, 0.0), "the hub is in the middle");
        let r = |i: usize| pos[i].0.hypot(pos[i].1);
        assert!(r(1) > 0.0 && (r(1) - r(2)).abs() < 1.0 && (r(1) - r(3)).abs() < 1.0);
        assert!(
            r(4) > r(3) && (r(4) - r(5)).abs() < 1.0,
            "the next ring is further out"
        );
        // Many spokes push the ring out until they fit.
        let many: Vec<(usize, usize)> = (1..40).map(|i| (0, i)).collect();
        let sizes = vec![(WIDE, NODE_H); 40];
        let wide = radial(&sizes, &many);
        let ring = wide[1].0.hypot(wide[1].1);
        assert!(ring > 40.0 * 100.0 / std::f32::consts::TAU, "{ring}");
        assert!(least_space(&wide, &sizes) > 0.0, "no two overlap");
    }

    #[test]
    fn orthogonal_nodes_sit_on_a_grid_and_a_route_has_only_right_angles() {
        let pos = on_grid(
            &[(WIDE, NODE_H); 6],
            &[(0, 1), (0, 2), (1, 3), (2, 3), (3, 4), (3, 5)],
        );
        let (column, row) = (WIDE + AIR, NODE_H + LAYER_AIR);
        for p in &pos {
            assert!(
                ((p.0 / column) - (p.0 / column).round()).abs() < 1e-3,
                "column {p:?}"
            );
            assert!(
                ((p.1 / row) - (p.1 / row).round()).abs() < 1e-3,
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
