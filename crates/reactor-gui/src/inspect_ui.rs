//! The context-window popup (SPEC.md §6.3): the request the model would be sent next, piece by
//! piece, each piece labelled with where it comes from.
//!
//! The data is `reactor-agent`'s `ContextPreview` — the same request the loop builds — so this
//! shows what the model gets, not a reconstruction. The popup only draws; the state lives on
//! [`ReactorApp`] (`inspect`).

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, Theme};
use gpui_kit::prelude::*;
use gpui_kit::{App, Hsla, SharedString, div, px, relative};

use reactor_agent::inspect::{ContextPreview, Origin, Section};

use crate::app::ReactorApp;

/// The open popup.
pub struct InspectUi {
    pub data: Option<ContextPreview>,
    pub loading: bool,
    pub error: Option<String>,
    /// Index into `data.segments` of the piece shown on the right.
    pub selected: usize,
    /// Only pieces from this source, when set.
    pub filter: Option<Origin>,
}

/// How much of one piece the text pane draws; Copy always takes all of it.
const SHOW_CHARS: usize = 40_000;

fn colour(origin: Origin, theme: &Theme) -> Hsla {
    match origin {
        Origin::Base => theme.muted_foreground,
        Origin::Identity => theme.chart_1,
        Origin::Registry => theme.chart_2,
        Origin::Skills => theme.chart_3,
        Origin::Manifest => theme.chart_4,
        Origin::Reporting => theme.chart_5,
        Origin::Scenario => theme.info,
        Origin::ToolDefinition => theme.muted_foreground,
        Origin::User => theme.accent,
        Origin::Assistant => theme.foreground,
        Origin::ToolResult => theme.warning,
        Origin::Reduction => theme.success,
        Origin::Reminder => theme.danger,
    }
}

fn badge(origin: Origin, theme: &Theme) -> impl IntoElement {
    let c = colour(origin, theme);
    h_flex()
        .gap_1()
        .items_center()
        .child(div().size(px(8.)).rounded_full().bg(c))
        .child(div().text_color(c).text_size(theme.font_size * 0.85).child(origin.label()))
}

