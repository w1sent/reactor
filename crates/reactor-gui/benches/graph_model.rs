//! The graph view's computations on a synthetic case of growing size: building what is drawn
//! from the stored graph, laying it out, and what a frame works out before it draws.
//!
//! ```text
//! cargo bench -p reactor-gui --bench graph_model
//! REACTOR_BENCH_SIZES=500,2000 REACTOR_BENCH_OUT=/tmp/gui.tsv cargo bench -p reactor-gui --bench graph_model -- layout
//! ```
//!
//! Sizes are entities; the graph has about as many claims again (see `reactor_db::bench::Shape`),
//! so the node and line counts are printed beside them. The force-directed layout is quadratic
//! in the size of a connected group: `REACTOR_BENCH_FORCE_MAX` (default 6000 nodes) is where the
//! bench stops asking it for a whole-graph layout.

use std::collections::HashMap;

use reactor_db::Case;
use reactor_db::bench::{Bench, Shape, populate};
use reactor_db::graph::fields::Fields;
use reactor_db::graph::query;
use reactor_gui::graph::arrange::{self, LayoutKind};
use reactor_gui::graph::card::{self, Metrics, Size};
use reactor_gui::graph::model::{self, Detail, FROM_ROLES};
use reactor_gui::graph::sift::{self, Layer, Mode, Scope};

fn main() {
    let mut bench = Bench::new("gui-graph");
    let force_max: usize = std::env::var("REACTOR_BENCH_FORCE_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6000);
    let from_roles: Vec<String> = FROM_ROLES.iter().map(|r| r.to_string()).collect();
    let metrics = Metrics::default();
    for entities in Bench::sizes(&[200, 1_000, 3_000, 10_000]) {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), None).unwrap();
        populate(case.db(), &Shape::of(entities)).unwrap();
        let (elements, edges) = query::snapshot(case.db(), false).unwrap();
        let fields = Fields::default();

        let m = model::build(&elements, &edges, &from_roles, &fields);
        let n = m.nodes.len();
        println!(
            "-- {entities} entities: {} elements, {} edges -> {n} nodes, {} lines",
            elements.len(),
            edges.len(),
            m.lines.len()
        );
        bench.run("model::build", n, || {
            model::build(&elements, &edges, &from_roles, &fields)
        });

        let cards = |plain| -> Vec<card::Card> {
            m.nodes
                .iter()
                .map(|node| card::card(node, Size::default(), None, plain, &metrics))
                .collect()
        };
        bench.run("cards (card::card for every node)", n, || cards(false));
        let sizes: Vec<(f32, f32)> = cards(false).iter().map(|c| (c.w, c.h)).collect();

        bench.run("connected_groups", n, || arrange_groups(&m));

        // A first layout: nothing placed yet.
        if n <= force_max {
            bench.run("layout force, nothing placed", n, || {
                model::layout(&m, &HashMap::new(), &sizes)
            });
        } else {
            println!("(layout force, nothing placed: skipped above {force_max} nodes)");
        }
        for kind in [
            LayoutKind::Hierarchical,
            LayoutKind::Radial,
            LayoutKind::Orthogonal,
        ] {
            bench.run(&format!("layout_as {}", kind.slug()), n, || {
                arrange::layout_as(&m, kind, &sizes)
            });
        }

        // A graph that was laid out, and then something new arrives: what an agent's tool call
        // does on every refresh. The positions stand in for a stored layout.
        let placed = arrange::layout_as(&m, LayoutKind::Hierarchical, &sizes);
        let fresh = 5.min(n);
        let known: HashMap<String, (f32, f32)> = m
            .nodes
            .iter()
            .zip(&placed)
            .take(n - fresh)
            .map(|(node, p)| (node.id.clone(), *p))
            .collect();
        bench.run("layout force, 5 new among placed", n, || {
            model::layout(&m, &known, &sizes)
        });
        let mut again = model::layout(&m, &known, &sizes);
        let fixed: Vec<bool> = m
            .nodes
            .iter()
            .map(|node| known.contains_key(&node.id))
            .collect();
        bench.run("separate, 5 new among placed", n, || {
            let mut pos = again.clone();
            model::separate(&mut pos, &sizes, &fixed, arrange::MIN_GAP);
            pos
        });
        again.truncate(n);

        // What a frame works out before it builds an element.
        for detail in [Detail::Basic, Detail::Analysis, Detail::Full] {
            bench.run(&format!("disclose {}", detail.label()), n, || {
                model::disclose(&m, detail)
            });
        }
        let positions: HashMap<String, (f32, f32)> = m
            .nodes
            .iter()
            .zip(&placed)
            .map(|(node, p)| (node.id.clone(), *p))
            .collect();
        let layer = |text: &str, mode| Layer {
            filter: model::parse_filter(text),
            mode,
            on: true,
        };
        let none: Vec<Layer> = Vec::new();
        let one = vec![layer("net.ipv4", Mode::ShowOnly)];
        let three = vec![
            layer("type:net", Mode::ShowOnly),
            layer("-status:soft-deleted tag:phase.", Mode::Hide),
            layer("by:agent|conf:>=likely", Mode::Dim),
        ];
        bench.run("sift, no layers", n, || sift::sift(&m, &none));
        bench.run("sift, one text layer", n, || sift::sift(&m, &one));
        bench.run("sift, three layers", n, || sift::sift(&m, &three));
        let shown = vec![true; n];
        bench.run("find 'pattern'", n, || {
            sift::find(&m, &shown, "pattern", Scope::default())
        });
        bench.run("tag_groups + padded_hull (12 groups)", n, || {
            model::tag_groups(&m, &positions, &sizes, &shown, 12)
                .iter()
                .map(|g| model::padded_hull(&g.boxes, 46.).len())
                .sum::<usize>()
        });
        bench.run("frame prep: disclose+sift+cards", n, || {
            let d = model::disclose(&m, Detail::Analysis);
            let s = sift::sift(&m, &one);
            let c = cards(false);
            (d.shown.len(), s.nodes.len(), c.len())
        });
    }
}

fn arrange_groups(m: &model::Model) -> usize {
    model::connected_groups(m).0.len()
}
