//! The graph as the database serves it: what it costs to write a case of a given size, and to
//! read it back the way the graph view does on every refresh.
//!
//! ```text
//! cargo bench -p reactor-db --bench graph
//! REACTOR_BENCH_SIZES=1000,10000 REACTOR_BENCH_OUT=/tmp/db.tsv cargo bench -p reactor-db --bench graph -- snapshot
//! ```

use std::time::Instant;

use reactor_db::Case;
use reactor_db::bench::{Bench, Shape, populate};
use reactor_db::graph::{self, fields, layout, query, views, vocab};

fn main() {
    let mut bench = Bench::new("db-graph");
    for entities in Bench::sizes(&[200, 1_000, 5_000, 20_000]) {
        // REACTOR_BENCH_KEEP=<dir> builds the case there and leaves it, to look at with sqlite3.
        let kept = std::env::var_os("REACTOR_BENCH_KEEP").map(std::path::PathBuf::from);
        let dir = tempfile::tempdir().unwrap();
        let root = kept.as_deref().unwrap_or(dir.path());
        let case = Case::create(root, None).unwrap();
        let db = case.db();
        let started = Instant::now();
        let made = populate(db, &Shape::of(entities)).unwrap();
        let elements = entities + made.claims + made.notes;
        bench.once("populate (write path)", elements, started.elapsed());

        // What the graph view reads on every refresh.
        bench.run("snapshot (elements + edges)", elements, || {
            query::snapshot(db, false).unwrap()
        });
        bench.run("fields::load", elements, || fields::load(db).unwrap());
        bench.run("vocab::load", elements, || vocab::load(db).unwrap());
        bench.run("views::list", elements, || views::list(db).unwrap());
        bench.run("edit::type_choices", elements, || {
            graph::edit::type_choices(db, &[graph::Kind::Entity]).unwrap()
        });

        // Positions: stored on layout, read on refresh.
        let positions: Vec<(String, (f32, f32))> = made
            .entities
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), (i as f32 * 3.0, i as f32 * 2.0)))
            .collect();
        bench.run("layout::save (all nodes)", positions.len(), || {
            layout::save(db, &positions).unwrap()
        });
        bench.run("layout::load", positions.len(), || {
            layout::load(db).unwrap()
        });
        bench.run("layout::save (one node)", 1, || {
            layout::save(db, &positions[..1]).unwrap()
        });

        // The detail pane and the agent's tools read one element at a time.
        bench.run("query::get (one element)", elements, || {
            query::get(db, &made.entities[made.entities.len() / 2]).unwrap()
        });
        bench.run("query::edges_to (one entity)", elements, || {
            query::edges_to(db, &made.entities[made.entities.len() / 2]).unwrap()
        });
        bench.run("query::find text 'net.ipv4' limit 50", elements, || {
            query::find(
                db,
                &query::Filter {
                    text: Some("net.ipv4-000".into()),
                    limit: 50,
                    ..query::Filter::default()
                },
            )
            .unwrap()
        });
        bench.run("query::relations (all)", elements, || {
            query::relations(db, None).unwrap()
        });
        bench.run("query::orphans", elements, || query::orphans(db).unwrap());
    }
}
