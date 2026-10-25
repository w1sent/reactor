//! A node as it is drawn: a card that says what the thing is before it says anything else.
//!
//! The header carries the type -- its icon, and its namespace's colour -- and, at its right,
//! what there is to know at a glance: a hypothesis's standing, how sure, who wrote it. Under
//! it the value or the text, wrapped and never cut. A disputed element has a dashed orange
//! border and says so; a claim that stays a node is an outline with rounder corners; what was
//! concluded sits on a lighter ground than what was found.
//!
//! How much a card says is its size ([`crate::graph::card`]): the view's, or the node's own.

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::{ActiveTheme as _, Theme};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, BoxShadow, Context, FontWeight, HighlightStyle, Hsla, MouseButton, SharedString,
    StyledText, Window, div, font, point, px, svg,
};
use reactor_db::graph::fields::Shape;
use reactor_db::graph::layout::{self, NodeSize};

use super::prop_draw;
use super::{Drag, GraphPanel, Selected};
use crate::graph::card::{self, Card, Face, Metrics, Size};
use crate::graph::look::{self, Look};
use crate::graph::model::{self, Node, NodeKind};
use crate::graph::sift;
use crate::hints;

/// What a node is drawn with, besides what is written on it.
pub(super) struct NodeView<'a> {
    pub index: usize,
    pub card: &'a Card,
    /// Its centre in the drawing area.
    pub at: (f32, f32),
    pub zoom: f32,
    /// Status and confidence are not drawn.
    pub plain: bool,
    /// The text is big enough to read; else the card is a coloured block.
    pub readable: bool,
    pub selected: bool,
    /// The only thing selected: it shows the grip its width is dragged by.
    pub alone: bool,
    pub drawn: sift::Drawn,
    /// Found by the search; `Some(true)` for the match the search is at.
    pub hit: Option<bool>,
    /// What the search looks for, to mark it in the text.
    pub query: Option<&'a str>,
}

/// The seven steps of the confidence scale as little bars, `filled` of them lit.
pub(super) fn meter(filled: usize, of: usize, colour: Hsla, empty: Hsla, unit: f32) -> AnyElement {
    h_flex()
        .flex_shrink_0()
        .gap(px((unit * 0.09).max(1.)))
        .children((0..of).map(|i| {
            div()
                .w(px((unit * 0.3).max(1.5)))
                .h(px(unit * 0.7))
                .bg(if i < filled { colour } else { empty })
        }))
        .into_any_element()
}

/// An icon by name, as big as `size` pixels, in `colour`.
pub(super) fn icon(path: SharedString, size: f32, colour: Hsla) -> AnyElement {
    svg()
        .path(path)
        .flex_shrink_0()
        .size(px(size))
        .text_color(colour)
        .into_any_element()
}

/// A ring round a card, glowing a little: what the search found, what a layer highlights.
fn ring(colour: Hsla, width: f32, glow: f32) -> Vec<BoxShadow> {
    vec![
        BoxShadow {
            color: colour,
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(width),
            inset: false,
        },
        BoxShadow {
            color: colour.opacity(0.35),
            offset: point(px(0.), px(0.)),
            blur_radius: px(glow),
            spread_radius: px(width + glow * 0.3),
            inset: false,
        },
    ]
}

impl GraphPanel {
    /// How wide text runs in the fonts in use, at the size the plane writes in at zoom 1.
    pub(super) fn measure(&self, window: &mut Window, theme: &Theme) -> Metrics {
        let size = px(100.);
        let text = window.text_system();
        let advance = |family: &SharedString, sample: &str| {
            let id = text.resolve_font(&font(family.clone()));
            let total: f32 = sample
                .chars()
                .map(|c| f32::from(text.layout_width(id, size, c)))
                .sum();
            total / sample.chars().count().max(1) as f32 / 100.
        };
        let usual = Metrics::default();
        let mono = advance(&theme.mono_font_family, "0");
        // What prose is mostly made of, spaces and all.
        let sans = advance(
            &theme.font_family,
            "the evidence was written to disk before 2 March, it seems",
        );
        let metrics = Metrics {
            font: f32::from(theme.font_size) * 0.85,
            mono: if mono.is_finite() && mono > 0.2 {
                mono
            } else {
                usual.mono
            },
            // A little over the average, so that a line is not thought to hold more than it does.
            sans: if sans.is_finite() && sans > 0.2 {
                sans * 1.06
            } else {
                usual.sans
            },
        };
        self.metrics.set(metrics);
        metrics
    }

