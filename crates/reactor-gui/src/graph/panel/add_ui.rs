//! Adding a node by hand: what to add is chosen from one list, then it is named.
//!
//! The list is every type there is -- the vocabulary's and the case's, whatever their kind --
//! and the person's own words for an annotation (note, hypothesis, assessment, cluster, same
//! as, searched). Typing narrows it; the type's kind comes with it, so there is no kind to pick
//! first. A type that is not in the list is offered as a new one, with the kind it is to be of.
//!
//! It opens two ways: as a palette on Ctrl/Cmd+N (and the toolbar's +), and, small, where the
//! plane was right-clicked, with the node showing there. What goes beyond a name -- text,
//! confidence, tags -- is for the inspector, which opens on the new node.

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::{Sizable as _, Theme};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Div, Focusable as _, MouseButton, SharedString, Stateful, Window, div, px,
};
use reactor_db::graph::Kind;
use reactor_db::graph::edit::{self, AnnotationKind};

use super::node_ui::icon;
use super::{AddMode, Form, GraphPanel, Stance};
use crate::graph::look;
use crate::hints;

/// How wide the palette is, in pixels: what it is centred by.
pub(super) const PALETTE_WIDTH: f32 = 580.;

/// The kinds a type that is not known yet can be made a type of.
pub(super) const NEW_KINDS: [(Kind, &str); 4] = [
    (Kind::Entity, "Entity"),
    (Kind::Event, "Event"),
    (Kind::Observation, "Observation"),
    (Kind::Annotation, "Annotation"),
];

/// One line of the list: something that can be added.
#[derive(Debug, Clone)]
pub(super) struct AddRow {
    /// The type it is (what the node will be of); for an external, nothing.
    pub type_: String,
    /// What the list calls it.
    pub label: String,
    pub kind: Kind,
    pub mode: AddMode,
    pub definition: String,
    /// How many nodes of the case have it.
    pub uses: usize,
    /// Not a type there is yet: it is made by being used.
    pub new: bool,
}

fn annotation_of(type_: &str) -> Option<AnnotationKind> {
    AnnotationKind::ALL
        .into_iter()
        .find(|k| k.type_name() == type_)
}

fn annotation_label(kind: AnnotationKind) -> &'static str {
    match kind {
        AnnotationKind::Hypothesis => "Hypothesis",
        AnnotationKind::Judgement => "Assessment",
        AnnotationKind::Note => "Note",
        AnnotationKind::Cluster => "Cluster",
        AnnotationKind::SameAs => "Same as",
        AnnotationKind::SearchNegative => "Searched",
    }
}

fn annotation_definition(kind: AnnotationKind) -> &'static str {
    match kind {
        AnnotationKind::Hypothesis => "A candidate explanation, open until supported or rejected.",
        AnnotationKind::Judgement => {
            "What you conclude about what is selected; with a hypothesis, it supports or refutes it."
        }
        AnnotationKind::Note => "Free text, put where it was thought of.",
        AnnotationKind::Cluster => "The selected nodes belong together.",
        AnnotationKind::SameAs => "Two selected entities are one thing.",
        AnnotationKind::SearchNegative => "A search that found nothing, and where it looked.",
    }
}

/// Whether `typed` can be a type's name: lowercase words joined by dots.
fn is_type_name(typed: &str) -> bool {
    !typed.is_empty()
        && typed.split('.').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

fn kind_word(kind: Kind) -> &'static str {
    match kind {
        Kind::Entity => "entity",
        Kind::Event => "event",
        Kind::Observation => "observation",
        Kind::Annotation => "annotation",
        Kind::External => "external",
    }
}

impl GraphPanel {
    /// What a type is the kind of: the vocabulary's word, else what the case made of it.
    pub(super) fn kind_of_type(&self, type_: &str) -> Kind {
        self.vocabulary
            .types
            .get(type_)
            .and_then(|t| t.kind)
            .or_else(|| {
                self.model
                    .nodes
                    .iter()
                    .find(|n| n.type_ == type_)
                    .map(|n| n.element)
            })
            .unwrap_or(Kind::Entity)
    }

