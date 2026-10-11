//! Tags in the inspector: chips in the colour the tag is drawn in, added and taken off where
//! they are, for one element or for several at once.
//!
//! A tag is on every selected element (a solid chip) or on some of them (a dashed one, with how
//! many). The colour is the tag's: the one it was given, its dotted prefix's, or the one its
//! name gives it -- the colour of its box on the plane. The dot on a chip chooses it.

use std::collections::BTreeMap;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::{Sizable as _, Theme};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, SharedString, Window, div, px};
use reactor_db::graph::edit;
use reactor_db::graph::vocab;

use super::GraphPanel;
use crate::graph::look;
use crate::hints;

impl GraphPanel {
    /// The tags on the selection, each with how many of it have it.
    fn selection_tags(&self) -> BTreeMap<String, usize> {
        let ids = self.selected_ids();
        let mut tags: BTreeMap<String, usize> = BTreeMap::new();
        if let [only] = ids.as_slice()
            && let Some(i) = self.inspected.as_ref().filter(|i| i.id == *only)
        {
            for t in &i.tags {
                *tags.entry(t.clone()).or_default() += 1;
            }
            return tags;
        }
        for id in &ids {
            if let Some(n) = self.model.nodes.iter().find(|n| n.id == *id) {
                for t in &n.tags {
                    *tags.entry(t.clone()).or_default() += 1;
                }
            }
        }
        tags
    }

    /// Every tag the graph has, with how many nodes carry it.
    fn case_tags(&self) -> BTreeMap<String, usize> {
        let mut tags: BTreeMap<String, usize> = BTreeMap::new();
        for n in &self.model.nodes {
            for t in &n.tags {
                *tags.entry(t.clone()).or_default() += 1;
            }
        }
        tags
    }

    /// Put these tags on, and take these off, everything selected.
    pub(super) fn change_tags(
        &mut self,
        add: &[String],
        remove: &[String],
        cx: &mut Context<Self>,
    ) {
        let ids = self.selected_ids();
        if ids.is_empty() || (add.is_empty() && remove.is_empty()) {
            return;
        }
        self.edit(cx, |db, at| edit::tag(db, at, &ids, add, remove));
    }

