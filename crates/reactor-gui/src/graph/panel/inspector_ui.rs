//! The graph panel's inspector, docked at its left, and the popup that reads and edits a long
//! text. One structure for every kind of element ([`super::inspector`]): the same header (what
//! it is, its title, how sure, who wrote it), the same tabs, the same actions below -- so a
//! note, a hypothesis, an observation and an entity read alike.
//!
//! The text of an annotation is Markdown, shown rendered; short ones are one line and need no
//! more. The agent is asked to use Markdown only for a long text, and a person may write
//! either. The components are gpui-kit's: `TabBar`, `Tag`, `TextView`, `Textarea`,
//! `Clipboard`, `DescriptionList` and its dialog.

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::description_list::DescriptionList;
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::label::Label;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, Theme};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Context, Entity, Hsla, SharedString, Window, div, px};

use super::{GraphPanel, Selected};
use crate::graph::inspector::{self, Group, Inspected, Stand, Tone};
use crate::hints;
use reactor_db::graph::edit;

/// The inspector's width to start with; it is dragged.
pub(super) const DOCK_WIDTH: f32 = 420.;

/// The reader and editor popup's state.
pub(super) struct Reader {
    /// Editing, or reading.
    pub edit: bool,
    pub input: Entity<TextareaState>,
    pub error: Option<String>,
}

/// The tabs an element has: its text (when it has a long one), what it is related to, what
/// happened to it, and the rest of what is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    Text,
    Relations,
    History,
    Details,
}

fn panes(i: &Inspected) -> Vec<Pane> {
    let mut panes = Vec::new();
    if i.text.is_some() {
        panes.push(Pane::Text);
    }
    panes.extend([Pane::Relations, Pane::History, Pane::Details]);
    panes
}

fn tone_colour(tone: Tone, theme: &Theme) -> Hsla {
    match tone {
        Tone::Entity => theme.chart_1,
        Tone::Event => theme.chart_2,
        Tone::Observation => theme.chart_3,
        Tone::Hypothesis => theme.chart_4,
        Tone::Judgement => theme.primary,
        Tone::Note => theme.info,
        Tone::Other => theme.muted_foreground,
    }
}

fn kind_tag(label: &str, colour: Hsla) -> Tag {
    Tag::custom(colour.opacity(0.15), colour, colour.opacity(0.5))
        .small()
        .child(SharedString::from(label.to_string()))
}

/// `2026-10-05T12:04:11Z` → `2026-10-05 12:04`.
fn short_time(rfc3339: &str) -> String {
    rfc3339.replace('T', " ").chars().take(16).collect()
}

impl GraphPanel {
    /// What a value in the Details tab wraps in: the width the inspector was drawn at, less
    /// the label column and the padding. Pixels, not "the rest": text is measured before the
    /// row is laid out, and with no width it reports one line's height.
    fn dock_value(&self) -> f32 {
        (self.dock_width.get() - 112. - 56.).max(120.)
    }