    /// The list for what is typed now, best first, and the new type it could be at its end.
    pub(super) fn rebuild_add_rows(&mut self, cx: &App) {
        let typed = self
            .entity_type
            .read(cx)
            .value()
            .trim()
            .to_lowercase()
            .replace(' ', "_");
        let mut rows: Vec<AddRow> = Vec::new();
        let uses_of = |name: &str| {
            self.add_choices
                .iter()
                .find(|c| c.name == name)
                .map(|c| c.uses)
                .unwrap_or(0)
        };
        for name in edit::find_types(&self.add_choices, &typed, 24) {
            let kind = self.kind_of_type(&name);
            if kind == Kind::External {
                continue;
            }
            let (mode, label, definition) = match annotation_of(&name) {
                Some(AnnotationKind::Note) => (
                    AddMode::Note,
                    "Note".to_string(),
                    annotation_definition(AnnotationKind::Note).to_string(),
                ),
                Some(k) => (
                    AddMode::Annotation(k),
                    annotation_label(k).to_string(),
                    annotation_definition(k).to_string(),
                ),
                None => (
                    AddMode::Node(kind),
                    name.clone(),
                    self.vocabulary
                        .types
                        .get(&name)
                        .map(|t| t.definition.clone())
                        .unwrap_or_default(),
                ),
            };
            rows.push(AddRow {
                uses: uses_of(&name),
                type_: name,
                label,
                kind,
                mode,
                definition,
                new: false,
            });
        }
        // The words for an annotation, found by what they are called as well as by their type.
        for k in AnnotationKind::ALL {
            let label = annotation_label(k);
            let present = rows.iter().any(|r| r.type_ == k.type_name());
            if !present && !typed.is_empty() && label.to_lowercase().starts_with(&typed) {
                rows.push(AddRow {
                    type_: k.type_name().to_string(),
                    label: label.to_string(),
                    kind: Kind::Annotation,
                    mode: if k == AnnotationKind::Note {
                        AddMode::Note
                    } else {
                        AddMode::Annotation(k)
                    },
                    definition: annotation_definition(k).to_string(),
                    uses: uses_of(k.type_name()),
                    new: false,
                });
            }
        }
        // A page, a file: an external is its address, and has no type to choose.
        let external = ["external", "link", "file", "url", "path", "http"];
        if typed.is_empty() || external.iter().any(|w| w.starts_with(&typed)) {
            rows.push(AddRow {
                type_: String::new(),
                label: "External: a link or a file".to_string(),
                kind: Kind::External,
                mode: AddMode::Node(Kind::External),
                definition:
                    "A page, a file or another place outside the case; its name is its address."
                        .to_string(),
                uses: 0,
                new: false,
            });
        }
        let known = rows.iter().any(|r| r.type_ == typed);
        if is_type_name(&typed) && !known {
            rows.push(AddRow {
                type_: typed.clone(),
                label: typed,
                kind: self.add_new_kind,
                mode: AddMode::Node(self.add_new_kind),
                definition: "Not in the vocabulary yet: it is made by being used.".to_string(),
                uses: 0,
                new: true,
            });
        }
        self.add_rows = rows;
        // Typing starts the list over; picking a kind for a new type keeps the place.
        self.add_at = self.add_at.min(self.add_rows.len().saturating_sub(1));
    }

    /// The row the keys and Enter are on.
    pub(super) fn add_row(&self) -> Option<&AddRow> {
        self.add_rows.get(self.add_at)
    }

    /// ↑ and ↓ in the type field.
    pub(super) fn add_move(&mut self, down: bool, cx: &mut Context<Self>) {
        let n = self.add_rows.len();
        if n == 0 {
            return;
        }
        self.add_at = if down {
            (self.add_at + 1) % n
        } else {
            (self.add_at + n - 1) % n
        };
        cx.notify();
    }

