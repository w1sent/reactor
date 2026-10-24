//! The transcript's rows in a real (headless) window: a tool call is closed until it is
//! clicked, and its output is on screen only while it is open.
#![recursion_limit = "256"]

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext, Bounds, Entity, Point, TestAppContext, WeakEntity, WindowBounds, WindowOptions, px,
    size,
};
use reactor_gui::panels::TranscriptPanel;
use reactor_gui::session::{ChatItem, Session};

fn call(name: &str, command: &str, output: &str, is_error: bool, done: bool) -> ChatItem {
    ChatItem::ToolCall {
        call_id: format!("call-{name}"),
        name: name.into(),
        args: serde_json::json!({ "command": command }),
        output: output.into(),
        is_error,
        done,
        entry: done.then_some(1),
    }
}

fn open(cx: &mut TestAppContext) -> (gpui_kit::AnyWindowHandle, Entity<TranscriptPanel>) {
    cx.update(gpui_kit::init);
    let (window, panel) = cx.update(|cx| {
        let session = cx.new(|_| {
            let mut s = Session::new();
            s.items = vec![
                ChatItem::User {
                    text: "Find the licence check.".into(),
                },
                ChatItem::AssistantText {
                    text: "Starting from the strings.".into(),
                    streaming: false,
                },
                // 1: a finished call with a long output.
                call(
                    "bash",
                    "strings -n 8 sample.bin",
                    "/lib64/ld-linux-x86-64.so.2\nlicence.dat\nlicence expired on %s\n",
                    false,
                    true,
                ),
                // 2: a failure.
                call(
                    "bash",
                    "gdb ./sample.bin",
                    "gdb: command not found",
                    true,
                    true,
                ),
                // 3: one still running.
                call("graph_write", "x", "writing node 1 of 2…", false, false),
                ChatItem::Error {
                    message: "The provider closed the stream.".into(),
                },
                ChatItem::Stats {
                    date: "2026-10-17".into(),
                    time: "14:32".into(),
                    duration_ms: 72_000,
                    tokens: 3410,
                    stream_ms: 65_000,
                },
            ];
            s
        });
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(1000.), px(2400.)),
        };
        let (window, panel) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let composer =
                    cx.new(|cx| gpui_kit::component::input::TextareaState::new(window, cx));
                cx.new(|cx| {
                    TranscriptPanel::new(
                        WeakEntity::new_invalid(),
                        "s1".into(),
                        session,
                        composer,
                        window,
                        cx,
                    )
                })
            },
        )
        .expect("open test window");
        (window, panel)
    });
    frames(cx, window, 3);
    (window, panel)
}

fn frames(cx: &mut TestAppContext, window: gpui_kit::AnyWindowHandle, n: usize) {
    for _ in 0..n {
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
}

fn present(
    cx: &mut TestAppContext,
    window: gpui_kit::AnyWindowHandle,
    id: (&'static str, usize),
) -> bool {
    cx.update_window(window, |_, window, _| window.try_find(id).is_some())
        .unwrap()
}

fn click(cx: &mut TestAppContext, window: gpui_kit::AnyWindowHandle, id: (&'static str, usize)) {
    cx.update_window(window, |_, window, cx| window.click(id, cx))
        .unwrap();
    frames(cx, window, 3);
}

#[gpui_kit::test]
async fn a_tool_call_is_closed_until_it_is_clicked(cx: &mut TestAppContext) {
    let (window, _panel) = open(cx);
    assert!(
        present(cx, window, ("tool-toggle", 2)),
        "the call has its row"
    );
    assert!(
        !present(cx, window, ("tool-copy", 2)),
        "its output (and Copy button) are not drawn while it is closed"
    );
    click(cx, window, ("tool-toggle", 2));
    assert!(
        present(cx, window, ("tool-copy", 2)),
        "a click opens it: the output is drawn"
    );
    click(cx, window, ("tool-toggle", 2));
    assert!(
        !present(cx, window, ("tool-copy", 2)),
        "a second click closes it again"
    );
}

#[gpui_kit::test]
async fn opening_one_call_leaves_the_others_closed(cx: &mut TestAppContext) {
    let (window, _panel) = open(cx);
    click(cx, window, ("tool-toggle", 3));
    assert!(present(cx, window, ("tool-copy", 3)));
    assert!(!present(cx, window, ("tool-copy", 2)));
    assert!(!present(cx, window, ("tool-copy", 4)));
}
