//! What sits over the plane: the toolbar, the filter layers, find, the legend, and the card a
//! type's icon and colour are chosen on.
//!
//! **Filter layers** (at the bottom right) stack: each is a filter and what it does to the nodes it
//! matches -- show only those, hide them, dim them, highlight them -- and each is switched
//! off on its own ([`crate::graph::sift`]). **Find** (Ctrl/Cmd+F) hides nothing: it rings what
//! matches for as long as it is open, marks the words, and steps from one match to the next.
//! The **legend** is behind the (i) and lists only what the picture on screen uses.

use std::collections::BTreeMap;

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Div, MouseButton, SharedString, Stateful, Window, div, px,
};
use reactor_db::graph::views::{self, SavedView, ViewLayer};
use reactor_db::graph::vocab::{self, Vocabulary};

use super::node_ui::{icon, meter};
use super::{GraphPanel, GraphSetDetail, GraphSetLayout, LayerRow};
use crate::graph::arrange::LayoutKind;
use crate::graph::card::Size;
use crate::graph::look;
use crate::graph::model::{self, LineKind};
use crate::graph::sift::{self, Found, Mode, Sifted};
use crate::hints;

/// How the filter is written, for the (i) beside the layers.
const FILTER_HELP: &str = "Each layer is a filter and what it does with the nodes it matches: show only those, hide them, dim them or highlight them. Layers apply top to bottom and each is switched off with its eye. In a filter, words must all hold. A bare word is looked for in a node's label, type and text. type:net. (a type that starts so, or has a dotted part that does: type:ipv4), tag:phase. (a tag that starts so), by:agent or by:person, kind:entity, kind:claim or kind:note, status:disputed or status:deleted, conf:>=likely (also <, <=, >, a number from 1, or conf:none), hyp:open. a|b is either: tag:c2|exfil. A leading - means not: -tag:benign. Lines are drawn between nodes that are shown.";

const PLANE_HELP: &str = "Click selects; Ctrl/Cmd+click adds a node to the selection or takes it out; Shift+drag sweeps an area; dragging a selected node moves them all. Right-click the plane adds a node there; right-click a node changes its type's icon and colour. Scroll zooms; drag the plane to pan. [ and ] make the selected nodes say less or more (every node, when none is selected) and 0 puts them back; drag a selected node's right edge, or Alt+drag it, to set its width. Ctrl/Cmd+F finds.";

/// A card over the plane: nothing behind it shows through, and a click on it is not a click
/// on the plane.
fn card(id: &'static str, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    div()
        .id(id)
        .absolute()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .text_size(theme.font_size * 0.85)
        .text_color(theme.muted_foreground)
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
}

impl GraphPanel {
    // -- filter layers -----------------------------------------------------------------------