    /// The size a node is drawn at and the width it was given by hand, if any.
    pub(super) fn size_of(&self, id: &str) -> (Size, Option<f32>) {
        match self.own_size.get(id) {
            Some((size, width)) => (size.unwrap_or(self.size), *width),
            None => (self.size, None),
        }
    }

    /// What every node says at its size, and how big that is.
    pub(super) fn cards(&self, plain: bool, metrics: &Metrics) -> Vec<Card> {
        self.model
            .nodes
            .iter()
            .map(|node| {
                let (size, width) = self.size_of(&node.id);
                card::card(node, size, width, plain, metrics)
            })
            .collect()
    }

    /// The nodes a size change applies to: the selected ones, else none (the view's size is
    /// what changes).
    fn sized(&self) -> Vec<String> {
        self.selected_nodes()
    }

    /// Keep how these nodes are sized with the case.
    pub(super) fn persist_sizes(&self, ids: &[String]) {
        let (Some(b), false) = (&self.backend, ids.is_empty()) else {
            return;
        };
        let rows: Vec<(String, NodeSize)> = ids
            .iter()
            .map(|id| {
                let (size, width) = self.own_size.get(id).copied().unwrap_or((None, None));
                (
                    id.clone(),
                    NodeSize {
                        size: size.map(|s| s.slug().to_string()),
                        width,
                    },
                )
            })
            .collect();
        let _ = layout::save_sizes(b.case.db(), &rows);
    }

    /// Read how nodes are sized from the case.
    pub(super) fn load_sizes(&mut self) {
        self.own_size = match &self.backend {
            Some(b) => layout::load_sizes(b.case.db())
                .unwrap_or_default()
                .into_iter()
                .map(|(id, s)| (id, (s.size.as_deref().and_then(Size::parse), s.width)))
                .collect(),
            None => Default::default(),
        };
    }

    /// One step smaller or larger: the selected nodes, or every node when none is selected.
    pub(super) fn step_size(&mut self, larger: bool, cx: &mut Context<Self>) {
        let step = |s: Size| if larger { s.larger() } else { s.smaller() };
        let ids = self.sized();
        if ids.is_empty() {
            self.size = step(self.size);
            self.make_room(&[]);
        } else {
            for id in &ids {
                let (now, _) = self.size_of(id);
                let next = step(now);
                let entry = self.own_size.entry(id.clone()).or_insert((None, None));
                // A size that is the view's is not the node's own.
                entry.0 = (next != self.size).then_some(next);
            }
            self.own_size.retain(|_, v| v.0.is_some() || v.1.is_some());
            self.persist_sizes(&ids);
            self.make_room(&ids);
        }
        cx.notify();
    }

    /// One step smaller or larger for the view: every node that has no size of its own. What
    /// the toolbar's buttons do, whatever is selected, so that the size they show is the one
    /// they change.
    pub(super) fn step_view(&mut self, larger: bool, cx: &mut Context<Self>) {
        self.size = if larger {
            self.size.larger()
        } else {
            self.size.smaller()
        };
        // A node whose own size is now the view's has none of its own any more.
        let size = self.size;
        let mut same = Vec::new();
        for (id, own) in self.own_size.iter_mut() {
            if own.0 == Some(size) {
                own.0 = None;
                same.push(id.clone());
            }
        }
        self.own_size.retain(|_, v| v.0.is_some() || v.1.is_some());
        self.persist_sizes(&same);
        self.make_room(&[]);
        cx.notify();
    }

    /// Back to the size and width that follow from what a node says: the selected nodes, or
    /// every node -- and the view back to its usual size -- when none is selected.
    pub(super) fn auto_size(&mut self, cx: &mut Context<Self>) {
        let ids = self.sized();
        if ids.is_empty() {
            let all: Vec<String> = self.own_size.keys().cloned().collect();
            self.own_size.clear();
            self.persist_sizes(&all);
            self.size = Size::default();
            self.make_room(&[]);
        } else {
            for id in &ids {
                self.own_size.remove(id);
            }
            self.persist_sizes(&ids);
            self.make_room(&ids);
        }
        cx.notify();
    }

