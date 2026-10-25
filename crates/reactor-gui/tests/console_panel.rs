//! The console tab in a real (headless) window, with a program behind it: it starts when it is
//! first drawn, keeps the program's output when that ends, and the tab names what runs in it.
#![recursion_limit = "256"]

use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext, Bounds, Entity, Point, TestAppContext, WeakEntity, WindowBounds,
    WindowOptions, px, size,
};

use reactor_gui::app::ReactorApp;
use reactor_gui::panels::ConsolePanel;

fn open(
    cx: &mut TestAppContext,
    program: &str,
    args: &[&str],
) -> (AnyWindowHandle, Entity<ConsolePanel>) {
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let program = program.to_owned();
    cx.update(|cx| {
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(900.), px(300.)),
        };
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                // No app behind it: the panel copes, it just cannot refresh the catalogue.
                cx.new(|cx| {
                    ConsolePanel::new(
                        WeakEntity::<ReactorApp>::new_invalid(),
                        None,
                        Some((program, args)),
                        window,
                        cx,
                    )
                })
            },
        )
        .expect("open test window")
    })
}

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
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
        if done(cx) {
            return;
        }
        assert!(Instant::now() < limit, "gave up waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[gpui_kit::test]
fn a_failing_command_keeps_its_output_and_the_panel_draws_the_strip(cx: &mut TestAppContext) {
    let (window, panel) = open(cx, "sh", &["-c", "echo kept-output; exit 3"]);
    until(cx, window, "the command to end", |cx| {
        cx.update_window(window, |_, _, cx| {
            panel.update(cx, |p, cx| p.exit_code(cx)) == Some(Some(3))
        })
        .unwrap()
    });
    // A few more frames with the strip showing: the draw must hold up.
    until(cx, window, "the output on the grid", |cx| {
        cx.update_window(window, |_, _, cx| {
            panel.update(cx, |p, cx| p.screen_text(cx).contains("kept-output"))
        })
        .unwrap()
    });
}

#[gpui_kit::test]
fn the_tab_names_the_command_it_runs(cx: &mut TestAppContext) {
    let (window, panel) = open(cx, "sh", &["-c", "sleep 30"]);
    until(cx, window, "the program to run", |cx| {
        cx.update_window(window, |_, _, cx| {
            panel.update(cx, |p, cx| p.is_running(cx))
        })
        .unwrap()
    });
    let label = cx
        .update_window(window, |_, _, cx| panel.update(cx, |p, cx| p.tab_label(cx)))
        .unwrap();
    assert_eq!(label, "sh -c sleep 30");
}