    /// Add what is typed in the tag field.
    pub(super) fn add_typed_tag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.tag.read(cx).value().trim().to_lowercase();
        if typed.is_empty() {
            return;
        }
        self.change_tags(&[typed], &[], cx);
        self.tag.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    /// Give a tag a colour (or, with `None`, the one its name gives it).
    pub(super) fn set_tag_colour(
        &mut self,
        tag: &str,
        colour: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some(b) = self.backend.clone() else {
            return;
        };
        match vocab::set_tag_colour(b.case.db(), tag, colour) {
            Ok(()) => {
                self.vocabulary = vocab::load(b.case.db()).unwrap_or_default();
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    /// The tags of the selection as chips, with a field to add one. The same for one element
    /// and for several; a change applies to all that are selected.
    pub(super) fn tags_section(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let small = theme.font_size * 0.85;
        let dark = theme.is_dark();
        let total = self.selected_ids().len().max(1);
        let tags = self.selection_tags();

        let chips = tags.iter().enumerate().map(|(i, (tag, n))| {
            let colour = look::tag_colour(&self.vocabulary, tag, dark);
            let partial = *n < total;
            let colouring = self.tag_colouring.as_deref() == Some(tag.as_str());
            let (dot_tag, add_tag, drop_tag) = (tag.clone(), tag.clone(), tag.clone());
            let chip = h_flex()
                .id(("graph-tag-chip", i))
                .gap_1p5()
                .items_center()
                .h(px(24.))
                .pl_2()
                .pr_1()
                .rounded_full()
                .border_1()
                .border_color(colour.opacity(if partial { 0.45 } else { 0.7 }))
                .when(partial, |d| d.border_dashed())
                .bg(colour.opacity(if partial { 0.05 } else { 0.14 }))
                .text_size(small)
                .child(hints::tip(
                    div()
                        .id(("graph-tag-dot", i))
                        .size(px(10.))
                        .rounded_full()
                        .bg(colour)
                        .cursor_pointer()
                        .border_1()
                        .border_color(if colouring { theme.foreground } else { colour })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.tag_colouring =
                                if this.tag_colouring.as_deref() == Some(dot_tag.as_str()) {
                                    None
                                } else {
                                    Some(dot_tag.clone())
                                };
                            cx.notify();
                        })),
                    "Choose this tag's colour",
                ))
                .child(
                    div()
                        .id(("graph-tag-name", i))
                        .text_color(theme.foreground)
                        .when(partial, |d| d.cursor_pointer())
                        .child(SharedString::from(tag.clone()))
                        .when(partial, |d| {
                            d.on_click(cx.listener(move |this, _, _, cx| {
                                this.change_tags(std::slice::from_ref(&add_tag), &[], cx)
                            }))
                        }),
                )
                .when(partial, |d| {
                    d.child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(format!("{n} of {total}"))),
                    )
                })
                .child(hints::tip(
                    div()
                        .id(("graph-tag-drop", i))
                        .px_1()
                        .text_color(theme.muted_foreground)
                        .cursor_pointer()
                        .child("×")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.change_tags(&[], std::slice::from_ref(&drop_tag), cx)
                        })),
                    if partial {
                        "Take it off where it is"
                    } else {
                        "Take it off"
                    },
                ));
            chip.into_any_element()
        });

        // The tags of the case that fit what is typed and are not on everything yet.
        let typed = self.tag.read(cx).value().trim().to_lowercase();
        let case_tags = self.case_tags();
        let offered: Vec<(String, usize)> = if self.tag_adding {
            let mut found: Vec<(String, usize)> = case_tags
                .iter()
                .filter(|(t, _)| tags.get(*t).is_none_or(|n| *n < total))
                .filter(|(t, _)| typed.is_empty() || t.contains(&typed))
                .map(|(t, n)| (t.clone(), *n))
                .collect();
            found.sort_by(|a, b| {
                (!a.0.starts_with(&typed), std::cmp::Reverse(a.1), &a.0).cmp(&(
                    !b.0.starts_with(&typed),
                    std::cmp::Reverse(b.1),
                    &b.0,
                ))
            });
            found.truncate(6);
            found
        } else {
            Vec::new()
        };
        let is_new =
            !typed.is_empty() && !case_tags.contains_key(&typed) && !tags.contains_key(&typed);
        let suggestions = offered.into_iter().enumerate().map(|(i, (tag, n))| {
            let colour = look::tag_colour(&self.vocabulary, &tag, dark);
            let name = tag.clone();
            h_flex()
                .id(("graph-tag-offer", i))
                .gap_2()
                .items_center()
                .px_2()
                .py_0p5()
                .rounded_md()
                .cursor_pointer()
                .text_size(small)
                .hover(|d| d.bg(theme.secondary))
                .child(div().size(px(8.)).rounded_full().bg(colour))
                .child(
                    div()
                        .flex_1()
                        .font_family(theme.mono_font_family.clone())
                        .child(SharedString::from(tag)),
                )
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(match n {
                            1 => "1 node".to_string(),
                            n => format!("{n} nodes"),
                        })),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.change_tags(std::slice::from_ref(&name), &[], cx);
                    this.tag.update(cx, |s, cx| s.set_value("", window, cx));
                }))
        });

        let swatches = self.tag_colouring.clone().map(|tag| {
            let given = self.vocabulary.tag_colour(&tag).map(str::to_string);
            let reset_tag = tag.clone();
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .text_size(small)
                        .text_color(theme.muted_foreground)
                        .child("Colour of")
                        .child(
                            div()
                                .font_family(theme.mono_font_family.clone())
                                .text_color(theme.foreground)
                                .child(SharedString::from(tag.clone())),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("graph-tag-colour-reset")
                                .small()
                                .ghost()
                                .label("Default")
                                .tooltip(
                                    "The colour its name gives it, which is how its box is drawn",
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_tag_colour(&reset_tag, None, cx)
                                })),
                        ),
                )
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .children(look::PALETTE.iter().enumerate().map(|(i, (name, _))| {
                            let colour = look::colour(name).unwrap_or_default();
                            let picked = given.as_deref() == Some(*name);
                            let tag = tag.clone();
                            hints::tip(
                                div()
                                    .id(("graph-tag-colour", i))
                                    .size(px(18.))
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
                                        this.set_tag_colour(&tag, Some(name), cx)
                                    })),
                                *name,
                            )
                        })),
                )
        });

        v_flex()
            .flex_none()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .justify_between()
                    .items_baseline()
                    .text_size(small)
                    .text_color(theme.muted_foreground)
                    .child("Tags")
                    .when(total > 1, |d| {
                        d.child(SharedString::from(format!("changes apply to all {total}")))
                    }),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .items_center()
                    .children(chips)
                    .when(!self.tag_adding, |d| {
                        d.child(
                            Button::new("graph-tag-add")
                                .small()
                                .ghost()
                                .label("+ tag")
                                .tooltip(if total > 1 {
                                    "Put a tag on every selected node"
                                } else {
                                    "Add a tag, e.g. phase.recon: the same tag on several nodes boxes them together"
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.tag_adding = true;
                                    this.tag.update(cx, |s, cx| s.focus(window, cx));
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .when(self.tag_adding, |d| {
                d.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().flex_1().child(Input::new(&self.tag).small()))
                        .child(
                            Button::new("graph-tag-done")
                                .small()
                                .ghost()
                                .label("Done")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.tag_adding = false;
                                    window.focus(&this.focus, cx);
                                    cx.notify();
                                })),
                        ),
                )
                .child(v_flex().children(suggestions))
                .when(is_new, |d| {
                    let colour = look::tag_colour(&self.vocabulary, &typed, dark);
                    d.child(
                        h_flex()
                            .id("graph-tag-new")
                            .gap_2()
                            .items_center()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .cursor_pointer()
                            .text_size(small)
                            .hover(|d| d.bg(theme.secondary))
                            .child(div().size(px(8.)).rounded_full().bg(colour))
                            .child(SharedString::from(format!("New tag {typed}")))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_typed_tag(window, cx)
                            })),
                    )
                })
            })
            .children(swatches)
            .into_any_element()
    }
}
