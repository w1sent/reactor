//! The view in a real (headless) window, with a real pty behind it: the grid is as big as the
//! window allows, keys reach the program, a drag selects text, the wheel scrolls the history, and
//! a change of focus is reported to a program that asked.

use std::time::{Duration, Instant};

use gpui_kit::InputEvent as _;
use gpui_kit::prelude::*;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext, Bounds, Context, Entity, FocusHandle, Focusable, Keystroke,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Point, ScrollDelta,
    ScrollWheelEvent, TestAppContext, TouchPhase, Window, WindowBounds, WindowOptions, div, px,
    size,
};

use super::keys;
use super::session::Kind;
use super::types::*;
use super::view::{Blink, Look, TerminalView};

fn look() -> Look {
    Look {
        family: "Courier".into(),
        size: px(16.),
        min_contrast: 1.0,
        cursor: CursorShape::Block,
        blink: Blink::Off,
        option_as_alt: false,
        shell: None,
        scrollback: 1000,
        copy_on_select: false,
    }
}

struct Host {
    terminal: Entity<TerminalView>,
    elsewhere: FocusHandle,
}

impl gpui_kit::Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(div().size_full().child(self.terminal.clone()))
            .child(div().id("elsewhere").track_focus(&self.elsewhere))
    }
}

fn open(
    cx: &mut TestAppContext,
    script: &str,
) -> (AnyWindowHandle, Entity<Host>, Entity<TerminalView>) {
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let (window, host) = cx.update(|cx| {
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(800.), px(400.)),
        };
        let script = script.to_owned();
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                let terminal = cx.new(|cx| {
                    TerminalView::new(
                        Kind::Task {
                            program: "sh".into(),
                            args: vec!["-c".into(), script],
                        },
                        None,
                        look(),
                        window,
                        cx,
                    )
                });
                cx.new(|cx| Host {
                    terminal,
                    elsewhere: cx.focus_handle(),
                })
            },
        )
        .expect("open test window")
    });
    let terminal = cx
        .update_window(window, |_, _, cx| host.read(cx).terminal.clone())
        .unwrap();
    (window, host, terminal)
}

fn frame(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .unwrap();
}

