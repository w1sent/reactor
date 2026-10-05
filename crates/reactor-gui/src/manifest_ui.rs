//! The two popups of the manifest: the editor for the goal and the guidelines, and the Frame
//! -- what the model is told (*As sent*) and what is kept (*State*). Both are gpui-kit dialogs
//! whose content is built again on every frame from the app's state, like the context window's
//! ([`crate::inspect_ui`]); the state lives on [`ReactorApp`].

use gpui_kit::base::{SelectableText, h_flex, v_flex};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::label::Label;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, SharedString, div, px};

use crate::app::ReactorApp;
use crate::plan_ui::Frame;

/// The goal-and-guidelines editor, while it is open.
pub struct GoalEditor {
    pub goal: Entity<TextareaState>,
    pub guidelines: Entity<TextareaState>,
    pub error: Option<String>,
}

/// The Frame popup, while it is open.
pub struct FramePopup {
    /// 0 is *As sent*, 1 is *State*.
    pub tab: usize,
    pub frame: Frame,
    /// An estimate of what *As sent* costs every request.
    pub tokens: u64,
}

/// The editor's contents.
pub fn editor_view(weak: &gpui_kit::WeakEntity<ReactorApp>, cx: &mut App) -> AnyElement {
    let Some(app) = weak.upgrade() else {
        return div().into_any_element();
    };
    let app = app.read(cx);
    let Some(ed) = app.goal_editor.as_ref() else {
        return div().into_any_element();
    };
    let theme = cx.theme().clone();
    let muted = theme.muted_foreground;
    v_flex()
        .gap_3()
        .child(
            v_flex()
                .gap_1()
                .child(Label::new("Goal").text_color(muted).text_size(theme.font_size * 0.9))
                .child(
                    div()
                        .text_color(muted)
                        .text_size(theme.font_size * 0.8)
                        .child("What this case is for: one or two sentences. The agent keeps its steps against it."),
                )
                .child(Textarea::new(&ed.goal).h(px(110.))),
        )
        .child(
            v_flex()
                .gap_1()
                .child(Label::new("Guidelines").text_color(muted).text_size(theme.font_size * 0.9))
                .child(
                    div()
                        .text_color(muted)
                        .text_size(theme.font_size * 0.8)
                        .child("How to work: constraints and habits, one per line."),
                )
                .child(Textarea::new(&ed.guidelines).h(px(170.))),
        )
        .child(
            div()
                .text_color(muted)
                .text_size(theme.font_size * 0.8)
                .child("The goal and the guidelines belong to the case: every session of it uses them. Steps are the agent's, per session."),
        )
        .children(ed.error.clone().map(|e| div().text_color(theme.danger).child(SharedString::from(e))))
        .into_any_element()
}

/// The Frame popup's contents.
pub fn frame_view(
    weak: &gpui_kit::WeakEntity<ReactorApp>,
    height: gpui_kit::Pixels,
    cx: &mut App,
) -> AnyElement {
    let Some(app) = weak.upgrade() else {
        return div().into_any_element();
    };
    let app = app.read(cx);
    let Some(popup) = app.frame_popup.as_ref() else {
        return div().into_any_element();
    };
    let theme = cx.theme().clone();
    let muted = theme.muted_foreground;
    let w = weak.clone();
    let tabs = TabBar::new("frame-tabs")
        .selected_index(popup.tab)
        .children([Tab::new().label("As sent"), Tab::new().label("State")])
        .on_click(move |ix: &usize, _w, cx| {
            let ix = *ix;
            w.update(cx, |app, cx| {
                if let Some(p) = &mut app.frame_popup {
                    p.tab = ix;
                }
                cx.notify();
            })
            .ok();
        });
    let (text, note) = match popup.tab {
        0 => match &popup.frame.sent {
            Some(sent) => (
                sent.clone(),
                format!("What the model is told, as it is sent: about {} tokens in every request.", popup.tokens),
            ),
            None => (
                String::new(),
                popup.frame.why_not.unwrap_or("Nothing is sent.").to_string(),
            ),
        },
        _ => (
            popup.frame.state.clone(),
            "What is kept, as /frame prints it. The goal and guidelines are the case's; the steps and the switch are this session's.".to_string(),
        ),
    };
    v_flex()
        .gap_2()
        .child(tabs)
        .child(
            div()
                .text_color(muted)
                .text_size(theme.font_size * 0.85)
                .child(SharedString::from(note)),
        )
        .child(
            div()
                .id("frame-text")
                .h(height)
                .w_full()
                .overflow_y_scrollbar()
                .p_2()
                .rounded_md()
                .bg(theme.secondary)
                .font_family(theme.mono_font_family.clone())
                .text_size(theme.mono_font_size)
                .child(SelectableText::new(
                    SharedString::from(format!("frame-text-{}", popup.tab)),
                    text,
                )),
        )
        .into_any_element()
}

/// The buttons under the frame.
pub fn frame_footer(weak: gpui_kit::WeakEntity<ReactorApp>) -> impl IntoElement {
    let (w_inspect, w_copy) = (weak.clone(), weak);
    h_flex()
        .w_full()
        .gap_2()
        .child(
            Button::new("frame-inspect")
                .label("Show in the context window")
                .tooltip("Where the manifest is in the request, and what it costs")
                .on_click(move |_, window, cx| {
                    let _ = w_inspect.update(cx, |app, cx| {
                        app.close_frame(window, cx);
                        app.show_kind(reactor_agent::inspect::Kind::Manifest, window, cx);
                    });
                }),
        )
        .child(div().flex_1())
        .child(
            Button::new("frame-copy")
                .label("Copy")
                .on_click(move |_, _w, cx| {
                    let _ = w_copy.update(cx, |app, cx| app.copy_frame(cx));
                }),
        )
        .child(
            Button::new("frame-close")
                .label("Close")
                .primary()
                .on_click(|_, window, cx| {
                    gpui_kit::component::WindowExt::close_dialog(window, cx);
                }),
        )
}