    /// Move nodes apart where they now overlap. Those in `keep` stay and the others give
    /// way: a node made bigger pushes its neighbours aside, it does not move itself.
    pub(super) fn make_room(&mut self, keep: &[String]) {
        let cards = self.cards(false, &self.metrics.get());
        let sizes: Vec<(f32, f32)> = cards.iter().map(|c| (c.w, c.h)).collect();
        let fixed: Vec<bool> = self
            .model
            .nodes
            .iter()
            .map(|n| keep.contains(&n.id))
            .collect();
        let mut pos: Vec<(f32, f32)> = self
            .model
            .nodes
            .iter()
            .map(|n| self.positions.get(&n.id).copied().unwrap_or((0., 0.)))
            .collect();
        let before = pos.clone();
        model::separate(&mut pos, &sizes, &fixed, ROOM);
        let mut moved = Vec::new();
        for ((node, now), was) in self.model.nodes.iter().zip(pos).zip(before) {
            if now != was {
                self.positions.insert(node.id.clone(), now);
                moved.push(node.id.clone());
            }
        }
        self.persist(&moved);
    }

    /// Open the look of this node's type for changing, at the pointer.
    fn pick_look(
        &mut self,
        index: usize,
        at: (f32, f32),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(node) = self.model.nodes.get(index) else {
            return;
        };
        self.picker = Some(super::Picker {
            type_: node.type_.clone(),
            kind: node.element,
            at,
        });
        self.picker_input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    /// One node, drawn.
    pub(super) fn node_element(
        &self,
        node: &Node,
        look: &Look,
        v: NodeView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let metrics = self.metrics.get();
        let (z, card) = (v.zoom, v.card);
        // One font size at this zoom: every measure of the card is a multiple of it.
        let f = metrics.font * z;
        let (w, h) = (card.w * z, card.h * z);
        let dark = theme.is_dark();
        let hue = look::ink(look.colour, dark);
        let disputed = node.status == "conflicting" && !v.plain;
        let deleted = node.status == "soft-deleted";
        let claim = node.kind == NodeKind::Claim;
        let concluded = node.kind == NodeKind::Annotation;

        let ground = match node.kind {
            NodeKind::Entity => theme.muted,
            NodeKind::Claim => theme.background,
            NodeKind::Annotation => theme.secondary,
        };
        let edge = if v.selected {
            theme.primary
        } else if disputed {
            theme.warning
        } else if concluded {
            theme.muted_foreground.opacity(0.55)
        } else if claim {
            theme.muted_foreground.opacity(0.8)
        } else {
            theme.border
        };
        let radius = if claim { 1.3 * f } else { 0.6 * f };
        let border = if v.selected || disputed { 2. } else { 1. };

        // What the search found is ringed in the brightest thing there is; what a layer
        // highlights, in the theme's blue.
        let shadows = match (v.hit, v.drawn.highlight) {
            (Some(true), _) => ring(theme.foreground, 3., 18.),
            (Some(false), _) => ring(theme.foreground.opacity(0.75), 2., 10.),
            (None, true) => ring(theme.info, 2., 8.),
            (None, false) => Vec::new(),
        };

        let id = node.id.clone();
        let index = v.index;
        let mut holder = div()
            .id(("graph-node", v.index))
            .absolute()
            .left(px(v.at.0 - w / 2.))
            .top(px(v.at.1 - h / 2.))
            .w(px(w))
            // As tall as the text is thought to be, and taller if it turns out to need it:
            // nothing is ever cut off.
            .min_h(px(h))
            .flex()
            .flex_col()
            .rounded(px(radius))
            .border_color(edge)
            .bg(ground)
            .cursor_pointer()
            .when(border > 1., |d| d.border_2())
            .when(border <= 1., |d| d.border_1())
            .when(disputed && !v.selected, |d| d.border_dashed())
            .when(!shadows.is_empty(), |d| d.shadow(shadows))
            .when(deleted, |d| d.opacity(0.45))
            .when(v.drawn.dim && !deleted, |d| d.opacity(0.38))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui_kit::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    let at = this.local(event.position);
                    this.pick_look(index, at, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui_kit::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    // The plane has the keys from here on: [ ] 0, find.
                    window.focus(&this.focus, cx);
                    // Making a relation: this is its other end.
                    if let Some(from) = this.relating.clone()
                        && from != id
                    {
                        this.open_relation_form(vec![from, id.clone()], window, cx);
                        return;
                    }
                    let m = event.modifiers;
                    // Alt drags its width.
                    if m.alt {
                        let start = this.width_of(&id);
                        let from = this.to_world(this.local(event.position)).0;
                        let grows = this.positions.get(&id).is_none_or(|p| from >= p.0);
                        this.drag = Some(Drag::Width {
                            id: id.clone(),
                            from,
                            start,
                            grows,
                        });
                        return;
                    }
                    // Shift or Ctrl/Cmd picks it up or puts it down.
                    if m.shift || m.secondary() {
                        this.toggle(&id, cx);
                        return;
                    }
                    // Pressing one of several that are selected drags them all;
                    // pressing another selects just it.
                    if !this.selected_nodes().contains(&id) {
                        this.select(Some(Selected::Nodes(vec![id.clone()])), cx);
                    }
                    let from = this.to_world(this.local(event.position));
                    let starts = this
                        .selected_nodes()
                        .into_iter()
                        .filter_map(|n| this.positions.get(&n).map(|p| (n, *p)))
                        .collect();
                    this.drag = Some(Drag::Nodes { from, starts });
                }),
            );

