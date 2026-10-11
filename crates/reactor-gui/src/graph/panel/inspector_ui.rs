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
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, Theme};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Context, Entity, SharedString, Window, div, px};

use super::node_ui::icon;
use super::{GraphPanel, GraphSetStatus, Selected};
use crate::graph::inspector::{self, Group, Inspected, Stand};
use crate::graph::look;
use crate::hints;
use reactor_db::graph::Kind;
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

/// `2026-10-05T12:04:11Z` → `2026-10-05 12:04`.
fn short_time(rfc3339: &str) -> String {
    rfc3339.replace('T', " ").chars().take(16).collect()
}

impl GraphPanel {
    /// What a value in the Details tab wraps in: the width the inspector was drawn at, less
    /// the label column and the padding. Pixels, not "the rest": text is measured before the
    /// row is laid out, and with no width it reports one line's height.
    fn dock_value(&self) -> f32 {
        (self.dock_width.get() - 88. - 12. - 40.).max(120.)
    }

    /// What an element is the kind of: the graph's word for it, else its type's.
    fn kind_of_id(&self, id: &str, type_: &str) -> Kind {
        self.model
            .nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.element)
            .unwrap_or_else(|| self.kind_of_type(type_))
    }

    /// The element's standing as one menu: active, disputed or soft-deleted. What is chosen
    /// applies to everything selected.
    pub(super) fn status_menu(&self, status: &str, theme: &Theme) -> impl IntoElement {
        let focus = self.focus.clone();
        let current = status.to_string();
        let (label, disputed) = match status {
            "conflicting" => ("Disputed", true),
            "soft-deleted" => ("Deleted", false),
            _ => ("Active", false),
        };
        let button = Button::new("graph-status").small().label(label);
        let button = if disputed {
            button.warning()
        } else {
            button.outline()
        };
        let _ = theme;
        button
            .tooltip("Its standing: dispute it (it stays visible, flagged — say why in a note) or hide it (nothing is lost)")
            .dropdown_menu(move |menu, _, _| {
                let pick = |status: &str| {
                    Box::new(GraphSetStatus {
                        status: status.to_string(),
                    })
                };
                menu.action_context(focus.clone())
                    .menu_with_check("Active", current == "active", pick("active"))
                    .menu_with_check(
                        "Disputed — stays visible, flagged",
                        current == "conflicting",
                        pick("conflicting"),
                    )
                    .separator()
                    .menu_with_check(
                        "Soft-delete — hidden, can come back",
                        current == "soft-deleted",
                        pick("soft-deleted"),
                    )
            })
    }

    /// What the inspector's header says: the node as it is drawn on the plane (its type's icon
    /// and colour), what it is, who wrote it and where it came from, its standing, and -- for
    /// what is concluded rather than found -- where a hypothesis stands and how sure it is.
    pub(super) fn inspector_head(
        &self,
        i: &Inspected,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme.is_dark();
        let kind = self.kind_of_id(&i.id, &i.type_);
        let look = look::look(&self.vocabulary, &i.type_, kind);
        let hue = look::ink(look.colour, dark);
        let muted = theme.muted_foreground;
        let small = theme.font_size * 0.85;
        // What is concluded has no source to rest on: how sure it is, is what it has.
        let concluded = kind == Kind::Annotation;
        let mut head = v_flex()
            .p_3()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(
                        div()
                            .flex_none()
                            .size(px(38.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_lg()
                            .bg(hue.opacity(0.14))
                            .border_1()
                            .border_color(hue.opacity(0.35))
                            .child(icon(look.path.clone(), 20., hue)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_1()
                                    .items_start()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(theme.font_size * 1.2)
                                            .child(SharedString::from(i.title.clone())),
                                    )
                                    .child(
                                        Clipboard::new("graph-copy-title")
                                            .value(SharedString::from(i.title.clone()))
                                            .tooltip("Copy its name"),
                                    ),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .text_size(small)
                                    .text_color(muted)
                                    .font_family(theme.mono_font_family.clone())
                                    .child(SharedString::from(format!(
                                        "{} · {}",
                                        i.type_,
                                        kind.as_str()
                                    ))),
                            ),
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
                h_flex()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .min_w_0()
                            .text_size(small)
                            .text_color(muted)
                            .child(SharedString::from(format!(
                                "{} · version {} · {}",
                                inspector::who(&i.author),
                                i.ver,
                                short_time(&i.recorded_at)
                            ))),
                    )
                    .child(
                        Button::new("graph-source")
                            .small()
                            .ghost()
                            .icon(gpui_kit::assets::IconName::ExternalLink)
                            .label("Source")
                            .tooltip("Open where this came from: the file, the page, or the session it was said in")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_source(window, cx)
                            })),
                    ),
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
                .justify_between()
                .child(self.status_menu(&i.status, theme))
                .when(concluded, |row| {
                    row.child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .when(steps > 0, |r| r.child(bars))
                            .child(
                                div()
                                    .text_size(small)
                                    .text_color(if i.confidence.is_some() {
                                        theme.foreground
                                    } else {
                                        theme.primary
                                    })
                                    .child(SharedString::from(
                                        i.confidence
                                            .clone()
                                            .unwrap_or_else(|| "How sure?".to_string()),
                                    )),
                            ),
                    )
                })
                .when(!concluded, |row| {
                    row.child(
                        div()
                            .text_size(small)
                            .text_color(muted)
                            .child("a source, not a judgement"),
                    )
                }),
        );
        head.into_any_element()
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
        let labels: Vec<(&str, Option<usize>)> = panes
            .iter()
            .map(|p| match p {
                Pane::Text => ("Text", None),
                Pane::Relations if i.hypothesis.is_some() => ("Evidence", Some(i.related())),
                Pane::Relations => ("Relations", Some(i.related())),
                Pane::History => ("History", Some(i.history.len())),
                Pane::Details => ("Details", None),
            })
            .collect();
        // The same pill control as the kinds in the add form and the layers' modes: the open
        // tab is the lighter pill, and what a tab holds is counted beside its name.
        let tabs = h_flex()
            .gap_0p5()
            .p_0p5()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .children(labels.into_iter().enumerate().map(|(ix, (name, count))| {
                let on = ix == at;
                h_flex()
                    .id(("graph-inspector-tab", ix))
                    .flex_1()
                    .justify_center()
                    .items_center()
                    .gap_1()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(on, |d| d.bg(theme.secondary).text_color(theme.foreground))
                    .when(!on, |d| {
                        d.text_color(theme.muted_foreground)
                            .hover(|d| d.text_color(theme.foreground))
                    })
                    .child(name)
                    .children(count.map(|n| {
                        div()
                            .text_size(theme.font_size * 0.85)
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(n.to_string()))
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.inspector_tab = ix;
                        cx.notify();
                    }))
            }));
        let content = match panes[at] {
            Pane::Text => self.text_pane(i, theme, cx),
            Pane::Relations => self.relations_pane(i, theme, cx),
            Pane::History => self.history_pane(i, theme, cx),
            Pane::Details => self.details_pane(i, theme),
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .child(div().flex_none().px_3().pt_2().pb_2().child(tabs))
            .child(
                div()
                    .id("graph-inspector-pane")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .border_t_1()
                    .border_color(theme.border)
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
        let dark = theme.is_dark();
        let small = theme.font_size * 0.85;
        let dot = match g.stand {
            Stand::Supports => theme.success,
            Stand::Refutes => theme.danger,
            Stand::Neutral => theme.muted_foreground,
        };
        let heading = h_flex()
            .gap_2()
            .items_center()
            .text_size(small)
            .text_color(theme.muted_foreground)
            .child(div().size(px(8.)).rounded_full().bg(dot))
            .child(SharedString::from(g.title.to_uppercase()))
            .child(SharedString::from(g.items.len().to_string()));
        // Each element at the other end as the plane draws it: its type's icon and colour,
        // what it says, and its type under that.
        let mut rows = v_flex().gap_2();
        for (ri, r) in g.items.iter().enumerate() {
            let id = r.id.clone();
            let node = self.model.nodes.iter().find(|n| n.id == id);
            let drawn = node.is_some();
            let (path, hue, type_) = match node {
                Some(n) => {
                    let look = look::look(&self.vocabulary, &n.type_, n.element);
                    (
                        look.path.clone(),
                        look::ink(look.colour, dark),
                        n.type_.clone(),
                    )
                }
                None => {
                    let look = look::look(&self.vocabulary, "", Kind::Entity);
                    (
                        look.path.clone(),
                        look::ink(look.colour, dark),
                        String::new(),
                    )
                }
            };
            rows = rows.child(
                h_flex()
                    .id(SharedString::from(format!("graph-related-{gi}-{ri}")))
                    .gap_2()
                    .items_center()
                    .px_2()
                    .py_1p5()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .when(drawn, |row| {
                        row.cursor_pointer()
                            .hover(|s| s.border_color(theme.muted_foreground))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select(Some(Selected::Nodes(vec![id.clone()])), cx)
                            }))
                    })
                    .when(!drawn, |row| row.opacity(0.7))
                    .child(icon(path, f32::from(theme.font_size) * 1.15, hue))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(SharedString::from(r.name.clone()))
                            .when(!type_.is_empty(), |c| {
                                c.child(
                                    div()
                                        .text_size(small)
                                        .text_color(theme.muted_foreground)
                                        .child(SharedString::from(type_)),
                                )
                            }),
                    )
                    .children(r.locator.clone().map(|l| {
                        div()
                            .flex_none()
                            .text_size(small)
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(l))
                    })),
            );
        }
        v_flex()
            .gap_2()
            .child(heading)
            .child(rows)
            .into_any_element()
    }

    /// Every version, newest first, and what each changed; and taking the latest back.
    fn history_pane(&self, i: &Inspected, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let small = theme.font_size * 0.85;
        let mut column = v_flex().gap_3().child(
            h_flex()
                .id("graph-undo")
                .gap_2()
                .items_center()
                .px_2()
                .py_2()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .cursor_pointer()
                .hover(|s| s.border_color(theme.muted_foreground))
                .child(icon(
                    look::icon_path("undo-2"),
                    f32::from(small) * 1.3,
                    theme.foreground,
                ))
                .child(
                    v_flex().child("Undo last change").child(
                        div()
                            .text_size(small)
                            .text_color(theme.muted_foreground)
                            .child("Takes back the latest version. Shows what would go first."),
                    ),
                )
                .on_click(cx.listener(|this, _, _, cx| this.preview_undo(cx))),
        );
        for v in i.history.iter().rev() {
            column = column.child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(
                        div()
                            .flex_none()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(theme.secondary)
                            .text_size(small)
                            .child(SharedString::from(format!("v{}", v.ver))),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().w_full().child(SharedString::from(v.change.clone())))
                            .child(
                                div()
                                    .text_size(small)
                                    .text_color(theme.muted_foreground)
                                    .child(SharedString::from(format!(
                                        "{} · {}",
                                        v.who,
                                        short_time(&v.when)
                                    ))),
                            ),
                    ),
            );
        }
        column.into_any_element()
    }

    /// The rest: when, what holds, its properties, and its id -- a label and its value a line
    /// each, every value with a button that copies it, and one that copies them all.
    fn details_pane(&self, i: &Inspected, theme: &Theme) -> AnyElement {
        let small = theme.font_size * 0.85;
        let mut rows: Vec<(String, String, bool)> = vec![("Type".into(), i.type_.clone(), true)];
        rows.extend(i.facts.iter().map(|(l, v)| (l.clone(), v.clone(), false)));
        rows.push(("Recorded".into(), short_time(&i.recorded_at), false));
        rows.push(("Id".into(), i.id.clone(), true));
        let all: String = rows
            .iter()
            .map(|(l, v, _)| format!("{l}: {v}"))
            .collect::<Vec<_>>()
            .join("\n");
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .justify_end()
                    .items_center()
                    .gap_1()
                    .text_size(small)
                    .text_color(theme.muted_foreground)
                    .child("Copy all")
                    .child(
                        Clipboard::new("graph-copy-details")
                            .value(SharedString::from(all))
                            .tooltip("Copy every line below, as label: value"),
                    ),
            )
            .children(
                rows.into_iter()
                    .enumerate()
                    .map(|(ix, (label, value, mono))| {
                        h_flex()
                            .gap_3()
                            .items_start()
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(88.))
                                    .text_size(small)
                                    .text_color(theme.muted_foreground)
                                    .child(SharedString::from(label)),
                            )
                            .child(
                                div()
                                    .w(px((self.dock_value() - 32.).max(100.)))
                                    .flex_none()
                                    .when(mono, |d| {
                                        d.font_family(theme.mono_font_family.clone())
                                            .text_size(small)
                                    })
                                    .child(SharedString::from(value.clone())),
                            )
                            .child(
                                Clipboard::new(("graph-copy-detail", ix))
                                    .value(SharedString::from(value))
                                    .tooltip("Copy this value"),
                            )
                    }),
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