    /// Add a layer under the others, with this typed in it.
    pub(super) fn add_layer(
        &mut self,
        text: &str,
        mode: Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("tag:phase.c2  type:net.  conf:<likely")
        });
        cx.subscribe_in(&input, window, |_, _, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        })
        .detach();
        input.update(cx, |s, cx| {
            if !text.is_empty() {
                s.set_value(SharedString::from(text.to_string()), window, cx);
            } else {
                s.focus(window, cx);
            }
        });
        self.layers.push(LayerRow {
            input,
            mode,
            on: true,
        });
        self.layers_open = true;
        cx.notify();
    }

    // -- saved views -------------------------------------------------------------------------

    /// How the graph is set up now, as a view with the name left open: its layers that have
    /// something typed (a blank one is nothing to keep), the view's size and its level.
    pub(super) fn current_view(&self, cx: &App) -> SavedView {
        SavedView {
            name: String::new(),
            layers: self
                .layers
                .iter()
                .filter_map(|row| {
                    let filter = row.input.read(cx).value().trim().to_string();
                    (!filter.is_empty()).then(|| ViewLayer {
                        filter,
                        mode: row.mode.slug().to_string(),
                        on: row.on,
                    })
                })
                .collect(),
            size: Some(self.size.slug().to_string()),
            detail: self.detail_pick.map(|d| d.label().to_lowercase()),
        }
    }

    /// Whether the graph is set up as this view says.
    pub(super) fn is_view(&self, view: &SavedView, cx: &App) -> bool {
        let now = self.current_view(cx);
        now.layers == view.layers
            && now.detail == view.detail
            && (view.size.is_none() || now.size == view.size)
    }

    /// Keep the current set-up under the name typed.
    pub(super) fn save_view(&mut self, cx: &mut Context<Self>) {
        let (Some(b), name) = (
            self.backend.clone(),
            self.view_name.read(cx).value().trim().to_string(),
        ) else {
            return;
        };
        if name.is_empty() {
            return;
        }
        let view = SavedView {
            name,
            ..self.current_view(cx)
        };
        match views::save(b.case.db(), &view) {
            Ok(_) => {
                self.views = views::list(b.case.db()).unwrap_or_default();
                self.saving_view = false;
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    /// Put a saved view back: its layers replace the ones there, and its size and level are
    /// the view's. A layer that cannot be read as a filter is kept as typed.
    pub(super) fn apply_view(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.views.iter().find(|v| v.name == name).cloned() else {
            return;
        };
        self.layers.clear();
        for layer in &view.layers {
            let mode = Mode::parse(&layer.mode).unwrap_or_default();
            self.add_layer(&layer.filter, mode, window, cx);
            if let Some(row) = self.layers.last_mut() {
                row.on = layer.on;
            }
        }
        self.layers_open = true;
        if let Some(size) = view.size.as_deref().and_then(Size::parse) {
            self.size = size;
            self.make_room(&[]);
        }
        self.detail_pick = match view.detail.as_deref() {
            Some("basic") => Some(model::Detail::Basic),
            Some("analysis") => Some(model::Detail::Analysis),
            Some("full") => Some(model::Detail::Full),
            _ => None,
        };
        cx.notify();
    }

    /// Forget a view.
    pub(super) fn delete_view(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(b) = self.backend.clone() else {
            return;
        };
        match views::delete(b.case.db(), name) {
            Ok(()) => {
                self.views = views::list(b.case.db()).unwrap_or_default();
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    /// The layers as typed, read against the case's confidence scale.
    pub(super) fn layers(&self, cx: &App) -> Vec<sift::Layer> {
        self.layers
            .iter()
            .map(|row| sift::Layer {
                filter: model::parse_filter_on(row.input.read(cx).value().as_ref(), &self.scale),
                mode: row.mode,
                on: row.on,
            })
            .collect()
    }

    /// How many layers do something now.
    pub(super) fn layers_on(&self, cx: &App) -> usize {
        self.layers
            .iter()
            .filter(|row| row.on && !row.input.read(cx).value().trim().is_empty())
            .count()
    }

    /// Which nodes are drawn: what the layers let through and the level of detail draws.
    pub(super) fn shown(&self, cx: &App) -> Vec<bool> {
        let level = self
            .detail_pick
            .unwrap_or_else(|| model::Detail::for_zoom(self.zoom));
        let disclosure = model::disclose(&self.model, level);
        sift::sift(&self.model, &self.layers(cx))
            .nodes
            .iter()
            .zip(&disclosure.shown)
            .map(|(d, level)| d.shown && *level)
            .collect()
    }

    pub(super) fn layers_panel(
        &self,
        sifted: &Sifted,
        shown: &[bool],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let drawn = shown.iter().filter(|s| **s).count();
        let dimmed = sifted
            .nodes
            .iter()
            .zip(shown)
            .filter(|(d, s)| **s && d.dim)
            .count();
        let hidden = self.model.nodes.len() - drawn;
        let views_row = self.views_row(cx);
        let rows = self.layers.iter().enumerate().map(|(i, row)| {
            let matched = sifted.matched.get(i).copied().unwrap_or(0);
            let (on, mode) = (row.on, row.mode);
            h_flex()
                .w_full()
                .gap_1()
                .px_2()
                .py_1()
                .items_center()
                .border_b_1()
                .border_color(theme.border.opacity(0.6))
                .when(!on, |d| d.opacity(0.55))
                .child(hints::tip(
                    div().id(("graph-layer-eye-holder", i)).child(
                        Button::new(("graph-layer-eye", i))
                            .small()
                            .ghost()
                            .icon(if on { IconName::Eye } else { IconName::EyeOff })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(row) = this.layers.get_mut(i) {
                                    row.on = !row.on;
                                }
                                cx.notify();
                            })),
                    ),
                    if on {
                        "This layer is on — click to switch it off"
                    } else {
                        "This layer is off — click to switch it on"
                    },
                ))
                .child(hints::tip(
                    div().id(("graph-layer-mode-holder", i)).w(px(86.)).child(
                        Button::new(("graph-layer-mode", i))
                            .small()
                            .outline()
                            .w_full()
                            .label(mode.label())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(row) = this.layers.get_mut(i) {
                                    row.mode = row.mode.next();
                                }
                                cx.notify();
                            })),
                    ),
                    "What the layer does with the nodes it matches. Click for the next: show only, hide, dim, highlight",
                ))
                .child(div().flex_1().min_w_0().child(Input::new(&row.input).small()))
                .child(
                    div()
                        .w(px(26.))
                        .text_color(theme.muted_foreground)
                        .flex()
                        .justify_end()
                        .child(SharedString::from(matched.to_string())),
                )
                .child(
                    Button::new(("graph-layer-remove", i))
                        .small()
                        .ghost()
                        .icon(IconName::Close)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if i < this.layers.len() {
                                this.layers.remove(i);
                            }
                            cx.notify();
                        })),
                )
        });
        card("graph-layers", cx)
            // At the bottom right, growing upward as layers are added.
            .bottom_3()
            .right_3()
            .w(px(380.))
            .child(
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().text_color(theme.foreground).child("Filter layers"))
                    .child(hints::info_button("graph-layers-help", FILTER_HELP))
                    .child(div().flex_1())
                    .child(
                        Button::new("graph-layers-close")
                            .small()
                            .ghost()
                            .icon(IconName::Close)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.layers_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(views_row)
            .children(rows)
            .child(
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("graph-layer-add")
                            .small()
                            .ghost()
                            .icon(IconName::Plus)
                            .label("Layer")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_layer("", Mode::default(), window, cx)
                            })),
                    )
                    .child(div().flex_1())
                    .child(SharedString::from(format!(
                        "{drawn} drawn · {dimmed} dimmed · {hidden} hidden"
                    ))),
            )
            .into_any_element()
    }

    /// The saved views as chips -- a click puts one back, and the one the graph is set up as
    /// is lit -- and the way to keep the current set-up as a new one.
    fn views_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let chips = self.views.iter().enumerate().map(|(i, view)| {
            let lit = self.is_view(view, cx);
            let (name, gone) = (view.name.clone(), view.name.clone());
            let what = format!(
                "{} layer{}{}{}",
                view.layers.len(),
                if view.layers.len() == 1 { "" } else { "s" },
                view.size
                    .as_deref()
                    .map(|s| format!(" · size {}", s.to_uppercase()))
                    .unwrap_or_default(),
                view.detail
                    .as_deref()
                    .map(|d| format!(" · {d} detail"))
                    .unwrap_or_default(),
            );
            h_flex()
                .items_center()
                .rounded(px(12.))
                .border_1()
                .border_color(if lit { theme.primary } else { theme.border })
                .when(lit, |d| d.bg(theme.primary.opacity(0.12)))
                .child(hints::tip(
                    div().id(("graph-view-holder", i)).child({
                        let b = Button::new(("graph-view", i))
                            .xsmall()
                            .ghost()
                            .label(SharedString::from(view.name.clone()));
                        let b = if lit { b.primary() } else { b };
                        b.on_click(cx.listener(move |this, _, window, cx| {
                            this.apply_view(&name, window, cx)
                        }))
                    }),
                    SharedString::from(format!("Put this view back: {what}")),
                ))
                .child(
                    Button::new(("graph-view-delete", i))
                        .xsmall()
                        .ghost()
                        .icon(IconName::Close)
                        .tooltip("Forget this view")
                        .on_click(cx.listener(move |this, _, _, cx| this.delete_view(&gone, cx))),
                )
        });
        let saving = self.saving_view;
        v_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_1()
            .border_b_1()
            .border_color(theme.border.opacity(0.6))
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .items_center()
                    .flex_wrap()
                    .child(div().text_color(theme.muted_foreground).child("Views"))
                    .children(chips)
                    .when(self.views.is_empty() && !saving, |d| {
                        d.child(
                            div()
                                .text_color(theme.muted_foreground.opacity(0.7))
                                .child("none saved"),
                        )
                    })
                    .child(div().flex_1())
                    .when(!saving, |d| {
                        d.child(hints::tip(
                            div().id("graph-view-save-holder").child(
                                Button::new("graph-view-save-open")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Save)
                                    .label("Save view")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.saving_view = true;
                                        this.view_name.update(cx, |s, cx| {
                                            s.set_value("", window, cx);
                                            s.focus(window, cx);
                                        });
                                        cx.notify();
                                    })),
                            ),
                            "Keep the layers, the size and the level of detail as they are now under a name, to put back in one click. A name that is taken is replaced",
                        ))
                    }),
            )
            .when(saving, |d| {
                d.child(
                    h_flex()
                        .w_full()
                        .gap_1()
                        .items_center()
                        .child(div().flex_1().child(Input::new(&self.view_name).small()))
                        .child(
                            Button::new("graph-view-save")
                                .small()
                                .primary()
                                .label("Save")
                                .on_click(cx.listener(|this, _, _, cx| this.save_view(cx))),
                        )
                        .child(
                            Button::new("graph-view-save-cancel")
                                .small()
                                .ghost()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.saving_view = false;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .into_any_element()
    }

    // -- find --------------------------------------------------------------------------------

    /// Open find, with what was last looked for selected for typing over.
    pub(super) fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = true;
        self.find_input.update(cx, |s, cx| s.focus(window, cx));
        self.find_changed(cx);
    }

    pub(super) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = false;
        self.found.clear();
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// What find looks for now, if it is open and something is typed.
    pub(super) fn find_query(&self, cx: &App) -> Option<String> {
        let query = self.find_input.read(cx).value().trim().to_string();
        (self.find_open && !query.is_empty()).then_some(query)
    }

    /// The matches, in reading order over the plane: top to bottom, then left to right.
    pub(super) fn find_now(&self, cx: &App) -> Vec<(String, Found)> {
        let Some(query) = self.find_query(cx) else {
            return Vec::new();
        };
        let mut hits = sift::find(&self.model, &self.shown(cx), &query, self.find_scope);
        let at = |i: usize| {
            self.positions
                .get(&self.model.nodes[i].id)
                .copied()
                .unwrap_or((0., 0.))
        };
        hits.sort_by(|a, b| {
            let (pa, pb) = (at(a.node), at(b.node));
            // Rows of about a node's height read as one line.
            ((pa.1 / 60.).round(), pa.0)
                .partial_cmp(&((pb.1 / 60.).round(), pb.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.into_iter()
            .map(|h| (self.model.nodes[h.node].id.clone(), h.found))
            .collect()
    }

    /// What is typed, or where it is looked for, changed: look again and go to the first.
    pub(super) fn find_changed(&mut self, cx: &mut Context<Self>) {
        self.found = self.find_now(cx);
        self.find_at = 0;
        if let Some((id, _)) = self.found.first() {
            let id = id.clone();
            self.reveal(&[id]);
        }
        cx.notify();
    }

    /// Go to the next match, or the one before.
    pub(super) fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.found = self.find_now(cx);
        let n = self.found.len();
        if n == 0 {
            return;
        }
        self.find_at = if forward {
            (self.find_at + 1) % n
        } else {
            (self.find_at + n - 1) % n
        };
        let id = self.found[self.find_at].0.clone();
        self.reveal(&[id]);
        cx.notify();
    }

    pub(super) fn find_bar(&self, right: f32, top: f32, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let n = self.found.len();
        let typed = !self.find_input.read(cx).value().trim().is_empty();
        let place = match (typed, n) {
            (false, _) => String::new(),
            (true, 0) => "none".to_string(),
            (true, n) => format!("{} of {n}", self.find_at.min(n - 1) + 1),
        };
        // Where the match the search is at was found, when that is not plain to see.
        let where_ = self
            .found
            .get(self.find_at)
            .and_then(|(_, found)| match found {
                Found::Meta(what) => Some(format!("in its {what}")),
                Found::Content => Some("in its text".to_string()),
                Found::Label => None,
            });
        let scope = self.find_scope;
        let toggle = |id: &'static str, label: &'static str, on: bool, which: u8| {
            let b = Button::new(id).small().label(label);
            let b = if on { b.primary() } else { b.outline() };
            b.on_click(cx.listener(move |this, _, _, cx| {
                let s = &mut this.find_scope;
                match which {
                    0 => s.labels = !s.labels,
                    1 => s.content = !s.content,
                    _ => s.meta = !s.meta,
                }
                this.find_changed(cx);
            }))
        };
        card("graph-find", cx)
            .top(px(top))
            .right(px(right))
            .w(px(440.))
            .p_2()
            .border_color(theme.ring)
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(icon(
                                look::icon_path("search"),
                                f32::from(theme.font_size),
                                theme.muted_foreground,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&self.find_input).small()),
                            )
                            .child(
                                div()
                                    .min_w(px(52.))
                                    .flex()
                                    .justify_center()
                                    .text_color(if typed && n == 0 {
                                        theme.danger
                                    } else {
                                        theme.foreground
                                    })
                                    .child(SharedString::from(place)),
                            )
                            .child(
                                Button::new("graph-find-prev")
                                    .small()
                                    .ghost()
                                    .icon(IconName::ChevronUp)
                                    .tooltip("The match before (Shift+Enter)")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.find_step(false, cx)
                                    })),
                            )
                            .child(
                                Button::new("graph-find-next")
                                    .small()
                                    .ghost()
                                    .icon(IconName::ChevronDown)
                                    .tooltip("The next match (Enter)")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.find_step(true, cx)
                                    })),
                            )
                            .child(
                                Button::new("graph-find-close")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Close)
                                    .tooltip("Close find and clear what it marked (Esc)")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.close_find(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child("In")
                            .child(toggle("graph-find-labels", "Labels", scope.labels, 0))
                            .child(toggle("graph-find-content", "Text", scope.content, 1))
                            .child(toggle("graph-find-meta", "Metadata", scope.meta, 2))
                            .children(where_.map(SharedString::from))
                            .child(div().flex_1())
                            .child(
                                Button::new("graph-find-keep")
                                    .small()
                                    .ghost()
                                    .label("Keep as layer")
                                    .tooltip("Make a layer that highlights what these words match, so it stays when find is closed")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if let Some(query) = this.find_query(cx) {
                                            this.add_layer(&query, Mode::Highlight, window, cx);
                                        }
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    // -- the toolbar ---------------------------------------------------------------------------

    pub(super) fn toolbar(
        &self,
        counts: String,
        level: model::Detail,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let layers_on = self.layers_on(cx);
        let size = self.size;
        let own = self
            .selected_nodes()
            .iter()
            .filter(|id| self.own_size.contains_key(*id))
            .count();
        card("graph-header", cx)
            // Off the edges, and a card of its own: what is drawn behind it (a tag box's
            // label, a node) does not show through.
            .top_3()
            .left_3()
            .px_3()
            .py_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(hints::tip(
                        div().id("graph-legend-holder").child({
                            let b = Button::new("graph-legend").small().icon(IconName::Info);
                            let b = if self.legend_open { b.primary() } else { b.ghost() };
                            b.on_click(cx.listener(|this, _, _, cx| {
                                this.legend_open = !this.legend_open;
                                cx.notify();
                            }))
                        }),
                        "The legend: what the colours, shapes and lines on screen mean, and how the plane is worked",
                    ))
                    .child(SharedString::from(counts))
                    .child({
                        // How the graph is arranged: a menu of the ways, the current one marked.
                        let (focus, current) = (self.focus.clone(), self.layout_kind);
                        hints::tip(
                            div().id("graph-layout-holder").child(
                                Button::new("graph-layout")
                                    .small()
                                    .icon(IconName::LayoutDashboard)
                                    .label(current.label())
                                    .dropdown_menu(move |mut menu, _, _| {
                                        menu = menu.action_context(focus.clone());
                                        for kind in LayoutKind::ALL {
                                            let mark = if kind == current { "● " } else { "   " };
                                            menu = menu.menu(
                                                format!("{mark}{}", kind.label()),
                                                Box::new(GraphSetLayout {
                                                    kind: kind.slug().to_string(),
                                                }),
                                            );
                                        }
                                        menu
                                    }),
                            ),
                            SharedString::from(format!(
                                "How the graph is arranged. {}. Picking one arranges the whole graph afresh",
                                current.tip()
                            )),
                        )
                    })
                    .child({
                        // How much is drawn: a level, or "auto", which follows the zoom.
                        let (focus, pick, now) = (self.focus.clone(), self.detail_pick, level);
                        let label = match pick {
                            Some(level) => format!("Detail: {}", level.label()),
                            None => format!("Detail: auto · {}", now.label().to_lowercase()),
                        };
                        hints::tip(
                            div().id("graph-detail-holder").child(
                                Button::new("graph-detail")
                                    .small()
                                    .icon(IconName::Layers)
                                    .label(label)
                                    .dropdown_menu(move |mut menu, _, _| {
                                        menu = menu.action_context(focus.clone());
                                        let mark = |on: bool| if on { "● " } else { "   " };
                                        menu = menu.menu(
                                            format!("{}Auto (by zoom)", mark(pick.is_none())),
                                            Box::new(GraphSetDetail { level: "auto".into() }),
                                        );
                                        for level in [
                                            model::Detail::Basic,
                                            model::Detail::Analysis,
                                            model::Detail::Full,
                                        ] {
                                            menu = menu.menu(
                                                format!("{}{}", mark(pick == Some(level)), level.label()),
                                                Box::new(GraphSetDetail {
                                                    level: level.label().to_lowercase(),
                                                }),
                                            );
                                        }
                                        menu
                                    }),
                            ),
                            SharedString::from(format!(
                                "How much is drawn. Basic: {}. Analysis: {}. Full: {}. Auto follows the zoom: zoomed out the overview, zoomed in the detail",
                                model::Detail::Basic.tip().to_lowercase(),
                                model::Detail::Analysis.tip().to_lowercase(),
                                model::Detail::Full.tip().to_lowercase(),
                            )),
                        )
                    })
                    .child(hints::tip(
                        h_flex()
                            .id("graph-size")
                            .items_center()
                            .child(
                                Button::new("graph-size-smaller")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Minus)
                                    .on_click(cx.listener(|this, _, _, cx| this.step_view(false, cx))),
                            )
                            .child(
                                div()
                                    .px_1()
                                    .text_color(theme.foreground)
                                    .child(SharedString::from(format!("Size {}", size.label()))),
                            )
                            .child(
                                Button::new("graph-size-larger")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Plus)
                                    .on_click(cx.listener(|this, _, _, cx| this.step_view(true, cx))),
                            ),
                        SharedString::from(format!(
                            "How much a node says: {}. Now {} — {}. These buttons change it for every node; the keys [ and ] change it for the selected nodes only (for every node when none is selected), and 0 puts them back{}",
                            Size::ALL
                                .iter()
                                .map(|s| format!("{} {}", s.label(), s.tip()))
                                .collect::<Vec<_>>()
                                .join("; "),
                            size.label(),
                            size.tip(),
                            if own > 0 {
                                format!(". {own} of the selected have a size or width of their own")
                            } else {
                                String::new()
                            }
                        )),
                    ))
                    .child({
                        let label = if layers_on > 0 {
                            format!("Filters · {layers_on}")
                        } else {
                            "Filters".to_string()
                        };
                        let b = Button::new("graph-layers-toggle")
                            .small()
                            .icon(IconName::Funnel)
                            .label(label);
                        let b = if self.layers_open || layers_on > 0 {
                            b.primary()
                        } else {
                            b
                        };
                        hints::tip(
                            div().id("graph-layers-toggle-holder").child(b.on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.layers_open = !this.layers_open;
                                    if this.layers_open && this.layers.is_empty() {
                                        this.add_layer("", Mode::default(), window, cx);
                                    }
                                    cx.notify();
                                }),
                            )),
                            "Filter layers: show only, hide, dim or highlight the nodes a filter matches, one layer on top of another",
                        )
                    })
                    .child(hints::tip(
                        div().id("graph-find-holder").child(
                            Button::new("graph-find-open")
                                .small()
                                .ghost()
                                .icon(IconName::Search)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_find(window, cx)
                                })),
                        ),
                        "Find in the graph (Ctrl/Cmd+F): rings every node that has the words in its label, text or metadata",
                    )),
            )
            .into_any_element()
    }

    // -- the legend --------------------------------------------------------------------------

    pub(super) fn legend(&self, shown: &[bool], plain: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let f = f32::from(theme.font_size) * 0.85;
        let dark = theme.is_dark();
        // The namespaces on screen: their colour, and how many nodes are in each.
        let mut spaces: BTreeMap<String, (gpui_kit::Hsla, usize)> = BTreeMap::new();
        let (mut disputed, mut sure, mut by_agent, mut by_person) = (false, false, false, false);
        for (node, _) in self.model.nodes.iter().zip(shown).filter(|(_, s)| **s) {
            let l = look::look(&self.vocabulary, &node.type_, node.element);
            let root = Vocabulary::root(&node.type_).to_string();
            spaces.entry(root).or_insert((l.colour, 0)).1 += 1;
            disputed |= node.status == "conflicting";
            sure |= node.confidence.is_some();
            by_agent |= node.by_agent;
            by_person |= !node.by_agent;
        }
        let mut spaces: Vec<(String, (gpui_kit::Hsla, usize))> = spaces.into_iter().collect();
        spaces.sort_by(|a, b| b.1.1.cmp(&a.1.1).then(a.0.cmp(&b.0)));
        let lines_of = |kind: fn(&LineKind) -> bool, label: Option<&str>| {
            self.model.lines.iter().any(|l| {
                shown[l.from] && shown[l.to] && kind(&l.kind) && label.is_none_or(|w| l.label == w)
            })
        };
        let heading = |text: &'static str| {
            div()
                .pt_1()
                .text_size(px(f * 0.9))
                .text_color(theme.muted_foreground)
                .child(text)
        };
        let row = |sample: AnyElement, text: String| {
            h_flex()
                .gap_2()
                .items_center()
                .text_color(theme.foreground)
                .child(div().w(px(f * 3.2)).flex().justify_center().child(sample))
                .child(SharedString::from(text))
        };
        let stroke = |colour: gpui_kit::Hsla, width: f32, dashed: bool| {
            div()
                .w(px(f * 3.))
                .border_color(colour)
                .border_t(px(width))
                .when(dashed, |d| d.border_dashed())
                .into_any_element()
        };
        let frame = |colour: gpui_kit::Hsla, dashed: bool, width: f32| {
            div()
                .w(px(f * 2.8))
                .h(px(f))
                .rounded(px(f * 0.3))
                .border_color(colour)
                .border(px(width))
                .when(dashed, |d| d.border_dashed())
                .into_any_element()
        };
        let mut body = v_flex()
            .gap_1()
            .child(heading("TYPE · THE HEADER'S COLOUR"))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_x_3()
                    .gap_y_1()
                    .max_w(px(f * 26.))
                    .children(spaces.iter().take(14).map(|(root, (colour, count))| {
                        h_flex()
                            .gap_1()
                            .items_center()
                            .text_color(theme.foreground)
                            .child(
                                div()
                                    .size(px(f * 0.8))
                                    .rounded(px(f * 0.15))
                                    .bg(look::ink(*colour, dark)),
                            )
                            .child(SharedString::from(format!("{root}. {count}")))
                    })),
            );
        if spaces.is_empty() {
            body = body.child("Nothing is drawn yet");
        }
        body = body.child(heading("ON A NODE"));
        if sure && !plain {
            body = body.child(row(
                meter(5, 7, theme.foreground, theme.border, f),
                format!("how sure, of {}", self.scale.len().max(7)),
            ));
        }
        if disputed && !plain {
            body = body.child(row(frame(theme.warning, true, 2.), "disputed".into()));
        }
        body = body
            .child(row(frame(theme.primary, false, 2.), "selected".into()))
            .child(row(
                frame(theme.foreground, false, 2.),
                "found by the search".into(),
            ))
            .child(row(
                frame(theme.info, false, 2.),
                "highlighted by a layer".into(),
            ))
            .child(row(
                div()
                    .w(px(f * 2.8))
                    .h(px(f))
                    .rounded(px(f * 0.3))
                    .border_1()
                    .border_color(theme.muted_foreground)
                    .opacity(0.4)
                    .into_any_element(),
                "dimmed by a layer, or deleted".into(),
            ));
        if by_agent {
            body = body.child(row(
                icon(look::icon_path("sparkle"), f, theme.foreground),
                "written by the agent".into(),
            ));
        }
        if by_person {
            body = body.child(row(
                icon(look::icon_path("user"), f, theme.foreground),
                "written by a person".into(),
            ));
        }
        body = body.child(heading("LINES"));
        let relation = |k: &LineKind| matches!(k, LineKind::Relation { .. });
        let role = |k: &LineKind| matches!(k, LineKind::Role);
        let note = |k: &LineKind| matches!(k, LineKind::Annotation);
        if lines_of(relation, None) {
            body = body.child(row(
                stroke(theme.foreground.opacity(0.7), 2., false),
                "relation, with an arrow where it has a direction".into(),
            ));
        }
        if lines_of(role, None) {
            body = body.child(row(
                stroke(theme.muted_foreground, 1., false),
                "a part in an event or observation".into(),
            ));
        }
        if lines_of(note, None) {
            body = body.child(row(
                stroke(theme.muted_foreground, 2., true),
                "what a note or assessment is about".into(),
            ));
        }
        if lines_of(note, Some("supports")) {
            body = body.child(row(stroke(theme.success, 2., true), "supports".into()));
        }
        if lines_of(note, Some("refutes")) {
            body = body.child(row(stroke(theme.danger, 2., true), "refutes".into()));
        }
        if !plain && self.model.lines.iter().any(|l| l.status == "conflicting") {
            body = body.child(row(stroke(theme.warning, 3., true), "disputed".into()));
        }
        card("graph-legend-card", cx)
            .top(px(60.))
            .left_3()
            .p_3()
            .max_w(px(f * 30.))
            .child(
                body.child(
                    div()
                        .pt_2()
                        .mt_1()
                        .border_t_1()
                        .border_color(theme.border)
                        .child(PLANE_HELP),
                ),
            )
            .into_any_element()
    }

    // -- a type's icon and colour ------------------------------------------------------------

    /// Give the type the picker is open on an icon or a colour, keeping the other; `None`
    /// for both takes its own look away.
    fn set_look(
        &mut self,
        icon: Option<Option<String>>,
        colour: Option<Option<String>>,
        cx: &mut Context<Self>,
    ) {
        let (Some(picker), Some(b)) = (&self.picker, self.backend.clone()) else {
            return;
        };
        let own = self.vocabulary.types.get(&picker.type_);
        let icon = icon.unwrap_or_else(|| own.and_then(|t| t.icon.clone()));
        let colour = colour.unwrap_or_else(|| own.and_then(|t| t.colour.clone()));
        match vocab::set_look(
            b.case.db(),
            &picker.type_,
            picker.kind,
            icon.as_deref(),
            colour.as_deref(),
        ) {
            Ok(()) => {
                self.vocabulary = vocab::load(b.case.db()).unwrap_or_default();
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    pub(super) fn picker_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.picker.as_ref()?;
        let theme = cx.theme().clone();
        let f = f32::from(theme.font_size);
        let dark = theme.is_dark();
        let now = look::look(&self.vocabulary, &picker.type_, picker.kind);
        let uses = self
            .model
            .nodes
            .iter()
            .filter(|n| n.type_ == picker.type_)
            .count();
        let typed = self.picker_input.read(cx).value().to_string();
        let offered = look::search(&typed, 48);
        let area = self.bounds.get().size;
        let at = (
            picker.at.0.min(f32::from(area.width) - 340.).max(8.),
            picker.at.1.min(f32::from(area.height) - 420.).max(8.),
        );
        let icons = offered.iter().enumerate().map(|(i, name)| {
            let picked = *name == now.icon;
            let name = name.to_string();
            hints::tip(
                div()
                    .id(("graph-pick-icon", i))
                    .size(px(f * 2.4))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .border_1()
                    .border_color(if picked { theme.primary } else { theme.border })
                    .bg(if picked {
                        theme.primary.opacity(0.14)
                    } else {
                        theme.muted
                    })
                    .cursor_pointer()
                    .child(icon(
                        look::icon_path(&name),
                        f * 1.2,
                        if picked {
                            theme.primary
                        } else {
                            theme.foreground
                        },
                    ))
                    .on_click(cx.listener({
                        let name = name.clone();
                        move |this, _, _, cx| this.set_look(Some(Some(name.clone())), None, cx)
                    })),
                name,
            )
        });
        let swatches = look::PALETTE.iter().enumerate().map(|(i, (name, _))| {
            let colour = look::colour(name).unwrap_or_default();
            let picked = *name == now.colour_name;
            hints::tip(
                div()
                    .id(("graph-pick-colour", i))
                    .size(px(f * 1.5))
                    .rounded_full()
                    .bg(look::ink(colour, dark))
                    .border_2()
                    .border_color(if picked {
                        theme.foreground
                    } else {
                        theme.popover
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_look(None, Some(Some(name.to_string())), cx)
                    })),
                *name,
            )
        });
        Some(
            card("graph-picker", cx)
                .left(px(at.0))
                .top(px(at.1))
                .w(px(332.))
                .p_3()
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(icon(now.path.clone(), f * 1.3, look::ink(now.colour, dark)))
                                .child(
                                    v_flex()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_color(theme.foreground)
                                                .font_family(theme.mono_font_family.clone())
                                                .child(SharedString::from(picker.type_.clone())),
                                        )
                                        .child(SharedString::from(format!(
                                            "Changes {} of this type",
                                            match uses {
                                                1 => "the 1 node".to_string(),
                                                n => format!("all {n} nodes"),
                                            }
                                        ))),
                                ),
                        )
                        .child(Input::new(&self.picker_input).small())
                        .child(h_flex().flex_wrap().gap_1().children(icons))
                        .when(offered.is_empty(), |d| d.child("No icon is called that"))
                        .child(h_flex().flex_wrap().gap_1().children(swatches))
                        .child(
                            h_flex()
                                .items_center()
                                .child(hints::tip(
                                    div().id("graph-pick-reset-holder").child(
                                        Button::new("graph-pick-reset")
                                            .small()
                                            .ghost()
                                            .label("Back to the default")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.set_look(Some(None), Some(None), cx)
                                            })),
                                    ),
                                    "Take the type's own icon and colour away: it is drawn as its namespace says, or as its kind is",
                                ))
                                .child(div().flex_1())
                                .child(
                                    Button::new("graph-pick-done")
                                        .small()
                                        .primary()
                                        .label("Done")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.picker = None;
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Esc: close what is open over the plane, the nearest thing first.
    pub(super) fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.saving_view) {
            window.focus(&self.focus, cx);
        } else if self.picker.take().is_some() || self.form.take().is_some() {
            window.focus(&self.focus, cx);
        } else if self.find_open {
            self.close_find(window, cx);
        } else if self.legend_open {
            self.legend_open = false;
        } else {
            // A relation being made is given up; the selection stays, as it does on a click.
            self.relating = None;
        }
        cx.notify();
    }
}
