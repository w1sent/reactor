//! What opening the graph, refreshing it and drawing a frame of it cost as the case grows,
//! in a real (headless) window. Not part of the normal run: it builds cases of thousands of
//! elements.
//!
//! ```text
//! cargo test --release -p reactor-gui --test graph_frame -- --ignored --nocapture
//! REACTOR_BENCH_SIZES=500,3000 REACTOR_BENCH_OUT=/tmp/frame.tsv cargo test --release -p reactor-gui --test graph_frame -- --ignored --nocapture
//! ```
//!
//! A frame here is the panel's whole `render` -- working out what is drawn, building the
//! elements, laying them out -- and the painting the headless platform does, which skips the
//! GPU.
#![recursion_limit = "256"]

use std::sync::Arc;
use std::time::Instant;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext, Bounds, Point, TestAppContext, WeakEntity, WindowBounds, WindowOptions, px, size,
};
use reactor_db::bench::{Bench, Shape, populate};
use reactor_db::graph::{Batch, Kind, Link, Put, Target, apply};
use reactor_db::manifest::Author;
use reactor_gui::graph::panel::GraphPanel;

#[gpui_kit::test]
#[ignore]
async fn frames_at_size(cx: &mut TestAppContext) {
    let mut bench = Bench::new("gui-frame").all();
    for entities in Bench::sizes(&[300, 1_000, 3_000]) {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        // SAFETY: nothing else in this test binary reads the environment while it is set.
        unsafe { std::env::set_var("REACTOR_CONFIG_DIR", root.path().join("cfg")) };
        let backend = Arc::new(
            reactor_gui::backend::Backend::start(reactor_gui::backend::StartOptions {
                cwd: work,
                ..Default::default()
            })
            .expect("a backend"),
        );
        let made = populate(backend.case.db(), &Shape::of(entities)).unwrap();
        let elements = entities + made.claims + made.notes;

        cx.update(|cx| {
            gpui_kit::init(cx);
            reactor_gui::graph::panel::bind_keys(cx);
        });
        let (window, panel) = cx.update(|cx| {
            let bounds = Bounds {
                origin: Point::default(),
                size: size(px(1400.), px(900.)),
            };
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| GraphPanel::new(WeakEntity::new_invalid(), window, cx)),
            )
            .expect("open test window")
        });
        // Twice: the first frame learns how big the drawing area is.
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();

        // Opening a case: read it all, build it, lay it all out.
        let started = Instant::now();
        cx.update_window(window, |_, _, cx| {
            panel.update(cx, |p, cx| p.set_backend(Some(backend.clone()), cx))
        })
        .unwrap();
        bench.once(
            "open (set_backend: read+build+layout)",
            elements,
            started.elapsed(),
        );

        // What every tool call of the agent costs the window: read it all again.
        bench.run("refresh, nothing new", elements, || {
            cx.update_window(window, |_, _, cx| panel.update(cx, |p, cx| p.refresh(cx)))
                .unwrap()
        });
        // What an agent's write looks like: new things related to what is already there, so
        // they join the big group and have to be placed among it.
        let started = Instant::now();
        let mut fresh = Batch::new(Author::agent("bench"));
        for k in 0..5 {
            fresh = fresh.put(Put {
                links: vec![
                    Link::new("src", Target::Id(made.entities[k * 7].clone())),
                    Link::entity("dst", "host.name", &format!("late-{k}")),
                    Link::source("artifact:late.csv"),
                ],
                ..Put::new(Kind::Event, "net.connection")
            });
        }
        apply(backend.case.db(), fresh).unwrap();
        cx.update_window(window, |_, _, cx| panel.update(cx, |p, cx| p.refresh(cx)))
            .unwrap();
        bench.once(
            "refresh, 5 new related entities",
            elements,
            started.elapsed(),
        );

        // Frames: everything in view, then a closer look.
        let mut frame = |bench: &mut Bench, name: &str, zoom: Option<f32>| {
            cx.update_window(window, |_, window, cx| {
                panel.update(cx, |p, cx| match zoom {
                    Some(z) => p.view_for_test(z, (0., 0.), cx),
                    None => p.fit_for_test(cx),
                });
                window.render_frame(cx);
            })
            .unwrap();
            bench.run(name, elements, || {
                cx.update_window(window, |_, window, cx| {
                    panel.update(cx, |_, cx| cx.notify());
                    window.render_frame(cx);
                })
                .unwrap()
            });
        };
        frame(&mut bench, "frame, whole graph in view", None);
        frame(&mut bench, "frame, zoom 0.5 on the middle", Some(0.5));
        frame(&mut bench, "frame, zoom 1.0 on the middle", Some(1.0));
    }
}