        // What the box cannot say at this size, its tooltip does.
        let mut full = match node.kind {
            NodeKind::Claim => node.type_.clone(),
            _ => format!("{} · {}", node.type_, node.label),
        };
        if let (Some(c), false) = (&node.confidence, v.plain) {
            full.push_str(&format!(" · {c}"));
        }
        if disputed {
            full.push_str(" · disputed");
        }

        if !v.readable {
            // Too small to read: the colour of what it is, and nothing else.
            holder = holder.child(
                div()
                    .w_full()
                    .h(px((h * 0.45).max(2.)))
                    .rounded_t(px(radius))
                    .bg(hue.opacity(if claim { 0.25 } else { 0.6 })),
            );
            return hints::tip(holder, full).into_any_element();
        }

        let mono = theme.mono_font_family.clone();
        let small = px(card::SMALL * f);
        let muted = theme.muted_foreground;
        let ink = theme.foreground;
        let mark = HighlightStyle {
            background_color: Some(theme.primary),
            color: Some(theme.primary_foreground),
            ..Default::default()
        };
        // Text with what the search looks for marked in it.
        let marked = |text: &str| -> AnyElement {
            let marks = v.query.map(|q| sift::marks(text, q)).unwrap_or_default();
            let text = SharedString::from(text.to_string());
            if marks.is_empty() {
                div().child(text).into_any_element()
            } else {
                StyledText::new(text)
                    .with_highlights(marks.into_iter().map(|r| (r, mark)))
                    .into_any_element()
            }
        };

        if let Some(head) = &card.head {
            let head_ink = if claim { ink } else { hue };
            let filled = node
                .confidence
                .as_ref()
                .and_then(|c| self.scale.iter().position(|s| s == c))
                .map_or(0, |i| i + 1);
            let standing = head.chip.as_deref().map(|chip| {
                let colour = match chip {
                    "supported" => theme.success,
                    "rejected" => theme.danger,
                    _ => muted,
                };
                div()
                    .flex_shrink_0()
                    .px(px(0.45 * f))
                    .rounded(px(0.7 * f))
                    .border_1()
                    .border_color(colour)
                    .text_color(colour)
                    .font_family(theme.font_family.clone())
                    .child(SharedString::from(chip.to_string()))
            });
            holder = holder.child(
                h_flex()
                    .w_full()
                    .h(px(card::HEAD_H * f))
                    .flex_shrink_0()
                    .px(px(card::PAD_X * f))
                    .gap(px(0.5 * f))
                    .items_center()
                    .rounded_t(px((radius - border).max(0.)))
                    .when(!claim, |d| {
                        d.bg(hue.opacity(if dark { 0.13 } else { 0.16 }))
                    })
                    .text_size(small)
                    .text_color(head_ink)
                    .font_family(mono.clone())
                    .child(icon(look.path.clone(), card::ICON * f, head_ink))
                    .child(div().min_w_0().truncate().child(marked(&head.label)))
                    .children(standing)
                    .child(div().flex_1())
                    .when(head.meter && filled > 0, |d| {
                        d.child(meter(
                            filled,
                            self.scale.len().max(filled),
                            head_ink,
                            theme.border,
                            f,
                        ))
                    })
                    .when(head.author, |d| {
                        d.child(icon(
                            look::icon_path(if node.by_agent { "sparkle" } else { "user" }),
                            card::ICON * f * 0.9,
                            head_ink.opacity(0.8),
                        ))
                    }),
            );
        }

