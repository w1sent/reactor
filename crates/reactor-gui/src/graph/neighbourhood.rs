//! What is next to the selected nodes, for the graph to draw in its own colour.

use super::model::{Line, Model, NodeKind};

/// What is next to the selected nodes: the nodes they are joined to and the lines between.
/// A claim is a node here, but it reads as the line between two things, so what a selected
/// thing shares a claim with is next to it too.
pub(super) struct Neighbourhood {
    nodes: std::collections::HashSet<usize>,
    /// The selected nodes and the claims next to them: a line with one of these at an end
    /// is in the neighbourhood.
    anchors: std::collections::HashSet<usize>,
}

impl Neighbourhood {
    pub(super) fn of(model: &Model, selected: &[String]) -> Neighbourhood {
        use std::collections::HashSet;
        let chosen: HashSet<usize> = model
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| selected.contains(&n.id))
            .map(|(i, _)| i)
            .collect();
        let other = |l: &Line, of: usize| match (l.from == of, l.to == of) {
            (true, false) => Some(l.to),
            (false, true) => Some(l.from),
            _ => None,
        };
        let mut nodes = HashSet::new();
        let mut anchors = chosen.clone();
        for &s in &chosen {
            for l in &model.lines {
                let Some(o) = other(l, s) else { continue };
                nodes.insert(o);
                // Through a claim to the things it is about, unless the selection is one.
                if model.nodes[o].kind == NodeKind::Claim && model.nodes[s].kind != NodeKind::Claim
                {
                    anchors.insert(o);
                    for m in &model.lines {
                        if let Some(p) = other(m, o) {
                            nodes.insert(p);
                        }
                    }
                }
            }
        }
        nodes.retain(|n| !chosen.contains(n));
        Neighbourhood { nodes, anchors }
    }

    pub(super) fn node(&self, index: usize) -> bool {
        self.nodes.contains(&index)
    }

    pub(super) fn line(&self, line: &Line) -> bool {
        self.anchors.contains(&line.from) || self.anchors.contains(&line.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::model::{LineKind, Node};

    fn line(from: usize, to: usize) -> Line {
        Line {
            from,
            to,
            label: String::new(),
            kind: LineKind::Role,
            claims: Vec::new(),
            status: "active".into(),
            confidence: None,
        }
    }

    /// 0 a -- 2 (claim) -- 1 b ;  1 b -- 3 (claim) -- 4 c ;  5 d alone.
    fn model() -> Model {
        let n = |i: &str, kind| Node::bare(i, kind, "t", i);
        Model {
            nodes: vec![
                n("a", NodeKind::Entity),
                n("b", NodeKind::Entity),
                n("x", NodeKind::Claim),
                n("y", NodeKind::Claim),
                n("c", NodeKind::Entity),
                n("d", NodeKind::Entity),
            ],
            lines: vec![line(2, 0), line(2, 1), line(3, 1), line(3, 4)],
        }
    }

    #[test]
    fn what_a_selected_thing_shares_a_claim_with_is_next_to_it_and_nothing_further() {
        let near = Neighbourhood::of(&model(), &["a".to_string()]);
        let mut nodes: Vec<usize> = near.nodes.iter().copied().collect();
        nodes.sort();
        // The claim and b, which it is about; not the second claim, c or d.
        assert_eq!(nodes, [1, 2]);
        assert!(near.line(&line(2, 1)) && near.line(&line(2, 0)));
        assert!(!near.line(&line(3, 1)) && !near.line(&line(3, 4)));
    }

    #[test]
    fn a_selected_claim_is_next_to_what_it_is_about_only() {
        let near = Neighbourhood::of(&model(), &["x".to_string()]);
        let mut nodes: Vec<usize> = near.nodes.iter().copied().collect();
        nodes.sort();
        assert_eq!(nodes, [0, 1]);
        assert!(!near.line(&line(3, 1)));
    }

    #[test]
    fn nothing_selected_has_no_neighbourhood() {
        let near = Neighbourhood::of(&model(), &[]);
        assert!(near.nodes.is_empty() && !near.line(&line(2, 0)));
    }
}
