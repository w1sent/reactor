//! Adding a node of any kind by hand (Ctrl/Cmd+N, and the right click), driven in a real
//! (headless) window; and the tags a node can be given, with their colours.
#![recursion_limit = "256"]

use std::sync::Arc;

use gpui_kit::InputEvent as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext, Bounds, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Point,
    TestAppContext, WeakEntity, WindowBounds, WindowOptions, point, px, size,
};
use reactor_db::graph::{Batch, Content, Kind, Put, apply, query, vocab};
use reactor_db::manifest::Author;
use reactor_gui::graph::panel::GraphPanel;

struct Rig {
    window: gpui_kit::AnyWindowHandle,
    panel: gpui_kit::Entity<GraphPanel>,
    backend: Arc<reactor_gui::backend::Backend>,
    host: String,
    _root: tempfile::TempDir,
}

fn rig(cx: &mut TestAppContext) -> Rig {
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
    let host = apply(
        backend.case.db(),
        Batch::new(Author::analyst()).put(Put {
            content: Content::value("ws-042"),
            ..Put::new(Kind::Entity, "host.name")
        }),
    )
    .unwrap()
    .ids
    .remove(0);
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
    let rig = Rig {
        window,
        panel,
        backend,
        host,
        _root: root,
    };
    cx.update_window(window, |_, window, cx| {
        rig.panel
            .update(cx, |p, cx| p.set_backend(Some(rig.backend.clone()), cx));
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    // The plane has the keys.
    rig.stage(cx, "focus");
    rig
}

impl Rig {
    fn stage(&self, cx: &mut TestAppContext, what: &str) {
        cx.update_window(self.window, |_, window, cx| {
            self.panel
                .update(cx, |p, cx| p.stage_for_test(what, window, cx));
            window.render_frame(cx);
        })
        .unwrap();
    }

    fn keys(&self, cx: &mut TestAppContext, keys: &str) {
        cx.update_window(self.window, |_, window, cx| {
            for key in keys.split(' ') {
                window.press(key, cx);
            }
            window.render_frame(cx);
        })
        .unwrap();
    }

    fn typed(&self, cx: &mut TestAppContext, text: &str) {
        cx.simulate_input(self.window, text);
        cx.update_window(self.window, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }

    fn click(&self, cx: &mut TestAppContext, at: (f32, f32)) {
        cx.update_window(self.window, |_, window, cx| {
            let position = point(px(at.0), px(at.1));
            let modifiers = Modifiers::default();
            window.dispatch_event(
                MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers,
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
                    modifiers,
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        })
        .unwrap();
    }

    fn adding(&self, cx: &mut TestAppContext) -> Option<(Vec<String>, usize)> {
        cx.update_window(self.window, |_, _, cx| self.panel.read(cx).add_for_test())
            .unwrap()
    }

    fn inspected(&self, cx: &mut TestAppContext) -> Option<(String, String)> {
        cx.update_window(self.window, |_, _, cx| {
            self.panel.read(cx).inspected_for_test()
        })
        .unwrap()
    }
}

fn secondary(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("cmd-{key}")
    } else {
        format!("ctrl-{key}")
    }
}

#[gpui_kit::test]
fn the_palette_opens_on_the_key_and_lists_types_of_every_kind_as_they_are_typed(
    cx: &mut TestAppContext,
) {
    let rig = rig(cx);
    assert_eq!(rig.adding(cx), None);
    rig.keys(cx, &secondary("n"));
    let (rows, at) = rig.adding(cx).expect("the palette is open");
    assert_eq!(at, 0);
    assert!(rows.len() > 5, "{rows:?}");

    // A type of entity, found by a part of its name.
    rig.typed(cx, "ipv4");
    let (rows, _) = rig.adding(cx).unwrap();
    assert_eq!(
        rows.first().map(String::as_str),
        Some("net.ipv4"),
        "{rows:?}"
    );

    // A word for an annotation, found by what it is called.
    cx.update_window(rig.window, |_, window, cx| {
        rig.panel
            .update(cx, |p, cx| p.stage_for_test("addtype:assess", window, cx));
        window.render_frame(cx);
    })
    .unwrap();
    let (rows, _) = rig.adding(cx).unwrap();
    assert!(rows.iter().any(|r| r == "Assessment"), "{rows:?}");

    // A name that is not a type is offered as a new one, at the end of the list.
    cx.update_window(rig.window, |_, window, cx| {
        rig.panel.update(cx, |p, cx| {
            p.stage_for_test("addtype:actor.agent", window, cx)
        });
        window.render_frame(cx);
    })
    .unwrap();
    let (rows, _) = rig.adding(cx).unwrap();
    assert_eq!(
        rows.last().map(String::as_str),
        Some("new:actor.agent"),
        "{rows:?}"
    );

    // Esc closes it.
    rig.keys(cx, "escape");
    assert_eq!(rig.adding(cx), None);
}

#[gpui_kit::test]
fn a_node_is_added_by_choosing_its_type_and_naming_it_and_opens_in_the_inspector(
    cx: &mut TestAppContext,
) {
    let rig = rig(cx);
    rig.keys(cx, &secondary("n"));
    rig.typed(cx, "actor.agent");
    // Down and up move along the list; with one new type there is only that to move to.
    rig.keys(cx, "down up");
    // Enter chooses the type and goes to the name; Enter again adds.
    rig.keys(cx, "enter");
    rig.typed(cx, "Triage agent");
    rig.keys(cx, "enter");

    assert_eq!(rig.adding(cx), None);
    assert_eq!(
        rig.inspected(cx),
        Some(("actor.agent".to_string(), "Triage agent".to_string()))
    );
    let db = rig.backend.case.db();
    let id = query::entity_id(db, "actor.agent", "Triage agent", None).unwrap();
    assert_eq!(query::get(db, &id).unwrap().unwrap().kind, Kind::Entity);
}

#[gpui_kit::test]
fn a_tag_is_put_on_and_taken_off_and_given_a_colour(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.stage(cx, &format!("select:{}", rig.host));
    rig.stage(cx, "tag:phase.recon");
    let db = rig.backend.case.db();
    let tags = |db: &reactor_db::Db, id: &str| query::get(db, id).unwrap().unwrap().content.tags;
    assert_eq!(tags(db, &rig.host), ["phase.recon"]);

    rig.stage(cx, "tagcolour:phase:teal");
    assert_eq!(
        vocab::load(db).unwrap().tag_colour("phase.recon"),
        Some("teal")
    );
    rig.stage(cx, "tagcolour:phase:default");
    assert_eq!(vocab::load(db).unwrap().tag_colour("phase.recon"), None);

    rig.stage(cx, "untag:phase.recon");
    assert!(tags(db, &rig.host).is_empty());
}

#[gpui_kit::test]
fn the_palette_closes_on_a_click_outside_and_on_esc_even_after_a_click_on_its_ground(
    cx: &mut TestAppContext,
) {
    let rig = rig(cx);
    // It is in the middle of the plane: a click near its top left corner is on it.
    rig.keys(cx, &secondary("n"));
    rig.click(cx, (415., 250.));
    assert!(rig.adding(cx).is_some(), "a click on it leaves it open");
    rig.keys(cx, "escape");
    assert_eq!(rig.adding(cx), None, "Esc, with no field focused");

    rig.keys(cx, &secondary("n"));
    assert!(rig.adding(cx).is_some());
    rig.click(cx, (60., 850.));
    assert_eq!(rig.adding(cx), None, "a click outside");
}
