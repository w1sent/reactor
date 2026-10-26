//! The graph panel's inspector, driven by the pointer in a real (headless) window: what a
//! click does where the unit tests cannot see.
#![recursion_limit = "256"]

use gpui_kit::InputEvent as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext, Bounds, MouseButton, MouseDownEvent, MouseUpEvent, Point, TestAppContext,
    WeakEntity, WindowBounds, WindowOptions, point, px, size,
};
use reactor_gui::graph::panel::GraphPanel;

fn open(cx: &mut TestAppContext) -> (gpui_kit::AnyWindowHandle, gpui_kit::Entity<GraphPanel>) {
    cx.update(gpui_kit::init);
    let (window, panel) = cx.update(|cx| {
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(1200.), px(700.)),
        };
        let (window, panel) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| GraphPanel::new(WeakEntity::new_invalid(), window, cx)),
        )
        .expect("open test window");
        (window, panel)
    });
    cx.update_window(window, |_, window, cx| {
        panel.update(cx, |p, cx| {
            p.select_for_test(cx);
            // As a double click on a node does.
            p.open_inspector_for_test(cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    (window, panel)
}

fn click(cx: &mut TestAppContext, window: gpui_kit::AnyWindowHandle, x: f32, y: f32) {
    cx.update_window(window, |_, window, cx| {
        let position = point(px(x), px(y));
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position,
                modifiers: Default::default(),
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    })
    .unwrap();
}

fn selected(
    cx: &mut TestAppContext,
    window: gpui_kit::AnyWindowHandle,
    panel: &gpui_kit::Entity<GraphPanel>,
) -> bool {
    cx.update_window(window, |_, _, cx| panel.read(cx).has_selection())
        .unwrap()
}

#[gpui_kit::test]
async fn a_click_on_the_graph_outside_the_inspector_leaves_it_open(cx: &mut TestAppContext) {
    let (window, panel) = open(cx);
    assert!(
        selected(cx, window, &panel),
        "something is selected to begin with"
    );
    // The inspector is at the left, 420 px; the plane is the rest.
    click(cx, window, 900., 400.);
    assert!(
        selected(cx, window, &panel),
        "a click on the plane does not close the inspector: only its X does"
    );
}

#[gpui_kit::test]
async fn the_x_closes_the_inspector(cx: &mut TestAppContext) {
    let (window, panel) = open(cx);
    cx.update_window(window, |_, window, cx| window.click("graph-deselect", cx))
        .unwrap();
    assert!(!selected(cx, window, &panel), "the X closes it");
}

#[gpui_kit::test]
async fn a_click_inside_the_inspector_leaves_it_open(cx: &mut TestAppContext) {
    let (window, panel) = open(cx);
    click(cx, window, 200., 400.);
    assert!(selected(cx, window, &panel));
}

// -- in the dock, as the application has it ------------------------------------------------

struct Host {
    area: gpui_kit::Entity<gpui_kit::component::dock::DockArea>,
}

impl gpui_kit::Render for Host {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::prelude::*;
        gpui_kit::div().size_full().child(self.area.clone())
    }
}

fn open_in_dock(
    cx: &mut TestAppContext,
) -> (gpui_kit::AnyWindowHandle, gpui_kit::Entity<GraphPanel>) {
    use gpui_kit::component::dock::{DockLayout, DockSkin, panel_handle};
    cx.update(gpui_kit::init);
    let (window, panel) = cx.update(|cx| {
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(1200.), px(700.)),
        };
        let mut made = None;
        let (window, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let (area, _skin) = DockSkin::dock_area("graph-test", Some(1), window, cx);
                let panel = cx.new(|cx| GraphPanel::new(WeakEntity::new_invalid(), window, cx));
                let layout = DockLayout::tabs().panel_view(panel_handle(panel.clone()), cx);
                area.update(cx, |area, cx| area.set_center(layout, window, cx));
                made = Some(panel);
                cx.new(|_| Host { area })
            },
        )
        .expect("open test window");
        (window, made.unwrap())
    });
    cx.update_window(window, |_, window, cx| {
        panel.update(cx, |p, cx| {
            p.select_for_test(cx);
            // As a double click on a node does.
            p.open_inspector_for_test(cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    (window, panel)
}

#[gpui_kit::test]
async fn a_click_on_the_graph_leaves_the_inspector_open_in_the_dock_too(cx: &mut TestAppContext) {
    let (window, panel) = open_in_dock(cx);
    assert!(selected(cx, window, &panel));
    click(cx, window, 900., 400.);
    assert!(
        selected(cx, window, &panel),
        "a click on the plane, in the dock"
    );
    // Nor does a drag across it, which pans.
    cx.update_window(window, |_, window, cx| {
        window.drag(point(px(800.), px(300.)), point(px(900.), px(350.)), cx)
    })
    .unwrap();
    assert!(selected(cx, window, &panel), "a drag across the plane");
}

// -- with a case behind it, as the application has it ---------------------------------------

#[gpui_kit::test]
async fn the_inspector_of_a_real_element_stays_until_its_x_is_clicked(cx: &mut TestAppContext) {
    use reactor_db::graph::{Batch, Content, Kind, Put, apply};
    use reactor_db::manifest::Author;
    use std::sync::Arc;

    let root = tempfile::tempdir().unwrap();
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    // SAFETY: nothing else in this test binary reads the environment.
    unsafe { std::env::set_var("REACTOR_CONFIG_DIR", root.path().join("cfg")) };
    let backend = reactor_gui::backend::Backend::start(reactor_gui::backend::StartOptions {
        cwd: work,
        ..Default::default()
    })
    .expect("a backend");
    let id = apply(
        backend.case.db(),
        Batch::new(Author::analyst()).put(Put {
            content: Content::value(
                "## What was seen\n\nA long text, with a heading, that has more than one line.\n\nAnd a second paragraph.",
            ),
            ..Put::new(Kind::Annotation, "analysis.hypothesis")
        }),
    )
    .unwrap()
    .ids
    .remove(0);

    let (window, panel) = open_in_dock(cx);
    cx.update_window(window, |_, window, cx| {
        panel.update(cx, |p, cx| {
            p.set_backend(Some(Arc::new(backend)), cx);
            p.select_id_for_test(&id, cx);
            p.open_inspector_for_test(cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    assert!(selected(cx, window, &panel));
    assert!(
        cx.update_window(window, |_, _, cx| panel.read(cx).has_inspected())
            .unwrap(),
        "the element is in the inspector"
    );
    click(cx, window, 900., 400.);
    assert!(
        selected(cx, window, &panel),
        "a click on the plane leaves the inspector of a real element open"
    );
    cx.update_window(window, |_, window, cx| window.click("graph-deselect", cx))
        .unwrap();
    assert!(!selected(cx, window, &panel), "its X closes it");
}

#[gpui_kit::test]
async fn a_selection_alone_does_not_open_the_inspector_and_clearing_it_closes_it(
    cx: &mut TestAppContext,
) {
    let (window, panel) = open(cx);
    cx.update_window(window, |_, _, cx| {
        panel.update(cx, |p, cx| p.close_inspector_for_test(cx));
    })
    .unwrap();
    assert!(selected(cx, window, &panel), "still selected");
    let open = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, _, cx| panel.read(cx).inspector_open_for_test())
            .unwrap()
    };
    assert!(!open(cx), "a click selects; it does not open the inspector");
    cx.update_window(window, |_, window, cx| {
        panel.update(cx, |p, cx| p.open_inspector_for_test(cx));
        window.render_frame(cx);
        window.click("graph-deselect", cx);
    })
    .unwrap();
    assert!(
        !selected(cx, window, &panel) && !open(cx),
        "the X closes it"
    );
}