    /// The label of the field that names what is added.
    fn add_name_label(row: &AddRow) -> &'static str {
        match row.mode {
            AddMode::Note => "Note",
            AddMode::Annotation(AnnotationKind::Hypothesis) => "Hypothesis",
            AddMode::Annotation(AnnotationKind::Judgement) => "Assessment",
            AddMode::Annotation(AnnotationKind::Cluster) => "Name",
            AddMode::Annotation(AnnotationKind::SameAs) => "Why",
            AddMode::Annotation(AnnotationKind::SearchNegative) => "Looked for",
            AddMode::Annotation(AnnotationKind::Note) => "Note",
            AddMode::Node(Kind::External) => "Address",
            AddMode::Node(Kind::Entity) => "Name",
            AddMode::Node(Kind::Event) | AddMode::Node(Kind::Observation) => "What",
            AddMode::Node(Kind::Annotation) => "Text",
        }
    }

    /// The add form: the palette, or its small form beside the pointer.
    pub(super) fn add_card(
        &self,
        compact: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let f = f32::from(theme.font_size);
        let small = theme.font_size * 0.85;
        let dark = theme.is_dark();
        let width = if compact { 440. } else { PALETTE_WIDTH };
        let picked = self.add_row().cloned();

        let rows = self.add_rows.iter().enumerate().map(|(i, r)| {
            let look = look::look(&self.vocabulary, &r.type_, r.kind);
            let hue = look::ink(look.colour, dark);
            let on = i == self.add_at;
            let (kind_label, new) = (kind_word(r.kind), r.new);
            let row = h_flex()
                .id(("graph-add-row", i))
                .gap_2()
                .items_center()
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .when(on, |d| d.bg(theme.secondary))
                .child(icon(look.path.clone(), f * 1.15, hue))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_color(theme.foreground).child(SharedString::from(
                            if new {
                                format!("New type {}", r.label)
                            } else {
                                r.label.clone()
                            },
                        )))
                        .when(!compact && !r.definition.is_empty(), |c| {
                            c.child(
                                div()
                                    .text_size(small)
                                    .text_color(theme.muted_foreground)
                                    .child(SharedString::from(r.definition.clone())),
                            )
                        }),
                )
                .when(r.uses > 0 && !compact, |d| {
                    d.child(
                        div()
                            .text_size(small)
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(format!("{}×", r.uses))),
                    )
                })
                .child(
                    div()
                        .text_size(small)
                        .text_color(theme.muted_foreground)
                        .child(kind_label),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.add_at = i;
                    this.entity_value.update(cx, |s, cx| s.focus(window, cx));
                    cx.notify();
                }));
            // A new type says what it is a type of, here and now.
            if new && on {
                v_flex()
                    .child(row)
                    .child(h_flex().gap_1().pl_8().pb_1().children(
                        NEW_KINDS.iter().enumerate().map(|(k, (kind, label))| {
                            let kind = *kind;
                            let b = Button::new(("graph-add-new-kind", k)).small().label(*label);
                            let b = if self.add_new_kind == kind {
                                b.primary()
                            } else {
                                b.ghost()
                            };
                            b.on_click(cx.listener(move |this, _, _, cx| {
                                this.add_new_kind = kind;
                                this.rebuild_add_rows(cx);
                                cx.notify();
                            }))
                        }),
                    ))
                    .into_any_element()
            } else {
                row.into_any_element()
            }
        });

        // What it will be about: the selection, by name (an annotation says something of it).
        let selection = self.selected_nodes();
        let about = if selection.is_empty() {
            "nothing selected".to_string()
        } else {
            selection
                .iter()
                .map(|id| {
                    self.model
                        .nodes
                        .iter()
                        .find(|n| n.id == *id)
                        .map(|n| n.label.clone())
                        .unwrap_or_else(|| id.clone())
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let hypotheses = selection
            .iter()
            .filter(|id| {
                self.model
                    .nodes
                    .iter()
                    .any(|n| n.id == **id && n.type_ == "analysis.hypothesis")
            })
            .count();
        let anchored = matches!(
            picked.as_ref().map(|r| r.mode),
            Some(AddMode::Annotation(
                AnnotationKind::Judgement | AnnotationKind::Cluster | AnnotationKind::SameAs
            ))
        );
        let can_stance = matches!(
            picked.as_ref().map(|r| r.mode),
            Some(AddMode::Annotation(AnnotationKind::Judgement))
        ) && hypotheses > 0
            && hypotheses < selection.len();
        let stance_button = |id: &'static str, label: &'static str, this_stance: Stance| {
            let b = Button::new(id).small().label(label);
            let b = if self.stance == this_stance {
                b.primary()
            } else {
                b.ghost()
            };
            b.on_click(cx.listener(move |this, _, _, cx| {
                this.stance = this_stance;
                cx.notify();
            }))
        };
        let field =
            |label: &'static str,
             input: &gpui_kit::Entity<gpui_kit::component::input::InputState>| {
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(84.))
                            .text_color(theme.muted_foreground)
                            .child(label),
                    )
                    .child(div().flex_1().child(Input::new(input).small()))
            };

        let name_label = picked.as_ref().map(Self::add_name_label).unwrap_or("Name");
        let searching = matches!(
            picked.as_ref().map(|r| r.mode),
            Some(AddMode::Annotation(AnnotationKind::SearchNegative))
        );
        let preview = picked.as_ref().map(|r| {
            let look = look::look(&self.vocabulary, &r.type_, r.kind);
            (
                look.path.clone(),
                look::ink(look.colour, dark),
                r.type_.clone(),
            )
        });

        v_flex()
            .id("graph-form")
            .gap_2()
            .p_2()
            .w(px(width))
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_size(theme.font_size * 0.92)
            .occlude()
            // A click on the card's own ground leaves no field focused, and Esc is only the
            // plane's while something in it is: the plane takes the focus back then.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    let in_a_field = [&this.entity_type, &this.entity_value, &this.add_where]
                        .iter()
                        .any(|f| f.read(cx).focus_handle(cx).is_focused(window));
                    if !in_a_field {
                        window.focus(&this.focus, cx);
                    }
                }),
            )
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            // A click anywhere else puts it away (this runs whatever else takes the click).
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                if matches!(this.form, Some(Form::Add { .. })) {
                    this.form = None;
                    window.focus(&this.focus, cx);
                    cx.notify();
                }
            }))
            .when(!compact, |c| {
                c.child(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .px_1()
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .child(div().text_color(theme.foreground).child("Add to the graph"))
                                .child(hints::info_button(
                                    "graph-form-info",
                                    "Pick what it is from the list: a type of entity, of event, of observation or of annotation, or your own words for an annotation (note, hypothesis, assessment, cluster, same as, searched). Name it and it is added and opened, so that its text, confidence and tags can be added there. A type that is not in the list is made by using it.",
                                )),
                        )
                        .child(
                            div()
                                .text_size(small)
                                .text_color(theme.muted_foreground)
                                .child(if cfg!(target_os = "macos") { "⌘N" } else { "Ctrl+N" }),
                        ),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(84.))
                            .text_color(theme.muted_foreground)
                            .child(if compact { "Add" } else { "Find" }),
                    )
                    .child(div().flex_1().child(Input::new(&self.entity_type).small())),
            )
            .child(
                v_flex()
                    .id("graph-add-list")
                    .max_h(px(if compact { 200. } else { 300. }))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .when(self.add_rows.is_empty(), |c| {
                c.child(
                    div()
                        .px_2()
                        .text_color(theme.muted_foreground)
                        .child("Nothing fits. Lowercase words joined by dots make a new type."),
                )
            })
            .child(field(name_label, &self.entity_value))
            .when(searching, |c| c.child(field("Where", &self.add_where)))
            .when(anchored, |c| {
                c.child(
                    div()
                        .pl(px(92.))
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(format!("About: {about}"))),
                )
            })
            .when(can_stance, |c| {
                c.child(
                    h_flex()
                        .gap_1()
                        .pl(px(92.))
                        .child(stance_button("graph-stance-plain", "Comments on", Stance::Plain))
                        .child(stance_button("graph-stance-supports", "Supports", Stance::Supports))
                        .child(stance_button("graph-stance-refutes", "Refutes", Stance::Refutes)),
                )
            })
            .child(
                div()
                    .w_full()
                    .text_size(small)
                    .text_color(theme.muted_foreground)
                    .child(if compact {
                        "↑↓ pick · ↵ name it · ↵ adds"
                    } else {
                        "↑↓ pick a type · ↵ then name it · ↵ adds and opens it · Esc closes"
                    }),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .justify_end()
                    .when_some(preview, |d, (path, hue, type_)| {
                        d.child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .items_center()
                                .text_size(small)
                                .text_color(theme.muted_foreground)
                                .child(icon(path, f, hue))
                                .child(SharedString::from(type_)),
                        )
                    })
                    .child(
                        Button::new("graph-form-ok")
                            .small()
                            .primary()
                            .label("Add")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_form(window, cx)
                            })),
                    )
                    .child(
                        Button::new("graph-form-cancel")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.form = None;
                                cx.notify();
                            })),
                    ),
            )
    }

    /// Enter in the type field: the highlighted row is the type, and the name comes next. With
    /// Ctrl/Cmd it adds at once.
    pub(super) fn add_choose(
        &mut self,
        secondary: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if secondary {
            self.submit_form(window, cx);
        } else if self.add_row().is_some() {
            self.entity_value.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }
}
