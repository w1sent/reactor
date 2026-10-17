//! The transcript's list, driven in a real (headless) window: a row that grows while the
//! person has scrolled up must be laid out at its new height at once, without a scroll to
//! prompt it, and without moving what they are reading.
#![recursion_limit = "256"]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::prelude::*;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext, Bounds, Context, Entity, Pixels, Point, TestAppContext, Window, WindowBounds,
    WindowOptions, canvas, div, px, size,
};

const LINE: f32 = 20.;

type Rows = Rc<RefCell<Vec<u32>>>;
type Seen = Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>;

/// How the host tells the list a row changed.
#[derive(Clone, Copy)]
enum Remeasure {
    /// Every row, as the transcript did.
    All,
    /// The row that changed.
    Row(usize),
}

struct Host {
    rows: Rows,
    seen: Seen,
    scroller: Entity<MessageScrollerState>,
}

impl gpui_kit::Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (rows, seen) = (self.rows.clone(), self.seen.clone());
        div().size_full().child(
            MessageScroller::new(
                "transcript",
                self.scroller.clone(),
                move |index, _window, _cx| {
                    let seen = seen.clone();
                    let lines = rows.borrow()[index];
                    // A row is as tall as it has lines, as wrapped text is.
                    div().relative().w_full().h(px(lines as f32 * LINE)).child(
                        canvas(
                            move |bounds, _, _| {
                                seen.borrow_mut().insert(index, bounds);
                            },
                            |_, _, _, _| {},
                        )
                        .size_full(),
                    )
                },
            )
            .with_row_style(gpui_kit::StyleRefinement::default().px_0().pb_0p5()),
        )
    }
}

fn open(cx: &mut TestAppContext) -> (gpui_kit::AnyWindowHandle, Entity<Host>, Rows, Seen) {
    cx.update(gpui_kit::init);
    let rows: Rows = Rc::new(RefCell::new(vec![3; 30]));
    let seen: Seen = Rc::default();
    let (window, host) = cx.update(|cx| {
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(800.), px(400.)),
        };
        let (rows, seen) = (rows.clone(), seen.clone());
        let (window, host) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            move |_, cx| {
                cx.new(|cx| Host {
                    rows,
                    seen,
                    scroller: cx.new(|cx| MessageScrollerState::new(30, cx)),
                })
            },
        )
        .expect("open test window");
        (window, host)
    });
    frames(cx, window, 2);
    (window, host, rows, seen)
}

fn frames(cx: &mut TestAppContext, window: gpui_kit::AnyWindowHandle, n: usize) {
    for _ in 0..n {
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
}

/// Scroll up so the last row is partly on screen but the list no longer follows its tail, grow
/// the last row by `extra` lines, tell the list the way `how` says, and draw once.
/// Returns the height the last row was drawn at and where the row at the top was.
fn grow_while_scrolled_up(cx: &mut TestAppContext, how: Remeasure, extra: u32) -> (f32, f32) {
    let mut result = (0., 0.);
    let (window, host, rows, seen) = open(cx);
    cx.update_window(window, |_, _, cx| {
        let scroller = host.read(cx).scroller.clone();
        scroller.update(cx, |state, cx| {
            assert!(state.scroll_to_item(23, cx));
        });
    })
    .unwrap();
    frames(cx, window, 2);
    let top_before = seen.borrow()[&23].origin.y;
    assert!(
        seen.borrow().contains_key(&29),
        "the last row is on screen before it grows"
    );

    rows.borrow_mut()[29] += extra;
    cx.update_window(window, |_, _, cx| {
        host.update(cx, |host, cx| {
            host.scroller.update(cx, |state, cx| match how {
                Remeasure::All => state.remeasure(cx),
                Remeasure::Row(i) => {
                    state.remeasure_items(i..i + 1, cx);
                }
            });
            cx.notify();
        });
    })
    .unwrap();
    frames(cx, window, 2);
    result.0 = f32::from(seen.borrow()[&29].size.height);
    result.1 = f32::from(seen.borrow()[&23].origin.y) - f32::from(top_before);
    result
}

#[gpui_kit::test]
async fn a_growing_last_row_is_drawn_at_its_new_height_when_every_row_is_remeasured(
    cx: &mut TestAppContext,
) {
    let (height, moved) = grow_while_scrolled_up(cx, Remeasure::All, 3);
    assert_eq!(height, 6. * LINE, "the last row has its new height");
    assert_eq!(moved, 0., "what is being read stays where it was");
}

#[gpui_kit::test]
async fn a_growing_last_row_is_drawn_at_its_new_height_when_only_it_is_remeasured(
    cx: &mut TestAppContext,
) {
    let (height, moved) = grow_while_scrolled_up(cx, Remeasure::Row(29), 3);
    assert_eq!(height, 6. * LINE, "the last row has its new height");
    assert_eq!(moved, 0., "what is being read stays where it was");
}
