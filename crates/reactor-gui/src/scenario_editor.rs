//! The scenario editor: a window of its own for writing a scenario -- its id, where it is kept,
//! what it is for, and its phases, each with a title, a toolset and a briefing
//! ([ADR-0065](../../../docs/adr/0065-a-scenario-is-general-starts-from-a-question-and-can-be-written-for-the-case.md)).
//! Opened from the Scenario chip (New, Edit), from a scenario the agent saved, and by
//! `/reactor-scenario new` and `/reactor-scenario edit <id>`.
//!
//! Saving writes the same files `scenario_save` does. A scenario opened from one place and
//! saved to another is a copy there: a shipped one saved for the case shadows it in this case
//! only.

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Root, Sizable as _,
};
use gpui_kit::prelude::*;
use gpui_kit::{App, Entity, SharedString, WeakEntity, Window, div, px, size};
use reactor_core::scenarios::{Phase, Scope};

use crate::app::ReactorApp;
use crate::hints;

/// What the editor opens on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    New,
    /// The scenario of this id, as the lookup finds it.
    Edit(String),
}

/// Open the editor window.
pub fn open(cx: &mut App, app: Entity<ReactorApp>, start: Start) {
    let lib = app.read(cx).scenario_library();
    let toolsets = app.read(cx).toolset_ids();
    // What the fields start with, and where the scenario was found.
    let (id, found, phases) = match &start {
        Start::New => (String::new(), None, vec![]),
        Start::Edit(id) => match lib.as_ref().and_then(|l| l.find(id)) {
            Some((scope, _)) => (
                id.clone(),
                Some(scope),
                lib.as_ref().map(|l| l.load(id)).unwrap_or_default(),
            ),
            None => (id.clone(), None, vec![]),
        },
    };
    let bounds = gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds::centered(
        None,
        size(px(980.), px(720.)),
        cx,
    ));
    let opened = cx.open_window(crate::chrome::window_options(bounds), move |window, cx| {
        crate::theme::install_ayu_dark(cx);
        let view = cx.new(|cx| Editor::new(app, id, found, phases, toolsets, window, cx));
        let root: gpui_kit::AnyView = view.into();
        cx.new(|cx| Root::new(root, window, cx))
    });
    if opened.is_ok() {
        cx.activate(true);
    }
}

struct Row {
    title: Entity<InputState>,
    toolset: Option<String>,
    body: Entity<TextareaState>,
}

struct Editor {
    app: Entity<ReactorApp>,
    /// Where the scenario being edited was found; `None` for a new one.
    found: Option<Scope>,
    /// The id it was opened under.
    opened_as: String,
    id: Entity<InputState>,
    scope: Scope,
    description: Entity<InputState>,
    rows: Vec<Row>,
    selected: usize,
    toolsets: Vec<String>,
    error: Option<String>,
}

fn row(phase: &Phase, window: &mut Window, cx: &mut Context<Editor>) -> Row {
    let title = cx.new(|cx| {
        let mut s = InputState::new(window, cx).placeholder("e.g. Triage");
        s.set_value(&phase.title, window, cx);
        s
    });
    let body = cx.new(|cx| {
        let mut s = TextareaState::new(window, cx).placeholder(
            "**Goal:** …\n\n**Applies when:** …\n\n**Activities:**\n- …\n\nCall `reactor_phase_complete` once …",
        );
        s.set_value(&phase.body, window, cx);
        s
    });
    Row {
        title,
        toolset: phase.toolset.clone(),
        body,
    }
}