/// The popup's contents, for its dialog. The dialog builds this again on every frame, so it reads
/// the app's current state each time; `height` is what the lists may fill.
pub fn view(weak: &gpui_kit::WeakEntity<ReactorApp>, height: gpui_kit::Pixels, cx: &mut App) -> gpui_kit::AnyElement {
    let Some(app) = weak.upgrade() else { return div().into_any_element() };
    let app = app.read(cx);
    let Some(state) = app.inspect.as_ref() else { return div().into_any_element() };
    let weak = weak.clone();
    let theme = cx.theme().clone();
    let muted = theme.muted_foreground;
    let (w_refresh, w_copy, w_close) = (weak.clone(), weak.clone(), weak.clone());

    let header = h_flex()
        .justify_between()
        .items_center()
        .child(
            h_flex().gap_1().items_center().child(Label::new("Context window").text_size(theme.font_size * 1.3)).child(crate::hints::info_button(
                "inspect-info",
                "The request the model would be sent next, in order: the system prompt's parts, the tool definitions, then the messages. Sizes are estimates (about four characters to a token).",
            )),
        )
        .child(
            h_flex()
                .gap_2()
                .child(Button::new("inspect-refresh").label("Refresh").small().disabled(state.loading).on_click(move |_, _w, cx| {
                    w_refresh.update(cx, |app, cx| app.refresh_inspect(cx)).ok();
                }))
                .child(Button::new("inspect-copy").label("Copy all").small().tooltip("Copy the whole request as text").on_click(move |_, _w, cx| {
                    w_copy.update(cx, |app, cx| app.copy_inspect(cx)).ok();
                }))
                .child(Button::new("inspect-close").label("Close").small().primary().on_click(move |_, window, cx| {
                    w_close.update(cx, |app, cx| app.close_inspect(window, cx)).ok();
                })),
        );

    let body = match (&state.data, &state.error) {
        (_, Some(e)) => div().p_3().text_color(theme.danger).child(e.clone()).into_any_element(),
        (None, _) => div().p_3().text_color(muted).child("measuring the context…").into_any_element(),
        (Some(data), None) => {
            // -- where the tokens go --
            let mut chips = h_flex().gap_1().flex_wrap().items_center().child(
                div().text_color(muted).text_size(theme.font_size * 0.85).child(format!("~{} tokens", data.total_tokens)),
            );
            for (origin, tokens) in data.by_origin() {
                let active = state.filter == Some(origin);
                let weak = weak.clone();
                chips = chips.child(
                    h_flex()
                        .id(SharedString::from(format!("inspect-chip-{}", origin.label())))
                        .gap_1()
                        .items_center()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .cursor_pointer()
                        .border_1()
                        .border_color(if active { colour(origin, &theme) } else { theme.border })
                        .hover(|s| s.bg(theme.list_hover))
                        .on_click(move |_, _w, cx| {
                            weak.update(cx, |app, cx| app.inspect_filter(origin, cx)).ok();
                        })
                        .child(badge(origin, &theme))
                        .child(div().text_color(muted).text_size(theme.font_size * 0.85).child(format!("{tokens}"))),
                );
            }

            // -- the pieces --
            let total = data.total_tokens.max(1);
            let mut list = v_flex().id("inspect-list").w(px(330.)).flex_none().overflow_y_scroll().gap_0p5().pr_1();
            let mut last_section: Option<Section> = None;
            for (index, seg) in data.segments.iter().enumerate() {
                if state.filter.is_some_and(|f| f != seg.origin) {
                    continue;
                }
                if last_section != Some(seg.section) {
                    last_section = Some(seg.section);
                    list = list.child(div().pt_2().px_1().text_color(muted).text_size(theme.font_size * 0.8).child(seg.section.label()));
                }
                let selected = index == state.selected;
                let weak = weak.clone();
                list = list.child(
                    v_flex()
                        .id(("inspect-row", index))
                        .px_2()
                        .py_1()
                        .gap_0p5()
                        .rounded_md()
                        .cursor_pointer()
                        .when(selected, |r| r.bg(theme.list_active))
                        .hover(|r| r.bg(theme.list_hover))
                        .on_click(move |_, _w, cx| {
                            weak.update(cx, |app, cx| app.inspect_select(index, cx)).ok();
                        })
                        .child(
                            h_flex()
                                .justify_between()
                                .gap_2()
                                .child(div().flex_1().overflow_hidden().text_ellipsis().text_size(theme.font_size * 0.9).child(seg.label.clone()))
                                .child(div().text_color(muted).text_size(theme.font_size * 0.8).child(format!("{}", seg.tokens))),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(badge(seg.origin, &theme))
                                .child(div().flex_1().h(px(3.)).rounded_full().bg(theme.secondary).child(
                                    div().h_full().rounded_full().bg(colour(seg.origin, &theme)).w(relative((seg.tokens as f32 / total as f32).clamp(0.0, 1.0))),
                                )),
                        ),
                );
            }

            // -- the selected piece --
            let detail = match data.segments.get(state.selected) {
                Some(seg) => {
                    let mut text = seg.text.clone();
                    if text.len() > SHOW_CHARS {
                        let mut end = SHOW_CHARS;
                        while !text.is_char_boundary(end) {
                            end -= 1;
                        }
                        text.truncate(end);
                        text.push_str("\n… (the rest is in Copy all)");
                    }
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(badge(seg.origin, &theme))
                                .child(div().child(seg.label.clone()))
                                .child(div().text_color(muted).text_size(theme.font_size * 0.85).child(format!("~{} tokens", seg.tokens)))
                                .child(crate::hints::info_button("inspect-source", seg.origin.source())),
                        )
                        .child(
                            div()
                                .id("inspect-text")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .p_2()
                                .rounded_md()
                                .bg(theme.secondary)
                                .font_family(theme.mono_font_family.clone())
                                .text_size(theme.mono_font_size)
                                .child(text),
                        )
                        .into_any_element()
                }
                None => div().flex_1().into_any_element(),
            };

            v_flex()
                .gap_2()
                .flex_1()
                .min_h_0()
                .child(chips)
                .child(h_flex().gap_3().flex_1().min_h_0().child(list).child(detail))
                .into_any_element()
        }
    };

    v_flex().key_context("ReactorInspect").h(height).gap_3().child(header).child(body).into_any_element()
}