    /// What the inspector's header says: what the element is, its title, where a hypothesis
    /// stands, how sure it is, who wrote it.
    pub(super) fn inspector_head(
        &self,
        i: &Inspected,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colour = tone_colour(i.tone, theme);
        let muted = theme.muted_foreground;
        let mut head = v_flex()
            .p_3()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(kind_tag(&i.label, colour))
                            .when(i.status != "active", |row| {
                                row.child(
                                    Tag::warning()
                                        .small()
                                        .child(SharedString::from(i.status.clone())),
                                )
                            }),
                    )
                    .child(
                        Button::new("graph-deselect")
                            .small()
                            .ghost()
                            .icon(gpui_kit::assets::IconName::X)
                            .tooltip("Close")
                            .on_click(cx.listener(|this, _, _, cx| this.select(None, cx))),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .text_size(theme.font_size * 1.2)
                    .child(SharedString::from(i.title.clone())),
            );
        if let Some(status) = &i.hypothesis {
            let at = edit::HYPOTHESIS_STATUSES
                .iter()
                .position(|s| s == status)
                .unwrap_or(0);
            head = head.child(
                TabBar::new("graph-hypothesis-status")
                    .segmented()
                    .small()
                    .selected_index(at)
                    .children(
                        edit::HYPOTHESIS_STATUSES
                            .iter()
                            .map(|s| Tab::new().label(capitalised(s))),
                    )
                    .on_click(cx.listener(|this, ix: &usize, _w, cx| {
                        if let Some(status) = edit::HYPOTHESIS_STATUSES.get(*ix) {
                            this.set_hypothesis(status, cx);
                        }
                    })),
            );
        }
        // How sure, as a scale of bars, weakest first: one bar per step, filled up to the
        // one it is at. A click on a bar sets that step; on the one it is at, clears it.
        let steps = self.scale.len();
        let at = i.level.map(|(at, _)| at);
        let bars = h_flex()
            .gap_1()
            .items_center()
            .children(self.scale.iter().enumerate().map(|(ix, name)| {
                let (value, hint) = (name.clone(), name.clone());
                let filled = at.is_some_and(|a| ix <= a);
                hints::tip(
                    div()
                        .id(("graph-confidence-step", ix))
                        .h(px(16.))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if at == Some(ix) {
                                this.clear_confidence(cx);
                            } else {
                                this.set_confidence(&value, cx);
                            }
                        }))
                        .child(div().w(px(20.)).h(px(6.)).rounded_sm().bg(if filled {
                            theme.accent
                        } else {
                            theme.secondary
                        })),
                    hint,
                )
            }));
        head = head.child(
            h_flex()
                .gap_3()
                .items_center()
                .when(steps > 0, |row| row.child(bars))
                .child(
                    div()
                        .text_size(theme.font_size * 0.85)
                        .text_color(muted)
                        .child(SharedString::from(
                            i.confidence
                                .clone()
                                .unwrap_or_else(|| "no confidence given".to_string()),
                        )),
                ),
        );
        head.child(
            div()
                .w_full()
                .text_size(theme.font_size * 0.8)
                .text_color(muted)
                .child(SharedString::from(format!(
                    "{} · version {} · {}",
                    inspector::who(&i.author),
                    i.ver,
                    short_time(&i.recorded_at)
                ))),
        )
        .into_any_element()
    }

    /// The tab bar and the open tab.
    pub(super) fn inspector_body(
        &self,
        i: &Inspected,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let panes = panes(i);
        let at = self.inspector_tab.min(panes.len() - 1);
        let labels: Vec<String> = panes
            .iter()
            .map(|p| match p {
                Pane::Text => "Text".to_string(),
                Pane::Relations if i.hypothesis.is_some() => format!("Evidence {}", i.related()),
                Pane::Relations => format!("Relations {}", i.related()),
                Pane::History => format!("History {}", i.history.len()),
                Pane::Details => "Details".to_string(),
            })
            .collect();
        let tabs = TabBar::new("graph-inspector-tabs")
            .selected_index(at)
            .children(labels.into_iter().map(|l| Tab::new().label(l)))
            .on_click(cx.listener(|this, ix: &usize, _w, cx| {
                this.inspector_tab = *ix;
                cx.notify();
            }));
        let content = match panes[at] {
            Pane::Text => self.text_pane(i, theme, cx),
            Pane::Relations => self.relations_pane(i, theme, cx),
            Pane::History => self.history_pane(i, theme),
            Pane::Details => self.details_pane(i, theme),
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .child(div().flex_none().px_3().child(tabs))
            .child(
                div()
                    .id("graph-inspector-pane")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p_3()
                    .child(content),
            )
            .into_any_element()
    }

    /// The text, rendered as Markdown, with the buttons to copy it, read it large and change it.
    fn text_pane(&self, i: &Inspected, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let text = i.text.clone().unwrap_or_default();
        let editable = i.editable;
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_1()
                    .justify_end()
                    .child(
                        Clipboard::new("graph-copy-text")
                            .value(SharedString::from(text.clone()))
                            .tooltip("Copy the text as written (Markdown)"),
                    )
                    .child(
                        Button::new("graph-read-text")
                            .small()
                            .ghost()
                            .label("Read")
                            .tooltip("Open it large, with an outline")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_reader(false, window, cx)
                            })),
                    )
                    .when(editable, |row| {
                        row.child(
                            Button::new("graph-edit-text")
                                .small()
                                .label("Edit")
                                .tooltip("Rewrite it: a new version, the old one is kept")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_reader(true, window, cx)
                                })),
                        )
                    }),
            )
            .child(
                div().w_full().child(
                    TextView::markdown(SharedString::from(format!("graph-text-{}", i.id)), text)
                        .style(crate::theme::text_view_style(theme))
                        .on_link_click(|url, _event, _window, cx| cx.open_url(url)),
                ),
            )
            .child(self.tag_chips(i))
            .into_any_element()
    }

    fn tag_chips(&self, i: &Inspected) -> AnyElement {
        h_flex()
            .gap_1()
            .flex_wrap()
            .children(i.tags.iter().map(|t| {
                Tag::secondary()
                    .small()
                    .outline()
                    .child(SharedString::from(t.clone()))
            }))
            .into_any_element()
    }

    /// What the element is related to, grouped by what it says: supports and refutes first.
    fn relations_pane(&self, i: &Inspected, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if i.groups.is_empty() {
            return div()
                .text_color(theme.muted_foreground)
                .child("Nothing is related to it yet. Relate to… states a relation.")
                .into_any_element();
        }
        let mut column = v_flex().gap_3();
        for (gi, g) in i.groups.iter().enumerate() {
            column = column.child(self.group(gi, g, theme, cx));
        }
        column.into_any_element()
    }

    fn group(&self, gi: usize, g: &Group, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let heading = match g.stand {
            Stand::Supports => Tag::success().small(),
            Stand::Refutes => Tag::danger().small(),
            Stand::Neutral => Tag::secondary().small().outline(),
        }
        .child(SharedString::from(format!(
            "{} · {}",
            g.title,
            g.items.len()
        )));
        let mut rows = v_flex().gap_0p5();
        for (ri, r) in g.items.iter().enumerate() {
            let id = r.id.clone();
            let drawn = self.model.nodes.iter().any(|n| n.id == id);
            rows = rows.child(
                h_flex()
                    .id(SharedString::from(format!("graph-related-{gi}-{ri}")))
                    .gap_2()
                    .items_center()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .when(drawn, |row| {
                        row.cursor_pointer()
                            .hover(|s| s.bg(theme.list_hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select(Some(Selected::Nodes(vec![id.clone()])), cx)
                            }))
                    })
                    .when(!drawn, |row| row.opacity(0.7))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(SharedString::from(r.name.clone())),
                    )
                    .children(r.locator.clone().map(|l| {
                        div()
                            .flex_none()
                            .text_size(theme.font_size * 0.8)
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(l))
                    })),
            );
        }
        v_flex()
            .gap_1()
            .child(heading)
            .child(rows)
            .into_any_element()
    }

    /// Every version, newest first, and what each changed.
    fn history_pane(&self, i: &Inspected, theme: &Theme) -> AnyElement {
        let mut column = v_flex().gap_2();
        for v in i.history.iter().rev() {
            column = column.child(
                v_flex()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Label::new(format!("Version {}", v.ver)))
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .text_size(theme.font_size * 0.85)
                                    .child(SharedString::from(short_time(&v.when))),
                            ),
                    )
                    .child(div().w_full().child(SharedString::from(v.change.clone())))
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .text_size(theme.font_size * 0.8)
                            .child(SharedString::from(v.who.clone())),
                    ),
            );
        }
        column.into_any_element()
    }

    /// The rest: when, what holds, its properties, and its id.
    fn details_pane(&self, i: &Inspected, theme: &Theme) -> AnyElement {
        let mut list = DescriptionList::new()
            .columns(1)
            .bordered(false)
            .label_width(px(112.))
            .small();
        let mut rows: Vec<(String, String)> = vec![("Type".into(), i.type_.clone())];
        rows.extend(i.facts.iter().cloned());
        rows.push(("Recorded".into(), short_time(&i.recorded_at)));
        rows.push(("Id".into(), i.id.clone()));
        for (label, value) in rows {
            list = list.item(
                SharedString::from(label),
                div()
                    .w(px(self.dock_value()))
                    .flex_none()
                    .child(SharedString::from(value))
                    .into_any_element(),
                1,
            );
        }
        v_flex()
            .gap_3()
            .child(list)
            .child(self.tag_chips(i))
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .text_size(theme.font_size * 0.8)
                    .child("Tags are added and taken off below."),
            )
            .into_any_element()
    }

    /// Several elements at once: what they are, not each one in full.
    pub(super) fn multi_body(&self, theme: &Theme) -> AnyElement {
        let mut list = DescriptionList::new()
            .columns(1)
            .bordered(false)
            .label_width(px(112.))
            .small();
        for (n, element) in self.detail.iter().enumerate() {
            if n > 0 {
                list = list.separator();
            }
            for (label, value) in element {
                list = list.item(
                    SharedString::from(label.clone()),
                    div()
                        .w(px(self.dock_value()))
                        .flex_none()
                        .child(SharedString::from(value.clone()))
                        .into_any_element(),
                    1,
                );
            }
        }
        div()
            .id("graph-inspector-multi")
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .p_3()
            .text_size(theme.font_size * 0.9)
            .child(list)
            .into_any_element()
    }

    // -- the reader and editor popup ---------------------------------------------------------

    /// Open the text large: rendered, or to change. The same data as the inspector, with room.
    pub(super) fn open_reader(&mut self, edit: bool, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        let Some(i) = self.inspected.clone() else {
            return;
        };
        let text = i.text.clone().unwrap_or_else(|| i.title.clone());
        let input = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .placeholder("Markdown: headings, lists and code, for a long text.");
            s.set_value(&text, window, cx);
            s
        });
        self.reader = Some(Reader {
            edit,
            input,
            error: None,
        });
        let weak = cx.weak_entity();
        let title = format!("{} · {}", i.label, i.title);
        window.open_dialog(cx, move |dialog, window, _cx| {
            let (w_content, w_footer, w_close) = (weak.clone(), weak.clone(), weak.clone());
            let height = window.viewport_size().height * 0.58;
            dialog
                .w(px(980.))
                .title(title.clone())
                .on_close(move |_, _w, cx| {
                    let _ = w_close.update(cx, |panel, _| panel.reader = None);
                })
                .content(move |content, _w, cx| content.child(reader_view(&w_content, height, cx)))
                .footer(reader_footer(w_footer))
        });
        cx.notify();
    }

    /// Write the text as a new version.
    pub(super) fn save_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        let (Some(i), Some(reader)) = (self.inspected.clone(), self.reader.as_ref()) else {
            return;
        };
        let text = reader.input.read(cx).value().to_string();
        let id = i.id.clone();
        self.error = None;
        let done = self.edit(cx, |db, at| edit::set_text(db, at, &id, &text));
        match (done, self.error.clone()) {
            (Some(_), _) => {
                self.reader = None;
                window.close_dialog(cx);
            }
            (None, error) => {
                if let Some(r) = &mut self.reader {
                    r.error = error;
                }
                // Said here, in the popup: not also over the graph.
                self.error = None;
                cx.notify();
            }
        }
    }
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The popup's contents: the text on the left, an outline and the evidence at its right.
fn reader_view(
    weak: &gpui_kit::WeakEntity<GraphPanel>,
    height: gpui_kit::Pixels,
    cx: &mut App,
) -> AnyElement {
    let Some(panel) = weak.upgrade() else {
        return div().into_any_element();
    };
    let panel = panel.read(cx);
    let (Some(i), Some(reader)) = (panel.inspected.as_ref(), panel.reader.as_ref()) else {
        return div().into_any_element();
    };
    let theme = cx.theme().clone();
    let muted = theme.muted_foreground;
    let text = i.text.clone().unwrap_or_else(|| i.title.clone());

    let main = if reader.edit {
        v_flex()
            .flex_1()
            .min_w_0()
            .gap_2()
            .child(Textarea::new(&reader.input).h(height))
            .children(
                reader
                    .error
                    .clone()
                    .map(|e| div().text_color(theme.danger).child(SharedString::from(e))),
            )
            .into_any_element()
    } else {
        div()
            .id("graph-reader-text")
            .flex_1()
            .min_w_0()
            .h(height)
            .overflow_y_scrollbar()
            .pr_4()
            .child(
                div().max_w(px(640.)).child(
                    TextView::markdown(
                        SharedString::from(format!("graph-reader-{}", i.id)),
                        text.clone(),
                    )
                    .style(crate::theme::text_view_style(&theme))
                    .on_link_click(|url, _event, _window, cx| cx.open_url(url)),
                ),
            )
            .into_any_element()
    };

    let heading = |t: &'static str| {
        div()
            .text_color(muted)
            .text_size(theme.font_size * 0.75)
            .child(t.to_uppercase())
    };
    let mut rail = v_flex()
        .w(px(260.))
        .flex_none()
        .gap_4()
        .pl_4()
        .border_l_1()
        .border_color(theme.border);
    let outline = inspector::headings(&text);
    if !outline.is_empty() {
        let mut o = v_flex().gap_1().child(heading("On this page"));
        for h in outline {
            o = o.child(
                div()
                    .text_size(theme.font_size * 0.9)
                    .child(SharedString::from(h)),
            );
        }
        rail = rail.child(o);
    }
    for g in &i.groups {
        let mut o = v_flex().gap_1().child(heading_owned(&g.title, &theme));
        for r in &g.items {
            o = o.child(
                div()
                    .text_size(theme.font_size * 0.9)
                    .child(SharedString::from(r.name.clone())),
            );
        }
        rail = rail.child(o);
    }
    rail = rail.child(
        v_flex()
            .gap_1()
            .child(heading("Details"))
            .child(
                div()
                    .text_color(muted)
                    .text_size(theme.font_size * 0.85)
                    .child(SharedString::from(format!(
                        "{} · version {} · {}",
                        inspector::who(&i.author),
                        i.ver,
                        short_time(&i.recorded_at)
                    ))),
            )
            .children(i.confidence.clone().map(|c| {
                div()
                    .text_size(theme.font_size * 0.85)
                    .child(SharedString::from(format!("Confidence: {c}")))
            })),
    );
    h_flex()
        .gap_4()
        .items_start()
        .child(main)
        .child(rail)
        .into_any_element()
}