impl Editor {
    fn new(
        app: Entity<ReactorApp>,
        id: String,
        found: Option<Scope>,
        phases: Vec<Phase>,
        toolsets: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let id_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("id, e.g. vpn-intrusion");
            s.set_value(&id, window, cx);
            s
        });
        let description = phases
            .first()
            .and_then(|p| p.description.clone())
            .unwrap_or_default();
        let description = cx.new(|cx| {
            let mut s = InputState::new(window, cx)
                .placeholder("One line: what it is for and when to use it");
            s.set_value(&description, window, cx);
            s
        });
        let mut rows: Vec<Row> = phases.iter().map(|p| row(p, window, cx)).collect();
        if rows.is_empty() {
            rows.push(row(&Phase::default(), window, cx));
        }
        let focus = id_input.clone();
        window.defer(cx, move |window, cx| {
            focus.update(cx, |s, cx| s.focus(window, cx));
        });
        Editor {
            app,
            // A shipped scenario can only be saved as the user's: globally by default.
            scope: match found {
                Some(Scope::Case) => Scope::Case,
                Some(_) => Scope::Global,
                None => Scope::Case,
            },
            found,
            opened_as: id,
            id: id_input,
            description,
            rows,
            selected: 0,
            toolsets,
            error: None,
        }
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let at = (self.selected + 1).min(self.rows.len());
        let r = row(&Phase::default(), window, cx);
        self.rows.insert(at, r);
        self.selected = at;
        cx.notify();
    }

    fn remove(&mut self, cx: &mut Context<Self>) {
        if self.rows.len() > 1 {
            self.rows.remove(self.selected);
            self.selected = self.selected.min(self.rows.len() - 1);
            cx.notify();
        }
    }

    fn shift(&mut self, up: bool, cx: &mut Context<Self>) {
        let i = self.selected;
        let j = if up { i.checked_sub(1) } else { Some(i + 1) };
        if let Some(j) = j.filter(|&j| j < self.rows.len()) {
            self.rows.swap(i, j);
            self.selected = j;
            cx.notify();
        }
    }

    fn phases(&self, cx: &App) -> Vec<Phase> {
        self.rows
            .iter()
            .map(|r| Phase {
                title: r.title.read(cx).value().trim().to_string(),
                toolset: r.toolset.clone(),
                description: None,
                body: r.body.read(cx).value().to_string(),
            })
            .collect()
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.id.read(cx).value().trim().to_string();
        let description = self.description.read(cx).value().trim().to_string();
        let phases = self.phases(cx);
        let scope = self.scope;
        let result = self.app.update(cx, |app, cx| {
            app.scenario_save(scope, &id, &description, &phases, cx)
        });
        match result {
            Ok(()) => window.remove_window(),
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }

    fn promote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.opened_as.clone();
        self.app.update(cx, |app, cx| app.scenario_promote(&id, cx));
        window.remove_window();
    }

    fn toolset_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.rows[self.selected].toolset.clone();
        let (toolsets, weak): (Vec<String>, WeakEntity<Editor>) =
            (self.toolsets.clone(), cx.weak_entity());
        Button::new("scenario-editor-toolset")
            .label(current.clone().unwrap_or_else(|| "no toolset".into()))
            .small()
            .tooltip("A toolset to enable when the phase starts: advisory, nothing is turned off")
            .dropdown_menu(move |mut menu, _window, _cx| {
                let pick = |t: Option<String>| {
                    let weak = weak.clone();
                    move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                        let _ = weak.update(cx, |ed, cx| {
                            let i = ed.selected;
                            ed.rows[i].toolset = t.clone();
                            cx.notify();
                        });
                    }
                };
                menu = menu.item(PopupMenuItem::new("No toolset").on_click(pick(None)));
                for t in &toolsets {
                    menu = menu.item(PopupMenuItem::new(t.clone()).on_click(pick(Some(t.clone()))));
                }
                menu
            })
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let body_height = (f32::from(window.viewport_size().height) - 330.).max(180.);
        let heading = match self.found {
            Some(scope) => format!("Scenario: {} ({})", self.opened_as, scope.as_str()),
            None => "A new scenario".to_string(),
        };
        let scope_button = |scope: Scope, label: &'static str, tip: &'static str| {
            Button::new(SharedString::from(format!(
                "scenario-editor-scope-{}",
                scope.as_str()
            )))
            .label(label)
            .small()
            .when(self.scope == scope, |b| b.primary())
            .when(self.scope != scope, |b| b.ghost())
            .tooltip(tip)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.scope = scope;
                cx.notify();
            }))
        };
        let mut list = v_flex().gap_0p5();
        for (i, r) in self.rows.iter().enumerate() {
            let title = r.title.read(cx).value().to_string();
            list = list.child(
                h_flex()
                    .id(("scenario-editor-phase", i))
                    .gap_2()
                    .px_2()
                    .h(px(26.))
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .when(i == self.selected, |r| {
                        r.bg(theme.list_active)
                            .border_1()
                            .border_color(theme.list_active_border)
                    })
                    .hover(|s| s.bg(theme.list_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = i;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(18.))
                            .font_family(theme.mono_font_family.clone())
                            .text_color(muted)
                            .child(format!("{}", i + 1)),
                    )
                    .child(
                        div()
                            .truncate()
                            .child(SharedString::from(if title.is_empty() {
                                "(untitled)".to_string()
                            } else {
                                title
                            })),
                    ),
            );
        }
        let n = self.rows.len();
        let selected = &self.rows[self.selected];
        let can_promote =
            self.found == Some(Scope::Case) && self.id.read(cx).value().trim() == self.opened_as;
        let same_id = self.id.read(cx).value().trim() == self.opened_as;
        let copy_note = match (self.found.filter(|_| same_id), self.scope) {
            (Some(Scope::Shipped), Scope::Global) => {
                Some("Saved globally, this becomes your copy of the shipped scenario.")
            }
            (Some(from), to) if from != to && to == Scope::Case => {
                Some("Saved for this case, it is a copy that wins over the other one here.")
            }
            (Some(Scope::Case), Scope::Global) => Some(
                "Saved globally as it is. Promote instead to have the agent take the case's specifics out first.",
            ),
            _ => None,
        };
        v_flex()
            .size_full()
            .child(crate::chrome::title_bar("REactor — Scenario", None, None, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(div().text_color(theme.foreground).child(heading))
                            .child(hints::info_button(
                                "scenario-editor-info",
                                "A scenario is phases the agent is briefed on one at a time; it moves on with reactor_phase_complete and a summary. Write each briefing as a method — what to do, when it applies, when it is done — not as this case's file layout: the question the scenario is started with carries the specifics. A case scenario is kept in the case and wins over a global one of the same id.",
                            )),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().w(px(84.)).text_color(muted).child("Id"))
                            .child(div().w(px(260.)).child(Input::new(&self.id)))
                            .child(div().w(px(12.)))
                            .child(div().text_color(muted).child("Kept"))
                            .child(scope_button(Scope::Case, "This case", "In the case: only this case sees it, and it wins over a global one"))
                            .child(scope_button(Scope::Global, "Every case", "In the config directory, for every case")),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().w(px(84.)).text_color(muted).child("Description"))
                            .child(div().flex_1().child(Input::new(&self.description))),
                    )
                    .children(copy_note.map(|t| {
                        div()
                            .text_color(muted)
                            .text_size(theme.font_size * 0.85)
                            .child(t)
                    }))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_h_0()
                            .gap_4()
                            .items_start()
                            .child(
                                v_flex()
                                    .w(px(240.))
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("scenario-editor-phases")
                                            .max_h(px(body_height + 70.))
                                            .overflow_y_scrollbar()
                                            .child(list),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(Button::new("scenario-editor-add").label("Add").small().tooltip("A new phase after the selected one").on_click(cx.listener(|this, _, window, cx| this.add(window, cx))))
                                            .child(Button::new("scenario-editor-up").label("Up").small().ghost().disabled(self.selected == 0).on_click(cx.listener(|this, _, _, cx| this.shift(true, cx))))
                                            .child(Button::new("scenario-editor-down").label("Down").small().ghost().disabled(self.selected + 1 >= n).on_click(cx.listener(|this, _, _, cx| this.shift(false, cx))))
                                            .child(Button::new("scenario-editor-remove").label("Remove").small().ghost().disabled(n <= 1).on_click(cx.listener(|this, _, _, cx| this.remove(cx)))),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_2()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(div().text_color(muted).child(format!("Phase {}", self.selected + 1)))
                                            .child(div().flex_1().child(Input::new(&selected.title)))
                                            .child(self.toolset_menu(cx)),
                                    )
                                    .child(Textarea::new(&selected.body).h(px(body_height))),
                            ),
                    )
                    .children(self.error.clone().map(|e| {
                        div().text_color(theme.danger).child(SharedString::from(e))
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .p_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .when(can_promote, |el| {
                        el.child(
                            Button::new("scenario-editor-promote")
                                .label("Promote to global…")
                                .tooltip("Ask the agent to rewrite it without this case's names, paths and indicators, and save it for every case. The case copy stays")
                                .on_click(cx.listener(|this, _, window, cx| this.promote(window, cx))),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("scenario-editor-cancel")
                            .label("Cancel")
                            .ghost()
                            .on_click(|_, window, _| window.remove_window()),
                    )
                    .child(
                        Button::new("scenario-editor-save")
                            .label("Save")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}