        let start_only = card.size == Size::M && concluded;
        let mut body = v_flex()
            .w_full()
            .px(px(card::PAD_X * f))
            .pt(px(card::PAD_TOP * f))
            .pb(px(card::PAD_BOTTOM * f))
            .gap(px(card::GAP * f))
            .text_size(px(f))
            .line_height(px(card::LINE * f))
            .text_color(ink);
        for (i, block) in card.blocks.iter().enumerate() {
            let text = if start_only && i == 0 {
                card::start_of(&block.text, card.w, &metrics)
            } else {
                block.text.clone()
            };
            let words = div()
                .min_w_0()
                .flex_1()
                .when(block.face == Face::Mono, |d| d.font_family(mono.clone()))
                .when(block.strong, |d| d.font_weight(FontWeight::SEMIBOLD))
                .when(block.muted, |d| d.text_color(muted).text_size(small))
                .when(deleted, |d| d.line_through())
                .child(marked(&text));
            let row = h_flex().w_full().items_start().gap(px(0.5 * f));
            body = body.child(if card.head.is_none() {
                // The smallest size: the icon, and the name.
                row.items_center()
                    .child(icon(look.path.clone(), card::ICON * f, hue))
                    .child(words)
                    .when(card.author, |d| {
                        d.child(icon(
                            look::icon_path(if node.by_agent { "sparkle" } else { "user" }),
                            card::ICON * f * 0.9,
                            muted,
                        ))
                    })
            } else if block.bullet {
                row.child(div().flex_shrink_0().text_color(muted).child("•"))
                    .child(words)
            } else {
                row.child(words)
            });
        }
        if card.more > 0 {
            let id = node.id.clone();
            body = body.child(
                div()
                    .id(("graph-node-more", v.index))
                    .text_size(small)
                    .line_height(px(card::ROW * f - card::GAP * f))
                    .text_color(theme.link)
                    .child(SharedString::from(format!(
                        "{} more line{}",
                        card.more,
                        if card.more == 1 { "" } else { "s" }
                    )))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            // Open this one in full; the rest stay as they are.
                            cx.stop_propagation();
                            let entry = this.own_size.entry(id.clone()).or_insert((None, None));
                            entry.0 = Some(Size::Xl);
                            this.persist_sizes(std::slice::from_ref(&id));
                            this.make_room(std::slice::from_ref(&id));
                            cx.notify();
                        }),
                    ),
            );
        }
        if !card.tags.is_empty() {
            body = body.child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap(px(0.4 * f))
                    .text_size(small)
                    .line_height(px(card::ROW * f * 0.8))
                    .font_family(mono.clone())
                    .text_color(muted)
                    .children(card.tags.iter().map(|tag| {
                        div()
                            .px(px(0.4 * f))
                            .rounded(px(0.25 * f))
                            .bg(theme.border)
                            .child(marked(tag))
                    })),
            );
        }
        if !card.props.is_empty() {
            body = body.child(
                v_flex()
                    .w_full()
                    .pt(px(card::GAP * f))
                    .border_t_1()
                    .border_color(theme.border)
                    .text_size(small)
                    .line_height(px(card::LINE * card::SMALL * f))
                    .font_family(mono.clone())
                    .children(card.props.iter().enumerate().map(|(at, row)| {
                        let parts = row.parts.iter();
                        let value = match row.shape {
                            // The same as the inspector draws: chips, a code block with line
                            // numbers, or text (a link in the link colour, a value that does not
                            // suit its name struck through). A card is dragged, so a click on a
                            // link is the inspector's.
                            Shape::Category => h_flex()
                                .flex_wrap()
                                .gap(px(0.2 * f))
                                .children(parts.map(|p| {
                                    let colour =
                                        prop_draw::category_colour(&self.vocabulary, &p.text, &theme);
                                    prop_draw::chip(&p.text, colour, small, px(card::CHIP_H * f))
                                }))
                                .into_any_element(),
                            Shape::Verbatim => v_flex()
                                .w_full()
                                .gap(px(card::PART_GAP * f))
                                .children(parts.map(|p| {
                                    prop_draw::verbatim_block(&p.text, small, px(0.2 * f), &theme)
                                }))
                                .into_any_element(),
                            _ => v_flex()
                                .w_full()
                                .gap(px(card::PART_GAP * f))
                                .children(parts.map(|p| {
                                    div()
                                        .when(p.go.is_some(), |d| d.text_color(theme.primary))
                                        .when(p.mismatch, |d| {
                                            d.text_color(muted).line_through()
                                        })
                                        .child(marked(&p.text))
                                }))
                                .into_any_element(),
                        };
                        h_flex()
                            .w_full()
                            .items_start()
                            .px(px(card::ROW_PX * f))
                            .py(px(card::ROW_PAD * f))
                            .rounded(px(0.3 * f))
                            .when_some(prop_draw::stripe(&theme, at), |d, shade| d.bg(shade))
                            .child(
                                div()
                                    .w(px(card::KEY_W * card::SMALL * f))
                                    .flex_shrink_0()
                                    .truncate()
                                    .text_color(muted)
                                    .child(marked(&row.label)),
                            )
                            .child(div().min_w_0().flex_1().child(value))
                    })),
            );
        }
        if let Some(foot) = &card.foot {
            let tallies = node.supports > 0 && foot.starts_with('+');
            body = body.child(
                div()
                    .text_size(small)
                    .line_height(px(card::ROW * f - card::GAP * f))
                    .text_color(if tallies { theme.success } else { muted })
                    .child(SharedString::from(foot.clone())),
            );
        }
        if card.disputed {
            body = body.child(
                h_flex()
                    .gap(px(0.4 * f))
                    .items_center()
                    .text_size(small)
                    .line_height(px(card::ROW * f - card::GAP * f))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.warning)
                    .child(icon(
                        look::icon_path("triangle-alert"),
                        card::ICON * f * 0.9,
                        theme.warning,
                    ))
                    .child("disputed"),
            );
        }
        holder = holder.child(body);

        if v.alone {
            // The grip its width is dragged by: the height follows the text.
            let id = node.id.clone();
            holder = holder.child(
                div()
                    .id(("graph-node-grip", v.index))
                    .absolute()
                    .top_0()
                    .right(px(-5.))
                    .w(px(9.))
                    .h_full()
                    .cursor_col_resize()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui_kit::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let start = this.width_of(&id);
                            let from = this.to_world(this.local(event.position)).0;
                            this.drag = Some(Drag::Width {
                                id: id.clone(),
                                from,
                                start,
                                grows: true,
                            });
                        }),
                    ),
            );
        }
        hints::tip(holder, full).into_any_element()
    }

    /// How wide a node is drawn now, in world units.
    pub(super) fn width_of(&self, id: &str) -> f32 {
        let (size, width) = self.size_of(id);
        self.model
            .nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| card::card(n, size, width, false, &self.metrics.get()).w)
            .unwrap_or(card::HAND_MIN)
    }

    /// The pointer moved while a width is being dragged: the card is as wide as that makes
    /// it, about its centre.
    pub(super) fn drag_width(&mut self, id: &str, from: f32, start: f32, grows: bool, at: f32) {
        let k = self.metrics.get().font / 12.;
        let delta = (at - from) * if grows { 2. } else { -2. };
        let width = (start + delta).clamp(card::HAND_MIN * k, card::HAND_MAX * k);
        self.own_size
            .entry(id.to_string())
            .or_insert((None, None))
            .1 = Some(width);
    }
}

/// The room kept between two nodes when they are moved apart.
pub(super) const ROOM: f32 = 18.;