/// Let the pty's thread, the panel's tasks and the clock run until `done`, drawing as it goes.
fn until(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    what: &str,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    let limit = Instant::now() + Duration::from_secs(20);
    loop {
        cx.executor().advance_clock(Duration::from_millis(20));
        cx.run_until_parked();
        frame(cx, window);
        if done(cx) {
            return;
        }
        assert!(Instant::now() < limit, "gave up waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn screen(cx: &mut TestAppContext, view: &Entity<TerminalView>) -> Vec<String> {
    cx.update(|cx| {
        let c = view.read(cx).backend.snapshot();
        (0..c.rows)
            .map(|r| c.row_text(r).trim_end().to_owned())
            .collect()
    })
}

fn has_line(cx: &mut TestAppContext, view: &Entity<TerminalView>, text: &str) -> bool {
    screen(cx, view).iter().any(|l| l.contains(text))
}

#[gpui_kit::test]
fn the_session_starts_on_the_first_layout_with_the_grid_the_window_allows(cx: &mut TestAppContext) {
    let (window, _host, view) = open(cx, "sleep 30");
    // A console that was made but never shown (the first one, with the dock collapsed) has run
    // nothing: it starts when it is first laid out.
    let unseen = cx
        .update_window(window, |_, window, cx| {
            cx.new(|cx| {
                TerminalView::new(
                    Kind::Task {
                        program: "sleep".into(),
                        args: vec!["30".into()],
                    },
                    None,
                    look(),
                    window,
                    cx,
                )
            })
        })
        .unwrap();
    frame(cx, window);
    cx.run_until_parked();
    assert!(
        cx.update(|cx| unseen.read(cx).session.is_none()),
        "nothing runs before it is laid out"
    );
    assert!(
        cx.update(|cx| view.read(cx).session.is_some()),
        "the one in the window does"
    );
    let (rows, cols, metrics) = cx.update(|cx| {
        let v = view.read(cx);
        (v.size.rows, v.size.cols, v.metrics)
    });
    assert_eq!((rows as usize, cols as usize), (metrics.rows, metrics.cols));
    assert!(rows >= 5 && cols >= 20, "{rows}x{cols}");
    // What fits: the window's height in lines, and its width in cells less a margin and the bar.
    let (r, c) = super::metrics::grid_for(800., 400., metrics.cell_w, metrics.line_h);
    assert_eq!((rows as usize, cols as usize), (r, c));
    cx.update(|cx| {
        let snap = view.read(cx).backend.snapshot();
        assert_eq!((snap.rows, snap.cols), (r, c));
    });
}

#[gpui_kit::test]
fn typed_keys_reach_the_program_and_come_back_on_the_grid(cx: &mut TestAppContext) {
    let (window, _host, view) = open(cx, "exec cat");
    frame(cx, window);
    cx.update_window(window, |_, window, cx| {
        let focus = view.read(cx).focus.clone();
        window.focus(&focus, cx);
    })
    .unwrap();
    frame(cx, window);
    for key in ["h->h", "i->i", "enter"] {
        cx.dispatch_keystroke(window, Keystroke::parse(key).unwrap());
    }
    until(cx, window, "the echo of what was typed", |cx| {
        has_line(cx, &view, "hi")
    });
    // `cat` was given a line, so it answers with it.
    until(cx, window, "cat's answer", |cx| {
        screen(cx, &view)
            .iter()
            .filter(|l| l.trim() == "hi")
            .count()
            >= 2
    });
}

#[gpui_kit::test]
fn a_drag_selects_and_the_text_is_copied(cx: &mut TestAppContext) {
    let (window, _host, view) = open(cx, "printf 'hello world\\n'; sleep 30");
    until(cx, window, "the output", |cx| {
        has_line(cx, &view, "hello world")
    });
    let m = cx.update(|cx| view.read(cx).metrics);
    let y = m.origin_y + m.line_h / 2.0;
    let at = |col: f32| Point::new(px(m.origin_x + col * m.cell_w), px(y));
    cx.update_window(window, |_, window, cx| {
        let down = MouseDownEvent {
            button: MouseButton::Left,
            position: at(0.1),
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        };
        window.dispatch_event(down.to_platform_input(), cx);
        let moved = MouseMoveEvent {
            position: at(4.9),
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        };
        window.dispatch_event(moved.to_platform_input(), cx);
        let up = MouseUpEvent {
            button: MouseButton::Left,
            position: at(4.9),
            modifiers: Modifiers::default(),
            click_count: 1,
        };
        window.dispatch_event(up.to_platform_input(), cx);
    })
    .unwrap();
    let copied = cx.update(|cx| view.read(cx).backend.selected_text());
    assert_eq!(copied.as_deref(), Some("hello"));
}

#[gpui_kit::test]
fn the_wheel_scrolls_the_history(cx: &mut TestAppContext) {
    let (window, _host, view) = open(cx, "seq 1 200; sleep 30");
    until(cx, window, "the output", |cx| has_line(cx, &view, "200"));
    assert_eq!(cx.update(|cx| view.read(cx).backend.display_offset()), 0);
    let m = cx.update(|cx| view.read(cx).metrics);
    cx.update_window(window, |_, window, cx| {
        let wheel = ScrollWheelEvent {
            position: Point::new(px(m.origin_x + 20.), px(m.origin_y + 20.)),
            delta: ScrollDelta::Lines(Point::new(0., 2.)),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        };
        window.dispatch_event(wheel.to_platform_input(), cx);
    })
    .unwrap();
    let offset = cx.update(|cx| view.read(cx).backend.display_offset());
    assert!(
        offset >= 3,
        "two notches scroll several lines, not {offset}"
    );
    // Typing goes back to the live end.
    cx.update_window(window, |_, window, cx| {
        let focus = view.read(cx).focus.clone();
        window.focus(&focus, cx);
    })
    .unwrap();
    cx.dispatch_keystroke(window, Keystroke::parse("x->x").unwrap());
    assert_eq!(cx.update(|cx| view.read(cx).backend.display_offset()), 0);
}

#[gpui_kit::test]
fn a_program_that_asked_is_told_when_the_focus_comes_and_goes(cx: &mut TestAppContext) {
    // Turn focus reporting on, then show every byte the program is sent.
    let (window, host, view) = open(cx, "printf '\\033[?1004h'; exec cat -v");
    until(cx, window, "the mode", |cx| {
        cx.update(|cx| view.read(cx).backend.modes().focus_in_out)
    });
    // Focus is only reported for a window that is active.
    cx.update_window(window, |_, window, _| window.activate_window())
        .unwrap();
    frame(cx, window);
    cx.update_window(window, |_, window, cx| {
        let focus = view.read(cx).focus.clone();
        window.focus(&focus, cx);
    })
    .unwrap();
    until(cx, window, "focus in", |cx| has_line(cx, &view, "^[[I"));
    cx.update_window(window, |_, window, cx| {
        let other = host.read(cx).elsewhere.clone();
        window.focus(&other, cx);
    })
    .unwrap();
    until(cx, window, "focus out", |cx| has_line(cx, &view, "^[[O"));
}

#[gpui_kit::test]
fn a_program_that_ends_leaves_its_output_and_an_exit_code(cx: &mut TestAppContext) {
    let (window, _host, view) = open(cx, "echo done-here; exit 7");
    until(cx, window, "the exit", |cx| {
        cx.update(|cx| view.read(cx).exit().is_some())
    });
    assert_eq!(
        cx.update(|cx| view.read(cx).exit()).and_then(|e| e.code),
        Some(7)
    );
    until(cx, window, "the output", |cx| {
        has_line(cx, &view, "done-here")
    });
    assert!(!cx.update(|cx| view.read(cx).running()));
    let _ = view.read_with(cx, |v, cx| v.focus_handle(cx));
}

/// Every key chord the terminal would send to a program that some binding would take first.
fn shadowed_keys(cx: &mut TestAppContext) -> Vec<String> {
    use gpui_kit::KeyContext;
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::app::bind_keys(cx);
    });
    let stack = [
        KeyContext::parse("Root").unwrap(),
        KeyContext::parse("Terminal").unwrap(),
    ];
    let mut names: Vec<String> = ('a'..='z').chain('0'..='9').map(String::from).collect();
    names.extend(
        [
            "space",
            "enter",
            "tab",
            "escape",
            "backspace",
            "delete",
            "insert",
            "home",
            "end",
            "pageup",
            "pagedown",
            "up",
            "down",
            "left",
            "right",
            ",",
            ".",
            "/",
            ";",
            "'",
            "[",
            "]",
            "\\",
            "-",
            "=",
            "`",
        ]
        .map(String::from),
    );
    names.extend((1..=12).map(|n| format!("f{n}")));
    let mut found = Vec::new();
    cx.update(|cx| {
        let bindings = cx.key_bindings();
        let bindings = bindings.borrow();
        for name in &names {
            for mods in [
                "",
                "ctrl-",
                "alt-",
                "shift-",
                "ctrl-shift-",
                "ctrl-alt-",
                "alt-shift-",
                "ctrl-alt-shift-",
            ] {
                let text = format!("{mods}{name}");
                let Ok(ks) = Keystroke::parse(&text) else {
                    continue;
                };
                if keys::encode(&ks, Modes::default(), true).is_none() {
                    continue;
                }
                let (hit, _) = bindings.bindings_for_input(&[ks], &stack);
                if let Some(b) = hit.first() {
                    found.push(format!("{text} -> {}", b.action().name()));
                }
            }
        }
    });
    found
}

/// A focused terminal gets every key it sends to a program, except its own chords: gpui matches a
/// binding before the focused element sees a key, so each one that a window-wide binding has
/// would never reach the shell (Tab, which moves the focus; Ctrl-C, which copies).
#[cfg(not(target_os = "macos"))]
#[gpui_kit::test]
fn a_terminal_keeps_every_key_it_sends_except_its_own_chords(cx: &mut TestAppContext) {
    let taken = shadowed_keys(cx);
    let mut expected = vec![
        "ctrl-shift-a -> terminal::SelectAllText",
        "ctrl-shift-c -> terminal::CopySelection",
        "ctrl-shift-f -> terminal::OpenFind",
        "ctrl-shift-v -> terminal::PasteClipboard",
        "f3 -> terminal::FindNext",
        "shift-f3 -> terminal::FindPrevious",
        // The toolkit's own inspector, for developers of the toolkit.
        "ctrl-shift-i -> inspector::ToggleInspector",
    ];
    expected.sort_unstable();
    let mut taken_sorted: Vec<&str> = taken.iter().map(String::as_str).collect();
    taken_sorted.sort_unstable();
    assert_eq!(
        taken_sorted, expected,
        "keys a binding takes from a focused terminal"
    );
}
