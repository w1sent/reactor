//! Notifications on screen: the bell in the title bar and the history list it opens. (The popups
//! are gpui-kit's notification component.) The rules are in [`crate::notifications`].

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, Theme};
use gpui_kit::prelude::*;
use gpui_kit::{App, FocusHandle, Hsla, MouseButton, div, px, relative};

use crate::app::ReactorApp;
use crate::notifications::Level;

/// The history popup's state.
pub struct NoticesUi {
    pub focus: FocusHandle,
}

fn colour(level: Level, theme: &Theme) -> Hsla {
    match level {
        Level::Error => theme.danger,
        Level::Warning => theme.warning,
        Level::Info => theme.accent,
    }
}

/// The bell for the title bar, with a count of what has not been looked at.
pub fn bell(app: &ReactorApp, weak: gpui_kit::WeakEntity<ReactorApp>, cx: &App) -> gpui_kit::AnyElement {
    let theme = cx.theme();
    let unread = app.notifier.unread();
    div()
        .relative()
        .child(
            Button::new("notices-bell")
                .icon(IconName::Bell)
                .ghost()
                .small()
                .tooltip("Notifications")
                .on_click(move |_, window, cx| {
                    weak.update(cx, |app, cx| app.toggle_notices(window, cx)).ok();
                }),
        )
        .when(unread > 0, |el| {
            el.child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .min_w(px(14.))
                    .h(px(14.))
                    .px_1()
                    .rounded_full()
                    .bg(theme.accent)
                    .text_color(theme.accent_foreground)
                    .text_size(px(9.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(if unread > 99 { "99+".to_string() } else { unread.to_string() }),
            )
        })
        .into_any_element()
}

/// The history: every notification, newest first, under the bell.
pub fn history(app: &ReactorApp, weak: gpui_kit::WeakEntity<ReactorApp>, cx: &App) -> Option<impl IntoElement> {
    let state = app.notices.as_ref()?;
    let theme = cx.theme();
    let (w_clear, w_close, w_scrim) = (weak.clone(), weak.clone(), weak);

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    // Wide enough for the date when some notice is from another day, so the times stay in a column.
    let stamp_width = if app.notifier.history().any(|n| n.date != today) { px(170.) } else { px(76.) };
    let mut list = v_flex().id("notices-list").flex_1().min_h_0().overflow_y_scroll().gap_1();
    let mut any = false;
    for notice in app.notifier.history() {
        any = true;
        list = list.child(
            h_flex()
                .gap_2()
                .items_start()
                .px_2()
                .py_1()
                .rounded_md()
                .hover(|r| r.bg(theme.list_hover))
                .child(div().mt(px(6.)).size(px(8.)).flex_none().rounded_full().bg(colour(notice.level, theme)))
                .child(div().w(stamp_width).flex_none().whitespace_nowrap().font_family(theme.mono_font_family.clone()).text_size(theme.mono_font_size).text_color(theme.muted_foreground).child(crate::notifications::Notifier::stamp(notice, &today)))
                .child(div().flex_1().min_w_0().child(notice.message.clone())),
        );
    }
    if !any {
        list = list.child(div().p_3().text_color(theme.muted_foreground).child("No notifications"));
    }

    let card = v_flex()
        .id("notices-card")
        .absolute()
        .top(px(46.))
        .right_4()
        .w(px(620.))
        .max_h(relative(0.7))
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .shadow_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(Label::new("Notifications"))
                .child(
                    h_flex()
                        .gap_1()
                        .child(Button::new("notices-clear").label("Clear").small().ghost().on_click(move |_, _w, cx| {
                            w_clear.update(cx, |app, cx| app.clear_notices(cx)).ok();
                        }))
                        .child(Button::new("notices-close").icon(IconName::Close).small().ghost().on_click(move |_, window, cx| {
                            w_close.update(cx, |app, cx| app.close_notices(window, cx)).ok();
                        })),
                ),
        )
        .child(list);

    Some(
        div()
            .id("notices-scrim")
            .key_context("ReactorNotices")
            .track_focus(&state.focus)
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                w_scrim.update(cx, |app, cx| app.close_notices(window, cx)).ok();
            })
            .child(card),
    )
}