fn heading_owned(text: &str, theme: &Theme) -> impl IntoElement {
    div()
        .text_color(theme.muted_foreground)
        .text_size(theme.font_size * 0.75)
        .child(SharedString::from(text.to_uppercase()))
}

/// The buttons under the popup: switch between reading and editing, copy, save, close.
fn reader_footer(weak: gpui_kit::WeakEntity<GraphPanel>) -> impl IntoElement {
    let (w_mode, w_copy, w_save) = (weak.clone(), weak.clone(), weak);
    h_flex()
        .w_full()
        .gap_2()
        .child(
            Button::new("graph-reader-mode")
                .label("Edit / Read")
                .tooltip("Switch between the rendered text and the Markdown")
                .on_click(move |_, _w, cx| {
                    let _ = w_mode.update(cx, |panel, cx| {
                        if let Some(r) = &mut panel.reader {
                            r.edit = !r.edit;
                        }
                        cx.notify();
                    });
                }),
        )
        .child(
            Button::new("graph-reader-copy")
                .label("Copy as Markdown")
                .on_click(move |_, _w, cx| {
                    let _ = w_copy.update(cx, |panel, cx| {
                        let text = match (&panel.reader, &panel.inspected) {
                            (Some(r), Some(_)) if r.edit => r.input.read(cx).value().to_string(),
                            (_, Some(i)) => i.text.clone().unwrap_or_else(|| i.title.clone()),
                            _ => String::new(),
                        };
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                    });
                }),
        )
        .child(div().flex_1())
        .child(
            Button::new("graph-reader-save")
                .label("Save")
                .primary()
                .tooltip("A new version; the old text is kept in its history")
                .on_click(move |_, window, cx| {
                    let _ = w_save.update(cx, |panel, cx| panel.save_text(window, cx));
                }),
        )
        .child(
            Button::new("graph-reader-close")
                .label("Close")
                .on_click(|_, window, cx| {
                    gpui_kit::component::WindowExt::close_dialog(window, cx);
                }),
        )
}
