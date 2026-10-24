//! The plane of the graph panel, driven in a real (headless) window: the keys that size
//! nodes, find, the filter layers, and the card a type's icon is chosen on.
#![recursion_limit = "256"]

use std::sync::Arc;

use gpui_kit::InputEvent as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext, Bounds, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Point,
    TestAppContext, WeakEntity, WindowBounds, WindowOptions, point, px, size,
};
use reactor_db::graph::{Batch, Content, Kind, Link, Put, apply, vocab};
use reactor_db::manifest::Author;
use reactor_gui::graph::panel::{GraphPanel, PlaneForTest};

const PATH: &str = "/private/var/mobile/Containers/Data/Application/7C1E0A52-9B3D-4F6A-A1C8-2E5D90B7F314/Library/Caches/com.apple.nsurlsessiond/Downloads/update_helper.dylib";

struct Rig {
    window: gpui_kit::AnyWindowHandle,
    panel: gpui_kit::Entity<GraphPanel>,
    backend: Arc<reactor_gui::backend::Backend>,
    /// The ids of: the path, the address, the hypothesis, the note.
    ids: Vec<String>,
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
    let text = |kind, type_: &str, value: &str| Put {
        content: Content::value(value),
        ..Put::new(kind, type_)
    };
    let mut address = text(Kind::Entity, "net.ipv4", "198.51.100.7");
    address.content.tags = vec!["c2.beacon".into()];
    let ids = apply(
        backend.case.db(),
        Batch::new(Author::analyst())
            .put(text(Kind::Entity, "file.path", PATH))
            .put(address)
            .put(text(
                Kind::Annotation,
                "analysis.hypothesis",
                "ws-042 was the initial foothold; the loader came over the beacon channel.",
            ))
            .put(text(
                Kind::Annotation,
                "analysis.note",
                "Ask IT whether ws-042 was reimaged in March.\n\n- EDR has no process events from 1 to 19 March.\n- The VPN log has jdoe logging on from ws-042 on 12 March.\n- Asset inventory lists a new disk serial from 20 March.\n\nIf it was reimaged, the foothold hypothesis needs another first host.",
            ))
            .put(Put {
                links: vec![
                    Link::entity("src", "host.name", "ws-042"),
                    Link::entity("dst", "net.ipv4", "198.51.100.7"),
                    Link::source("artifact:fw.csv"),
                ],
                ..Put::new(Kind::Event, "net.connection")
            }),
    )
    .unwrap()
    .ids;

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
        ids,
        _root: root,
    };
    cx.update_window(window, |_, window, cx| {
        rig.panel
            .update(cx, |p, cx| p.set_backend(Some(rig.backend.clone()), cx));
        // Twice: the first frame learns how big the drawing area is.
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    rig
}

impl Rig {
    fn plane(&self, cx: &mut TestAppContext) -> PlaneForTest {
        cx.update_window(self.window, |_, _, cx| {
            self.panel.read(cx).plane_for_test(cx)
        })
        .unwrap()
    }

    fn node(&self, cx: &mut TestAppContext, id: &str) -> ((f32, f32), (f32, f32), &'static str) {
        cx.update_window(self.window, |_, _, cx| {
            self.panel.read(cx).node_for_test(id).expect("the node")
        })
        .unwrap()
    }

    fn press(
        &self,
        cx: &mut TestAppContext,
        button: MouseButton,
        at: (f32, f32),
        modifiers: Modifiers,
    ) {
        cx.update_window(self.window, |_, window, cx| {
            let position = point(px(at.0), px(at.1));
            window.dispatch_event(
                MouseDownEvent {
                    button,
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
                    button,
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

    fn click(&self, cx: &mut TestAppContext, at: (f32, f32)) {
        self.press(cx, MouseButton::Left, at, Modifiers::default());
    }

    fn keys(&self, cx: &mut TestAppContext, keys: &str) {
        cx.update_window(self.window, |_, window, cx| {
            for key in keys.split(' ') {
                window.press(key, cx);
            }
        })
        .unwrap();
    }

    fn typed(&self, cx: &mut TestAppContext, text: &str) {
        cx.simulate_input(self.window, text);
        self.frame(cx);
    }

    fn frame(&self, cx: &mut TestAppContext) {
        cx.update_window(self.window, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }

    /// Somewhere on the plane that no node or card is on: the bottom, left of the layers.
    fn empty(&self) -> (f32, f32) {
        (900., 880.)
    }
}

/// Ctrl on Linux and Windows, Cmd on macOS: what `secondary` means.
fn secondary(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("cmd-{key}")
    } else {
        format!("ctrl-{key}")
    }
}

#[gpui_kit::test]
async fn nodes_are_as_big_as_what_they_say_and_do_not_lie_on_one_another(cx: &mut TestAppContext) {
    let r = rig(cx);
    assert_eq!(r.plane(cx).drawn, 5, "four that were put and the host");
    let (_, (path_w, path_h), size) = r.node(cx, &r.ids[0]);
    let (_, (ip_w, ip_h), _) = r.node(cx, &r.ids[1]);
    assert_eq!(size, "M");
    // The path is shown whole: wider than the address, and several lines tall.
    assert!(
        path_w > ip_w && path_h > ip_h * 2.,
        "{path_w}x{path_h} {ip_w}x{ip_h}"
    );
    assert!(
        path_w / path_h > 1.5 && path_w / path_h < 3.6,
        "{path_w}x{path_h}"
    );
    // No two cards overlap.
    let boxes: Vec<((f32, f32), (f32, f32))> = r.ids[..4]
        .iter()
        .map(|id| {
            let (at, extent, _) = r.node(cx, id);
            (at, extent)
        })
        .collect();
    for a in 0..boxes.len() {
        for b in (a + 1)..boxes.len() {
            let ((ax, ay), (aw, ah)) = boxes[a];
            let ((bx, by), (bw, bh)) = boxes[b];
            assert!(
                (ax - bx).abs() >= (aw + bw) / 2. - 0.5 || (ay - by).abs() >= (ah + bh) / 2. - 0.5,
                "{a} and {b} overlap: {:?} {:?}",
                boxes[a],
                boxes[b]
            );
        }
    }
}

#[gpui_kit::test]
async fn the_bracket_keys_size_every_node_or_the_selected_ones_and_zero_puts_them_back(
    cx: &mut TestAppContext,
) {
    let r = rig(cx);
    r.click(cx, r.empty());
    r.keys(cx, "]");
    assert_eq!(r.plane(cx).size, "L");
    r.keys(cx, "] ]");
    assert_eq!(r.plane(cx).size, "XL", "it stops at the largest");
    r.keys(cx, "[ [ [ [");
    assert_eq!(r.plane(cx).size, "S");
    r.keys(cx, "0");
    assert_eq!(r.plane(cx).size, "M");

    // With a node selected the keys are that node's.
    let note = r.ids[3].clone();
    let (at, (_, h_m), _) = r.node(cx, &note);
    r.click(cx, at);
    r.keys(cx, "] ]");
    let (_, (w_xl, h_xl), size) = r.node(cx, &note);
    assert_eq!((size, r.plane(cx).size), ("XL", "M"));
    assert!(
        h_xl > h_m * 1.5,
        "the whole text is taller: {h_m} then {h_xl}"
    );
    assert!(w_xl >= 300., "{w_xl}");
    let (_, _, other) = r.node(cx, &r.ids[0]);
    assert_eq!(other, "M", "the others stay as they were");
    // It is kept with the case.
    let kept = reactor_db::graph::layout::load_sizes(r.backend.case.db()).unwrap();
    assert_eq!(kept[&note].size.as_deref(), Some("xl"));
    r.keys(cx, "0");
    assert_eq!(r.node(cx, &note).2, "M");
    assert!(
        reactor_db::graph::layout::load_sizes(r.backend.case.db())
            .unwrap()
            .is_empty()
    );
}

#[gpui_kit::test]
async fn find_rings_what_matches_steps_through_it_and_hides_nothing(cx: &mut TestAppContext) {
    let r = rig(cx);
    r.click(cx, r.empty());
    r.keys(cx, &secondary("f"));
    assert!(r.plane(cx).find_open);
    r.typed(cx, "beacon");
    let p = r.plane(cx);
    // The hypothesis has it in its text, the address in a tag.
    assert_eq!((p.found, p.find_at, p.drawn), (2, 0, 5), "{p:?}");
    r.keys(cx, "enter");
    assert_eq!(r.plane(cx).find_at, 1);
    r.keys(cx, "enter");
    assert_eq!(r.plane(cx).find_at, 0, "round to the first");
    r.keys(cx, "shift-enter");
    assert_eq!(r.plane(cx).find_at, 1);
    // A bracket typed into find is a character, not a size.
    r.typed(cx, "]");
    let p = r.plane(cx);
    assert_eq!((p.size, p.found), ("M", 0), "{p:?}");
    r.keys(cx, "escape");
    let p = r.plane(cx);
    assert!(!p.find_open && p.found == 0, "{p:?}");
    // The plane has the keys again.
    r.keys(cx, "]");
    assert_eq!(r.plane(cx).size, "L");
}

#[gpui_kit::test]
async fn filter_layers_stack_on_one_another(cx: &mut TestAppContext) {
    let r = rig(cx);
    let add = |cx: &mut TestAppContext, text: &'static str, mode: &'static str| {
        cx.update_window(r.window, |_, window, cx| {
            r.panel
                .update(cx, |p, cx| p.add_layer_for_test(text, mode, window, cx));
            window.render_frame(cx);
        })
        .unwrap();
    };
    add(cx, "kind:entity", "only");
    assert_eq!(r.plane(cx).drawn, 3, "the path, the address and the host");
    add(cx, "type:file.", "hide");
    let p = r.plane(cx);
    assert_eq!((p.drawn, p.layers, p.layers_open), (2, 2, true), "{p:?}");
    // Dimming and highlighting hide nothing.
    add(cx, "tag:c2.", "dim");
    add(cx, "conf:none", "highlight");
    assert_eq!(r.plane(cx).drawn, 2);
    // Find only looks in what is drawn.
    r.click(cx, r.empty());
    r.keys(cx, &secondary("f"));
    r.typed(cx, "beacon");
    assert_eq!(r.plane(cx).found, 1, "the hypothesis is filtered out");
}

#[gpui_kit::test]
async fn a_right_click_on_a_node_opens_its_types_look_and_esc_closes_it(cx: &mut TestAppContext) {
    let r = rig(cx);
    let (at, _, _) = r.node(cx, &r.ids[1]);
    r.press(cx, MouseButton::Right, at, Modifiers::default());
    assert_eq!(r.plane(cx).picking.as_deref(), Some("net.ipv4"));
    r.keys(cx, "escape");
    assert_eq!(r.plane(cx).picking, None);
    // The look itself is the vocabulary's: set, it is what the type is drawn with.
    vocab::set_look(
        r.backend.case.db(),
        "net.ipv4",
        Kind::Entity,
        Some("radar"),
        Some("teal"),
    )
    .unwrap();
    let v = vocab::load(r.backend.case.db()).unwrap();
    let look = reactor_gui::graph::look::look(&v, "net.ipv4", Kind::Entity);
    assert_eq!(
        (look.icon.as_str(), look.colour_name.as_str(), look.own),
        ("radar", "teal", true)
    );
}

#[gpui_kit::test]
async fn the_toolbars_size_buttons_do_what_the_keys_do(cx: &mut TestAppContext) {
    let r = rig(cx);
    let click = |cx: &mut TestAppContext, id: &'static str| {
        cx.update_window(r.window, |_, window, cx| {
            window.click(id, cx);
            window.render_frame(cx);
        })
        .unwrap();
    };
    click(cx, "graph-size-larger");
    assert_eq!(r.plane(cx).size, "L");
    click(cx, "graph-size-smaller");
    click(cx, "graph-size-smaller");
    assert_eq!(r.plane(cx).size, "S");
}

#[gpui_kit::test]
async fn a_view_keeps_the_layers_and_size_and_puts_them_back_in_a_click(cx: &mut TestAppContext) {
    let r = rig(cx);
    let on_panel = |cx: &mut TestAppContext,
                    f: &dyn Fn(
        &mut GraphPanel,
        &mut gpui_kit::Window,
        &mut gpui_kit::Context<GraphPanel>,
    )| {
        cx.update_window(r.window, |_, window, cx| {
            r.panel.update(cx, |p, cx| f(p, window, cx));
            window.render_frame(cx);
        })
        .unwrap();
    };
    on_panel(cx, &|p, w, cx| {
        p.add_layer_for_test("kind:entity", "only", w, cx);
        p.add_layer_for_test("type:file.", "hide", w, cx);
        p.stage_for_test("size:l", w, cx);
    });
    assert_eq!(r.plane(cx).drawn, 2);
    on_panel(cx, &|p, w, cx| {
        p.save_view_for_test("entities, no files", w, cx)
    });
    assert_eq!(r.plane(cx).views, ["entities, no files"]);
    // It is kept with the case, not only in the window.
    let kept = reactor_db::graph::views::list(r.backend.case.db()).unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].size.as_deref(), Some("l"));
    assert_eq!(kept[0].layers.len(), 2);

    // Set the graph up another way, then put the view back with a click on its chip.
    on_panel(cx, &|p, w, cx| {
        p.stage_for_test("size:s", w, cx);
        p.add_layer_for_test("tag:c2.", "hide", w, cx);
    });
    assert_eq!(r.plane(cx).size, "S");
    cx.update_window(r.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(("graph-view", 0usize), cx);
        window.render_frame(cx);
    })
    .unwrap();
    let p = r.plane(cx);
    assert_eq!((p.size, p.layers, p.drawn), ("L", 2, 2), "{p:?}");
    let layers = cx
        .update_window(r.window, |_, _, cx| r.panel.read(cx).layers_for_test(cx))
        .unwrap();
    assert_eq!(layers, ["kind:entity|only|true", "type:file.|hide|true"]);
    // Taking it back leaves the graph as it is.
    cx.update_window(r.window, |_, window, cx| {
        window.click(("graph-view-delete", 0usize), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert!(r.plane(cx).views.is_empty());
    assert!(
        reactor_db::graph::views::list(r.backend.case.db())
            .unwrap()
            .is_empty()
    );
    assert_eq!(r.plane(cx).layers, 2);
}
