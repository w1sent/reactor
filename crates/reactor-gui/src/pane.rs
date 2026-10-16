//! What a transcript pane shows of its session around the conversation.
//!
//! Above the transcript: the usage bar (its details open in place) and, while the manifest
//! is on, the goal line. On the right edge, while a scenario runs, its steps as numbers.
//! Under the prompt, the chips: what is switched on for this session, each a chip, and what
//! is off behind *Activate*.
//!
//! Every pane shows its own session's ([`crate::app::PaneView`]), so two sessions side by
//! side each say where they stand. A click acts on the pane's session: it is made the active
//! one first ([`on_session`]), because the app's actions are the active session's.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::label::Label;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::stepper::{Stepper, StepperItem};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, Theme};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{SharedString, Window, div, px};
use reactor_agent::entry::Mode;
use reactor_context::settings::{ContextSettings, MemorySettings, Origin};

use crate::app::{PaneView, ReactorApp};
use crate::backend::ContextView;
use crate::panels::tokens_short;
use crate::scenario_ui::{Outcome, Run};

type Weak = gpui_kit::WeakEntity<ReactorApp>;

/// Something the pane itself changes (open the details, select a step).
pub type Local = Rc<dyn Fn(&mut Window, &mut App)>;

/// Select a scenario step, or none.
pub type SelectStep = Rc<dyn Fn(Option<usize>, &mut Window, &mut App)>;

/// Turn something of the pane's own on or off.
pub type Flip = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// A click that acts on session `id`: it is made the active one, then `f` runs.
fn on_session(
    app: &Weak,
    id: &str,
    f: impl Fn(&mut ReactorApp, &mut Window, &mut Context<ReactorApp>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let (app, id) = (app.clone(), id.to_string());
    move |_, window, cx| {
        let _ = app.update(cx, |app, cx| {
            app.activate_session(&id, window, cx);
            f(app, window, cx);
        });
    }
}

fn small(theme: &Theme) -> Pixels {
    theme.font_size * 0.85
}

// -- the usage bar ------------------------------------------------------------------------------

/// The usage bar by kind, with the mark where a reduction starts. Colours are
/// [`crate::inspect_ui::kind_colour`]'s, so bar, details and context window agree.
fn usage_bar(view: &ContextView, theme: &Theme) -> impl IntoElement {
    let hard = view.hard.max(1) as f32;
    let counted: u64 = view.usage.iter().map(|(_, t)| *t).sum();
    // The kinds are estimates of the same request `used` measures: if they come to more than
    // the window, they are drawn to scale rather than past the end of the bar.
    let scale = if counted as f32 > hard {
        hard / counted as f32
    } else {
        1.0
    };
    let trigger_at = (view.trigger as f32 / hard).min(1.0);
    let mut bar = h_flex()
        .relative()
        .h(px(8.))
        .gap(px(1.))
        .rounded_full()
        .overflow_hidden()
        .bg(theme.secondary);
    if view.usage.is_empty() {
        let fraction = (view.used as f32 / hard).min(1.0);
        bar = bar.child(
            div()
                .h_full()
                .w(relative(fraction))
                .bg(if view.used > view.trigger {
                    theme.warning
                } else {
                    theme.accent
                }),
        );
    }
    for (kind, tokens) in &view.usage {
        let share = (*tokens as f32 / hard * scale).max(0.0);
        bar = bar.child(crate::hints::tip(
            div()
                .id(SharedString::from(format!("usage-{kind:?}")))
                .h_full()
                .w(relative(share))
                .flex_none()
                .bg(crate::inspect_ui::kind_colour(*kind, theme)),
            format!("{}: ~{} tokens", kind.label(), tokens),
        ));
    }
    div().relative().child(bar).child(
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(2.))
            .left(relative(trigger_at))
            .bg(theme.foreground.opacity(0.6)),
    )
}

/// The first line of a pane: how full the next request is, the bar, and *Details*. Opened,
/// every kind with its size under it; a row shows that kind in the context window.
pub fn context_bar(
    app: &Weak,
    id: &str,
    view: Option<&ContextView>,
    open: bool,
    toggle: Local,
    theme: &Theme,
) -> AnyElement {
    use reactor_agent::inspect::Group;
    let muted = theme.muted_foreground;
    let Some(view) = view else {
        return h_flex()
            .h(px(32.))
            .px_3()
            .border_b_1()
            .border_color(theme.border)
            .text_size(small(theme))
            .text_color(muted)
            .child("measuring the context…")
            .into_any_element();
    };
    let (on_bar, on_button) = (toggle.clone(), toggle);
    let row = h_flex()
        .h(px(32.))
        .px_3()
        .gap_3()
        .items_center()
        .child(
            div()
                .flex_none()
                .text_size(small(theme))
                .text_color(muted)
                .child(format!(
                    "~{} of {} · {}%",
                    tokens_short(view.used),
                    tokens_short(view.hard),
                    view.percent()
                )),
        )
        .child(
            div()
                .id("usage-bar")
                .flex_1()
                .min_w(px(40.))
                .py_2()
                .cursor_pointer()
                .on_click(move |_, window, cx| on_bar(window, cx))
                .child(usage_bar(view, theme)),
        )
        .child(
            Button::new("usage-details")
                .label("Details")
                .icon(if open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .ghost()
                .small()
                .tooltip("What the request is made of, kind by kind")
                .on_click(move |_, window, cx| on_button(window, cx)),
        );
    let mut block = v_flex()
        .border_b_1()
        .border_color(theme.border)
        .when(open, |b| b.bg(theme.popover))
        .child(row);
    if open {
        let group_total = |group: Group| -> u64 {
            view.usage
                .iter()
                .filter(|(k, _)| k.group() == group)
                .map(|(_, t)| *t)
                .sum()
        };
        let mut legend = v_flex().gap_2().px_3().pb_2();
        for group in [Group::System, Group::Tools, Group::Messages] {
            let mut rows = v_flex().gap_0p5().child(
                h_flex()
                    .justify_between()
                    .text_color(muted)
                    .text_size(theme.font_size * 0.75)
                    .child(group.label().to_uppercase())
                    .child(tokens_short(group_total(group))),
            );
            let mut any = false;
            for (kind, tokens) in view.usage.iter().filter(|(k, _)| k.group() == group) {
                any = true;
                let kind = *kind;
                rows = rows.child(
                    h_flex()
                        .id(SharedString::from(format!("usage-row-{kind:?}")))
                        .gap_2()
                        .items_center()
                        .px_1()
                        .h(px(22.))
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.list_hover))
                        .on_click(on_session(app, id, move |app, window, cx| {
                            app.show_kind(kind, window, cx)
                        }))
                        .child(
                            div()
                                .size(px(10.))
                                .rounded_sm()
                                .flex_none()
                                .bg(crate::inspect_ui::kind_colour(kind, theme)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(theme.font_size * 0.9)
                                .child(kind.label()),
                        )
                        .child(
                            div()
                                .text_color(muted)
                                .text_size(small(theme))
                                .child(tokens_short(*tokens)),
                        ),
                );
            }
            if any {
                legend = legend.child(rows);
            }
        }
        block = block.child(
            legend.child(
                div()
                    .text_color(muted)
                    .text_size(theme.font_size * 0.75)
                    .child("A row opens that kind in the context window."),
            ),
        );
    }
    block.into_any_element()
}

// -- the goal line ------------------------------------------------------------------------------

/// Whether there is a manifest to show: it is switched on and has a goal or guidelines. With
/// neither there is nothing to send, so no goal line and no Goal chip; *Goal* is then under
/// Activate and opens the editor.
fn goal_shown(plan: &crate::plan_ui::PlanView) -> bool {
    plan.manifest_on && (plan.goal.is_some() || plan.guidelines.is_some())
}

/// The goal, while the manifest is on: one line, with the editor and the Frame as icon
/// buttons. Opened, the whole goal, the guidelines and the steps the agent keeps.
pub fn goal_line(
    app: &Weak,
    id: &str,
    plan: &crate::plan_ui::PlanView,
    open: bool,
    toggle: Local,
    theme: &Theme,
) -> Option<AnyElement> {
    if !goal_shown(plan) {
        return None;
    }
    let muted = theme.muted_foreground;
    let (on_text, on_chevron) = (toggle.clone(), toggle);
    let text = match (&plan.goal, open) {
        // Opened, the whole goal is below: the line keeps only its name.
        (Some(_), true) => div().flex_1(),
        (Some(g), false) => div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(SharedString::from(g.clone())),
        (None, _) => div().flex_1().text_color(muted).child("No goal, guidelines only"),
    };
    let row = h_flex()
        .h(px(32.))
        .pl_3()
        .pr_1()
        .gap_2()
        .items_center()
        .child(
            div()
                .flex_none()
                .text_size(theme.font_size * 0.75)
                .text_color(muted)
                .child("GOAL"),
        )
        .child(
            text.id("goal-text")
                .cursor_pointer()
                .on_click(move |_, window, cx| on_text(window, cx)),
        )
        .child(
            Button::new("goal-edit")
                .icon(IconName::Pencil)
                .ghost()
                .small()
                .tooltip("Write the goal and the guidelines")
                .on_click(on_session(app, id, |app, window, cx| {
                    app.open_goal_editor(window, cx)
                })),
        )
        .child(
            Button::new("goal-frame")
                .icon(IconName::Eye)
                .ghost()
                .small()
                .tooltip("Frame: what the model is told, and what is kept")
                .on_click(on_session(app, id, |app, window, cx| {
                    app.open_frame(window, cx)
                })),
        )
        .child(
            Button::new("goal-open")
                .icon(if open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .ghost()
                .small()
                .tooltip("The guidelines, and the steps the agent keeps against the goal")
                .on_click(move |_, window, cx| on_chevron(window, cx)),
        );
    let mut block = v_flex()
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .child(row);
    if open {
        let mut details = v_flex().gap_2().px_3().pb_2();
        if let Some(g) = &plan.goal {
            details = details.child(div().child(SharedString::from(g.clone())));
        }
        if let Some(g) = &plan.guidelines {
            details = details.child(
                v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(theme.font_size * 0.75)
                            .text_color(muted)
                            .child("GUIDELINES"),
                    )
                    .child(div().child(SharedString::from(g.clone()))),
            );
        }
        if !plan.steps.is_empty() {
            let mut steps = v_flex().gap_0p5().child(
                h_flex()
                    .justify_between()
                    .text_size(theme.font_size * 0.75)
                    .text_color(muted)
                    .child("STEPS")
                    .child(format!("{}/{}", plan.steps.len(), plan.soft_limit)),
            );
            for (status, summary) in &plan.steps {
                let lower = status.to_lowercase();
                let (glyph, colour) = if lower.contains("done") || lower.contains("complete") {
                    ("●", theme.success)
                } else if lower.contains("progress")
                    || lower.contains("working")
                    || lower.contains("active")
                {
                    ("◐", theme.warning)
                } else {
                    ("○", muted)
                };
                steps = steps.child(
                    h_flex()
                        .gap_2()
                        .items_start()
                        .text_size(theme.font_size * 0.9)
                        .child(div().flex_none().text_color(colour).child(glyph))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(SharedString::from(summary.clone())),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_color(muted)
                                .text_size(theme.font_size * 0.8)
                                .child(SharedString::from(status.clone())),
                        ),
                );
            }
            details = details.child(steps);
        }
        block = block.child(details);
    }
    Some(block.into_any_element())
}

// -- the scenario's steps -------------------------------------------------------------------------

/// How wide the stepper's gutter is, and so how far its card sits from the pane's edge.
pub const STEPPER_WIDTH: f32 = 40.;

/// What a step's line says: what a finished one concluded, else the toolset it asks for and
/// whether all of it is here.
fn step_line(
    run: &Run,
    ix: usize,
    toolsets: Option<&reactor_client::ToolsetsPayload>,
    tools: Option<&reactor_client::ToolsPayload>,
    theme: &Theme,
) -> (String, Hsla) {
    let Some(phase) = run.phases.get(ix) else {
        return (String::new(), theme.muted_foreground);
    };
    let missing = phase
        .toolset
        .as_deref()
        .map(|t| crate::scenario_ui::missing_tools(t, toolsets, tools))
        .unwrap_or_default();
    match &phase.outcome {
        Outcome::Summary(t) => (t.clone(), theme.foreground),
        Outcome::Skipped => ("Skipped.".to_string(), theme.muted_foreground),
        Outcome::Awaiting => (
            "Say what it concluded in your next prompt.".to_string(),
            theme.warning,
        ),
        Outcome::None => match (&phase.toolset, missing.is_empty()) {
            (Some(t), true) => (format!("Toolset {t}."), theme.muted_foreground),
            (Some(t), false) => (
                format!(
                    "Toolset {t}. Not available: {}. The step starts anyway, and the agent is not told about them.",
                    missing.join(", ")
                ),
                theme.warning,
            ),
            (None, _) => (String::new(), theme.muted_foreground),
        },
    }
}

/// The running scenario's steps as numbers down the pane's right edge. A click on a number
/// selects it (its card opens); a click on the selected one, or one with Ctrl held, goes
/// there. Moving by hand is for between turns, so it is disabled while the agent works
/// (ADR-0049).
pub fn stepper(
    app: &Weak,
    id: &str,
    run: &Run,
    selected: Option<usize>,
    idle: bool,
    select: SelectStep,
    theme: &Theme,
) -> AnyElement {
    let count = run.phases.len();
    let current = run.current;
    let (app, id) = (app.clone(), id.to_string());
    let steps = Stepper::new("scenario-steps")
        .vertical()
        .small()
        .selected_index(current)
        .disabled(!idle)
        // Each step but the last takes an equal share of this height: it is what keeps the
        // numbers a fixed distance apart instead of bunched at the top.
        .h(px(36. * count.max(1) as f32))
        .items((0..count).map(|_| StepperItem::new()))
        .on_click(move |ix, window, cx| {
            let ix = *ix;
            let modifiers = window.modifiers();
            let go =
                ix != current && (selected == Some(ix) || modifiers.control || modifiers.platform);
            if go {
                select(None, window, cx);
                let _ = app.update(cx, |app, cx| {
                    app.activate_session(&id, window, cx);
                    app.scenario_goto(ix + 1, cx);
                });
            } else if selected == Some(ix) {
                select(None, window, cx);
            } else {
                select(Some(ix), window, cx);
            }
        });
    div()
        .flex_none()
        .w(px(STEPPER_WIDTH))
        .pt_3()
        .pl_2()
        .border_l_1()
        .border_color(theme.border.opacity(0.5))
        .child(steps)
        .into_any_element()
}

/// The card of the selected step, beside the stepper.
#[allow(clippy::too_many_arguments)]
pub fn step_card(
    app: &Weak,
    id: &str,
    run: &Run,
    ix: usize,
    toolsets: Option<&reactor_client::ToolsetsPayload>,
    tools: Option<&reactor_client::ToolsPayload>,
    idle: bool,
    select: SelectStep,
    theme: &Theme,
) -> Option<AnyElement> {
    let phase = run.phases.get(ix)?;
    let muted = theme.muted_foreground;
    let (line, line_colour) = step_line(run, ix, toolsets, tools, theme);
    let standing = match ix.cmp(&run.current) {
        std::cmp::Ordering::Less => "done",
        std::cmp::Ordering::Equal => "current",
        std::cmp::Ordering::Greater => "not reached",
    };
    let close = select.clone();
    let mut card = v_flex()
        .id("step-card")
        .absolute()
        .top_2()
        .right(px(STEPPER_WIDTH + 6.))
        .w(px(300.))
        .max_w(relative(0.8))
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(theme.input)
        .bg(theme.popover)
        // A click in the card is not a click on the transcript behind it.
        .occlude()
        .child(
            h_flex()
                .justify_between()
                .items_start()
                .gap_2()
                .child(div().flex_1().min_w_0().child(SharedString::from(format!(
                    "{} · {}",
                    ix + 1,
                    phase.title
                ))))
                .child(
                    div()
                        .flex_none()
                        .text_size(small(theme))
                        .text_color(muted)
                        .child(standing),
                )
                .child(
                    Button::new("step-card-close")
                        .icon(IconName::Close)
                        .ghost()
                        .xsmall()
                        .on_click(move |_, window, cx| close(None, window, cx)),
                ),
        );
    if !line.is_empty() {
        card = card.child(
            div()
                .text_size(small(theme))
                .text_color(line_colour)
                .child(SharedString::from(line)),
        );
    }
    if ix != run.current {
        let n = ix + 1;
        let label = if ix > run.current {
            format!("Skip to step {n}")
        } else {
            format!("Go back to step {n}")
        };
        let after = select.clone();
        let (app, id) = (app.clone(), id.to_string());
        card = card.child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Button::new("step-go")
                        .label(label)
                        .small()
                        .primary()
                        .disabled(!idle)
                        .tooltip(if ix > run.current {
                            "The steps between are marked skipped, and your next prompt is recorded as what the step you leave concluded"
                        } else {
                            "The summaries of the steps that will be done again are dropped"
                        })
                        .on_click(move |_, window, cx| {
                            after(None, window, cx);
                            let _ = app.update(cx, |app, cx| {
                                app.activate_session(&id, window, cx);
                                app.scenario_goto(n, cx);
                            });
                        }),
                )
                .child(
                    div()
                        .text_size(theme.font_size * 0.8)
                        .text_color(muted)
                        .child(if idle {
                            format!("or click {n} again")
                        } else {
                            "when the agent has finished".to_string()
                        }),
                ),
        );
    }
    Some(card.into_any_element())
}

// -- the chips ----------------------------------------------------------------------------------

/// One chip: a small outlined, rounded button. `dot` marks a thing that is on (and can be
/// turned off); the Budget chip, which cannot, has none.
fn chip(id: &'static str, text: impl Into<SharedString>, dot: bool, theme: &Theme) -> Button {
    Button::new(id).small().outline().rounded(px(12.)).child(
        h_flex()
            .gap_1p5()
            .items_center()
            .when(dot, |row| {
                row.child(
                    div()
                        .size(px(6.))
                        .rounded_full()
                        .flex_none()
                        .bg(theme.accent),
                )
            })
            .child(div().text_size(small(theme)).child(text.into())),
    )
}

/// What a popover's rows are set in.
fn popover_body() -> Div {
    v_flex().gap_2().w(px(300.))
}

fn turn_off(
    id: &'static str,
    label: &'static str,
    tip: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    Button::new(id)
        .label(label)
        .small()
        .danger()
        .outline()
        .tooltip(tip)
        .on_click(on_click)
}

const LEVELS: [&str; 3] = ["prompt", "nag", "strict"];

/// The chips under the prompt, for session `id`.
#[allow(clippy::too_many_arguments)]
pub fn chips(
    app: &Weak,
    id: &str,
    pane: &PaneView,
    idle: bool,
    choosing_scenario: bool,
    choose_scenario: Flip,
    theme: &Theme,
) -> AnyElement {
    let (history_on, graph_on, docs_on) = pane.memory;
    let mut row = h_flex().gap_1p5().items_center().flex_wrap();

    // -- Budget: always there, first, with the mode in force --
    let mode = pane
        .context
        .as_ref()
        .map(|c| c.mode.clone())
        .unwrap_or_else(|| "…".into());
    row = row.child({
        let (app, id) = (app.clone(), id.to_string());
        Popover::new("budget-popover")
            .anchor(Anchor::BottomLeft)
            .trigger(chip(
                "chip-budget",
                format!("Budget · {mode}"),
                false,
                theme,
            ))
            .content(move |_, _, cx| budget_popover(&app, &id, idle, cx))
    });

    // -- the switches: a click turns one off --
    let memory = |label: &'static str,
                  chip_id: &'static str,
                  tip: &'static str,
                  off: fn() -> MemorySettings| {
        chip(chip_id, label, true, theme)
            .tooltip(tip)
            .on_click(on_session(app, id, move |app, _, cx| {
                app.set_memory(off(), cx)
            }))
    };
    if history_on {
        row = row.child(memory(
            "Other sessions",
            "chip-history",
            "On: the agent may search and read this case's other sessions. Click to turn off: it then sees only this session.",
            || MemorySettings {
                session_history: Some(false),
                ..Default::default()
            },
        ));
    }
    if graph_on {
        row = row.child(memory(
            "Case graph",
            "chip-graph",
            "On: the agent reads and writes the case's graph. Click to turn off: this session is then detached from it and leaves the case's results untouched.",
            || MemorySettings {
                case_graph: Some(false),
                ..Default::default()
            },
        ));
    }
    if docs_on {
        row = row.child(memory(
            "Documents",
            "chip-documents",
            "On: the agent may search the case's imported documents (docs_search). Click to turn off: the tool is withdrawn from this session; you can still search them yourself in the Documents panel.",
            || MemorySettings {
                documents: Some(false),
                ..Default::default()
            },
        ));
    }

    // -- the ones with settings: a click opens them, and Turn off is in there --
    if pane.plan.report_on {
        let (app, id) = (app.clone(), id.to_string());
        let level = LEVELS[pane.plan.report_level.min(2) as usize];
        row = row.child(
            Popover::new("reporting-popover")
                .anchor(Anchor::BottomLeft)
                .trigger(chip(
                    "chip-reporting",
                    format!("Reporting · {level}"),
                    true,
                    theme,
                ))
                .content(move |_, _, cx| reporting_popover(&app, &id, cx)),
        );
    }
    if goal_shown(&pane.plan) {
        let (app, id) = (app.clone(), id.to_string());
        row = row.child(
            Popover::new("goal-popover")
                .anchor(Anchor::BottomLeft)
                .trigger(chip("chip-goal", "Goal", true, theme))
                .content(move |_, _, cx| goal_popover(&app, &id, cx)),
        );
    }
    match &pane.scenario.run {
        Some(run) => {
            let (app, id) = (app.clone(), id.to_string());
            row = row.child(
                Popover::new("scenario-popover")
                    .anchor(Anchor::BottomLeft)
                    .trigger(chip(
                        "chip-scenario",
                        format!("Scenario · {}", run.id),
                        true,
                        theme,
                    ))
                    .content(move |_, _, cx| scenario_popover(&app, &id, cx)),
            );
        }
        None if choosing_scenario => {
            let (app, id) = (app.clone(), id.to_string());
            let cancel = choose_scenario.clone();
            row = row.child(
                Popover::new("scenario-choose-popover")
                    .anchor(Anchor::BottomLeft)
                    .default_open(true)
                    .trigger(chip("chip-scenario", "Scenario", false, theme))
                    .content(move |_, _, cx| scenario_chooser(&app, &id, idle, cancel.clone(), cx)),
            );
        }
        None => {}
    }
    if let Some(name) = &pane.identity {
        let (app, id) = (app.clone(), id.to_string());
        row = row.child(
            Popover::new("identity-popover")
                .anchor(Anchor::BottomLeft)
                .trigger(chip(
                    "chip-identity",
                    format!("Identity · {name}"),
                    true,
                    theme,
                ))
                .content(move |_, _, cx| identity_popover(&app, &id, cx)),
        );
    }

    // -- Activate: what is off --
    let scenario_off = pane.scenario.run.is_none() && !choosing_scenario;
    let anything_off = !history_on
        || !graph_on
        || !docs_on
        || !pane.plan.report_on
        || !goal_shown(&pane.plan)
        || scenario_off
        || pane.identity.is_none();
    if anything_off {
        let (app, id) = (app.clone(), id.to_string());
        let plan = pane.plan.clone();
        let identity_off = pane.identity.is_none();
        let scenarios = !pane.scenario.available.is_empty();
        row = row.child(
            Button::new("chip-activate")
                .small()
                .ghost()
                .icon(IconName::Plus)
                .label("Activate")
                .tooltip("What is off for this session")
                .dropdown_menu(move |mut menu, window, cx| {
                    let memory = |label: &'static str, on: fn() -> MemorySettings| {
                        PopupMenuItem::new(label).on_click(on_session(
                            &app,
                            &id,
                            move |app, _, cx| app.set_memory(on(), cx),
                        ))
                    };
                    if !history_on {
                        menu = menu.item(memory("Other sessions", || MemorySettings {
                            session_history: Some(true),
                            ..Default::default()
                        }));
                    }
                    if !graph_on {
                        menu = menu.item(memory("Case graph", || MemorySettings {
                            case_graph: Some(true),
                            ..Default::default()
                        }));
                    }
                    if !docs_on {
                        menu = menu.item(memory("Documents", || MemorySettings {
                            documents: Some(true),
                            ..Default::default()
                        }));
                    }
                    if !plan.report_on {
                        menu = menu.item(PopupMenuItem::new("Reporting").on_click(on_session(
                            &app,
                            &id,
                            |app, _, cx| app.report_set(true, cx),
                        )));
                    }
                    if !goal_shown(&plan) {
                        // Switched off with something set: switch it on again. Nothing set:
                        // there is no manifest until there is a goal, so write one.
                        let (switch_on, write) =
                            (!plan.manifest_on, plan.goal.is_none() && plan.guidelines.is_none());
                        menu = menu.item(PopupMenuItem::new("Goal").on_click(on_session(
                            &app,
                            &id,
                            move |app, window, cx| {
                                if switch_on {
                                    app.manifest_set(true, cx);
                                }
                                if write {
                                    app.open_goal_editor(window, cx);
                                }
                            },
                        )));
                    }
                    if scenario_off {
                        let choose = choose_scenario.clone();
                        menu = menu.item(
                            PopupMenuItem::new("Scenario")
                                .disabled(!scenarios)
                                .on_click(move |_, window, cx| choose(true, window, cx)),
                        );
                    }
                    if identity_off {
                        let names: Vec<String> = app
                            .upgrade()
                            .map(|a| {
                                a.read(cx)
                                    .identity_rows()
                                    .into_iter()
                                    .map(|r| r.name)
                                    .collect()
                            })
                            .unwrap_or_default();
                        let (app, id) = (app.clone(), id.clone());
                        menu = menu.submenu("Identity", window, cx, move |mut sub, _, _| {
                            for name in &names {
                                let chosen = name.clone();
                                sub = sub.item(PopupMenuItem::new(name.clone()).on_click(
                                    on_session(&app, &id, move |app, _, cx| {
                                        app.identity_use(&chosen, cx)
                                    }),
                                ));
                            }
                            sub
                        });
                    }
                    menu
                }),
        );
    }
    row.into_any_element()
}

fn pane_of(app: &Weak, id: &str, cx: &App) -> PaneView {
    app.upgrade()
        .map(|a| a.read(cx).pane(id, cx))
        .unwrap_or_default()
}

fn reporting_popover(app: &Weak, id: &str, cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    let plan = pane_of(app, id, cx).plan;
    let level = TabBar::new("reporting-level")
        .segmented()
        .small()
        .selected_index(plan.report_level.min(2) as usize)
        .children(LEVELS.iter().map(|l| Tab::new().label(*l)))
        .on_click({
            let (app, id) = (app.clone(), id.to_string());
            move |ix: &usize, window, cx| {
                let level = *ix as u8;
                let _ = app.update(cx, |app, cx| {
                    app.activate_session(&id, window, cx);
                    app.report_level(level, cx);
                });
            }
        });
    popover_body()
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(Label::new("Reporting"))
                .child(crate::hints::info_button(
                    "reporting-info",
                    "Reporting makes the agent write its findings up as it goes, in a folder of the case. prompt adds only the instructions; nag also reminds the agent whenever the folder has not changed for a while; strict reverts a turn that left it undocumented. Folder, thresholds and template are in Settings › Reporting. /report does the same.",
                )),
        )
        .child(level)
        .child(
            h_flex()
                .justify_between()
                .text_size(small(&theme))
                .child(div().text_color(theme.muted_foreground).child("folder"))
                .child(
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .child(SharedString::from(plan.report_folder.clone())),
                ),
        )
        .child(h_flex().justify_end().child(turn_off(
            "reporting-off",
            "Turn off",
            "The agent is no longer held to documenting its findings",
            on_session(app, id, |app, _, cx| app.report_set(false, cx)),
        )))
        .into_any_element()
}

fn goal_popover(app: &Weak, id: &str, cx: &mut App) -> AnyElement {
    let _ = cx;
    popover_body()
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(Label::new("Goal"))
                .child(crate::hints::info_button(
                    "goal-info",
                    "The goal and the guidelines belong to the case: every session of it is told them. The agent keeps its steps against the goal with update_steps, and they are this session's. Turning it off pauses all of it: nothing is sent and update_steps goes quiet, and what is set is kept. Frame shows what is sent. /goal, /guidelines, /manifest and /frame do the same.",
                )),
        )
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("goal-pop-edit")
                        .label("Edit…")
                        .small()
                        .on_click(on_session(app, id, |app, window, cx| {
                            app.open_goal_editor(window, cx)
                        })),
                )
                .child(
                    Button::new("goal-pop-frame")
                        .label("Frame…")
                        .small()
                        .on_click(on_session(app, id, |app, window, cx| {
                            app.open_frame(window, cx)
                        })),
                )
                .child(div().flex_1())
                .child(turn_off(
                    "goal-off",
                    "Turn off",
                    "Nothing of the goal, guidelines and steps is sent, and what is set is kept",
                    on_session(app, id, |app, _, cx| app.manifest_set(false, cx)),
                )),
        )
        .into_any_element()
}

/// The running scenario: every step with what it concluded, and *Stop*.
fn scenario_popover(app: &Weak, id: &str, cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    let muted = theme.muted_foreground;
    let (scenario, toolsets, tools) = app
        .upgrade()
        .map(|a| {
            let a = a.read(cx);
            (
                a.pane(id, cx).scenario,
                a.toolsets.clone(),
                a.catalogue.clone(),
            )
        })
        .unwrap_or_default();
    let Some(run) = scenario.run else {
        return popover_body()
            .child(div().text_color(muted).child("No scenario is running."))
            .into_any_element();
    };
    let mut body = popover_body().child(
        h_flex()
            .justify_between()
            .items_center()
            .child(
                div()
                    .font_family(theme.mono_font_family.clone())
                    .child(SharedString::from(run.id.clone())),
            )
            .child(
                div()
                    .text_size(small(&theme))
                    .text_color(muted)
                    .child(format!("step {} of {}", run.current + 1, run.phases.len())),
            ),
    );
    for (i, phase) in run.phases.iter().enumerate() {
        let (line, colour) = step_line(&run, i, toolsets.as_ref(), tools.as_ref(), &theme);
        let number_colour = match i.cmp(&run.current) {
            std::cmp::Ordering::Less => theme.success,
            std::cmp::Ordering::Equal => theme.accent,
            std::cmp::Ordering::Greater => muted,
        };
        body = body.child(
            h_flex()
                .gap_2()
                .items_start()
                .child(
                    div()
                        .flex_none()
                        .w(px(16.))
                        .font_family(theme.mono_font_family.clone())
                        .text_color(number_colour)
                        .child(format!("{}", i + 1)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_color(if i == run.current {
                                    theme.foreground
                                } else {
                                    muted
                                })
                                .child(SharedString::from(phase.title.clone())),
                        )
                        .when(!line.is_empty(), |c| {
                            c.child(
                                div()
                                    .text_size(theme.font_size * 0.8)
                                    .text_color(colour)
                                    .child(SharedString::from(line)),
                            )
                        }),
                ),
        );
    }
    body.child(h_flex().justify_end().child(turn_off(
        "scenario-stop",
        "Stop scenario",
        "The agent is no longer briefed and phase_complete goes away. What it did stays",
        on_session(app, id, |app, _, cx| app.scenario_abort(cx)),
    )))
    .into_any_element()
}

/// The scenarios there are to start, and *Start*.
fn scenario_chooser(app: &Weak, id: &str, idle: bool, done: Flip, cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    let (available, pick) = app
        .upgrade()
        .map(|a| {
            let a = a.read(cx);
            (a.pane(id, cx).scenario.available, a.scenario_pick.clone())
        })
        .unwrap_or_default();
    let chosen = pick
        .filter(|p| available.contains(p))
        .or_else(|| available.first().cloned());
    let mut choices = v_flex().gap_0p5();
    for name in &available {
        let (a, picked) = (app.clone(), name.clone());
        let is_chosen = chosen.as_deref() == Some(name.as_str());
        choices = choices.child(
            h_flex()
                .id(SharedString::from(format!("scenario-pick-{name}")))
                .px_2()
                .h(px(26.))
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .font_family(theme.mono_font_family.clone())
                .when(is_chosen, |r| {
                    r.bg(theme.list_active)
                        .border_1()
                        .border_color(theme.list_active_border)
                })
                .hover(|s| s.bg(theme.list_hover))
                .on_click(move |_, _, cx| {
                    let _ = a.update(cx, |app, cx| {
                        app.scenario_pick = Some(picked.clone());
                        cx.notify();
                    });
                })
                .child(SharedString::from(name.clone())),
        );
    }
    let (cancel, started) = (done.clone(), done);
    let (a, sid) = (app.clone(), id.to_string());
    popover_body()
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(Label::new("Scenario"))
                .child(crate::hints::info_button(
                    "scenario-info",
                    "A scenario is a series of steps, each with a briefing for the agent: it is added to the system prompt while the step is current, and the agent moves to the next with a tool call that carries a summary. Click a number at the pane's edge to read a step, and again to go there yourself — only between turns. After moving on by hand, say in your next prompt what the step you left concluded: it is recorded as its summary. /reactor-scenario list | start <id> | status | next | goto <n> | stop does the same.",
                )),
        )
        .child(choices)
        .child(
            h_flex()
                .gap_2()
                .justify_end()
                .child(
                    Button::new("scenario-cancel")
                        .label("Cancel")
                        .small()
                        .ghost()
                        .on_click(move |_, window, cx| cancel(false, window, cx)),
                )
                .child(
                    Button::new("scenario-start")
                        .label("Start")
                        .small()
                        .primary()
                        .disabled(!idle || available.is_empty())
                        .tooltip("Start the chosen scenario: its first step is briefed and the agent begins")
                        .on_click(move |_, window, cx| {
                            started(false, window, cx);
                            let _ = a.update(cx, |app, cx| {
                                app.activate_session(&sid, window, cx);
                                app.scenario_start(cx);
                            });
                        }),
                ),
        )
        .into_any_element()
}

fn identity_popover(app: &Weak, id: &str, cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    let muted = theme.muted_foreground;
    let active = pane_of(app, id, cx).identity;
    let rows = app
        .upgrade()
        .map(|a| a.read(cx).identity_rows())
        .unwrap_or_default();
    let mut list = v_flex().gap_0p5();
    for row in rows {
        let is_active = active.as_deref() == Some(row.name.as_str());
        let name = row.name.clone();
        let detail = reactor_context::identity::builtin_description(&row.name).unwrap_or("saved");
        list = list.child(
            h_flex()
                .id(SharedString::from(format!("identity-{}", row.name)))
                .px_2()
                .h(px(26.))
                .gap_2()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .when(is_active, |r| {
                    r.bg(theme.list_active)
                        .border_1()
                        .border_color(theme.list_active_border)
                })
                .hover(|s| s.bg(theme.list_hover))
                .on_click(on_session(app, id, move |app, _, cx| {
                    app.identity_use(&name, cx)
                }))
                .child(
                    div()
                        .flex_none()
                        .font_family(theme.mono_font_family.clone())
                        .child(SharedString::from(row.name.clone())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(theme.font_size * 0.8)
                        .text_color(muted)
                        .child(detail),
                ),
        );
    }
    popover_body()
        .w(px(340.))
        .child(Label::new("Identity"))
        .child(list)
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("identity-show")
                        .label("Show text")
                        .small()
                        .on_click(on_session(app, id, |app, window, cx| {
                            app.run_command("identity show", window, cx)
                        })),
                )
                .child(
                    Button::new("identity-manage")
                        .label("Manage…")
                        .small()
                        .on_click(on_session(app, id, |app, window, cx| {
                            app.run_command("identity manage", window, cx)
                        })),
                )
                .child(div().flex_1())
                .child(turn_off(
                    "identity-off",
                    "Turn off",
                    "No persona is put into the system prompt",
                    on_session(app, id, |app, _, cx| app.identity_use("off", cx)),
                )),
        )
        .into_any_element()
}

/// The context budget: the mode and its knobs, where each comes from, reducing by hand, and
/// what is in force (ADR-0037, ADR-0038). It has no off: a mode always applies.
fn budget_popover(app: &Weak, id: &str, idle: bool, cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    let muted = theme.muted_foreground;
    let (view, preview, busy) = app
        .upgrade()
        .map(|a| {
            let a = a.read(cx);
            let active = a.active_session_id() == id;
            (
                a.pane(id, cx).context,
                // The preview and the reduction in flight are the active session's.
                a.preview.clone().filter(|_| active),
                a.context_busy && active,
            )
        })
        .unwrap_or_default();
    let Some(view) = view else {
        return popover_body()
            .child(div().text_color(muted).child("measuring the context…"))
            .into_any_element();
    };
    let origin_of = |key: &str| {
        view.origins
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, o)| *o)
            .unwrap_or(Origin::Default)
    };
    let setting = |label: &'static str, value: String, key: &str| {
        h_flex()
            .justify_between()
            .items_center()
            .text_size(small(&theme))
            .child(div().text_color(muted).child(label))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .font_family(theme.mono_font_family.clone())
                            .child(value),
                    )
                    .child(crate::panels::origin_badge(origin_of(key), &theme)),
            )
    };
    let modes = ["auto", "fade", "compact"];
    let mode_tabs = TabBar::new("context-mode")
        .segmented()
        .small()
        .selected_index(modes.iter().position(|m| *m == view.mode).unwrap_or(0))
        .children(modes.iter().map(|m| Tab::new().label(*m)))
        .on_click({
            let (app, id) = (app.clone(), id.to_string());
            move |ix: &usize, window, cx| {
                let Some(mode) = modes.get(*ix) else { return };
                let _ = app.update(cx, |app, cx| {
                    app.activate_session(&id, window, cx);
                    app.set_context(
                        ContextSettings {
                            mode: Some(mode.to_string()),
                            ..Default::default()
                        },
                        cx,
                    );
                });
            }
        });
    let action = |button_id: &'static str, label: &'static str, mode: Mode, reduce: bool| {
        Button::new(button_id)
            .label(label)
            .small()
            .disabled(busy || !idle)
            .on_click(on_session(app, id, move |app, _, cx| {
                if reduce {
                    app.reduce_now(mode, cx)
                } else {
                    app.preview_reduction(mode, cx)
                }
            }))
    };
    let mut body = popover_body()
        .w(px(340.))
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(Label::new("Context budget"))
                .child(crate::hints::info_button(
                    "context-settings-info",
                    "Change one with /context <mode|window|reserve|pct|keep|mask|protect|summarizer> <value>, or use the mode buttons. Old tool output is masked (shown as a stub, the original kept) down to \"mask to\" before anything is summarized; \"spare newest\" is the share of the window never masked. \"this session\" values apply to this session only; \"default\" ones come from settings.json.",
                )),
        )
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(div().flex_1().child(mode_tabs))
                .child(crate::panels::origin_badge(origin_of("mode"), &theme)),
        )
        .child(setting("window", view.window.to_string(), "window"))
        .child(setting("reserve", view.reserve.to_string(), "reserve"))
        .child(setting("reduce at", format!("{:.0}%", view.pct * 100.0), "pct"))
        .child(setting("keep", format!("{:.0}%", view.keep * 100.0), "keep"))
        .child(setting(
            "mask to",
            format!("{:.0}%", view.mask_target * 100.0),
            "maskTarget",
        ))
        .child(setting(
            "spare newest",
            format!("{:.0}%", view.protect * 100.0),
            "protect",
        ))
        .child(setting(
            "summarizer",
            view.summarizer
                .clone()
                .unwrap_or_else(|| "the session's model".into()),
            "summarizer",
        ))
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("context-default")
                        .label("Make default")
                        .small()
                        .tooltip("Every new session starts with these settings")
                        .on_click(on_session(app, id, |app, _, cx| {
                            app.make_context_default(cx)
                        })),
                )
                .child(
                    Button::new("context-inherit")
                        .label("Inherit")
                        .small()
                        .tooltip("Drop this session's own values")
                        .on_click(on_session(app, id, |app, _, cx| app.inherit_context(cx))),
                )
                .child(div().flex_1())
                .child(
                    Button::new("context-inspect")
                        .label("Context window")
                        .small()
                        .tooltip("Everything the model would be sent next, and where each piece comes from")
                        .on_click(on_session(app, id, |app, window, cx| {
                            app.toggle_inspect(window, cx)
                        })),
                ),
        )
        .child(
            v_flex()
                .gap_1()
                .pt_2()
                .border_t_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .gap_1()
                        .child(action(
                            "preview-compact",
                            "Preview: summarize",
                            Mode::Compact,
                            false,
                        ))
                        .child(action("preview-fade", "Preview: fade", Mode::Fade, false)),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .child(action("reduce-compact", "Summarize now", Mode::Compact, true))
                        .child(action("reduce-fade", "Fade now", Mode::Fade, true)),
                ),
        );
    if busy {
        body = body.child(
            div()
                .text_size(small(&theme))
                .text_color(muted)
                .child("working…"),
        );
    }
    if let Some(p) = &preview {
        body = body.child(
            v_flex()
                .gap_1()
                .p_2()
                .rounded_md()
                .bg(theme.secondary)
                .text_size(small(&theme))
                .child(format!(
                    "{:?}: would reduce {} entries, keeping the newest",
                    p.mode, p.entries
                ))
                .child(div().text_color(muted).child(format!(
                    "{} tool results (~{} tokens) and {} messages (~{} tokens)",
                    p.mechanical, p.mechanical_tokens, p.conceptual, p.conceptual_tokens
                )))
                .child(div().text_color(muted).child(format!(
                    "~{} → ~{} tokens{}",
                    p.before_tokens,
                    p.after_tokens,
                    if p.needs_model {
                        " · asks the model for a summary"
                    } else {
                        " · no model call"
                    }
                ))),
        );
    }
    let mut in_force = v_flex()
        .gap_1()
        .pt_2()
        .border_t_1()
        .border_color(theme.border)
        .text_size(small(&theme))
        .child(
            div()
                .text_color(muted)
                .child(if view.reductions.is_empty() {
                    "In force: none"
                } else {
                    "In force"
                }),
        );
    for r in &view.reductions {
        let entry = r.entry;
        in_force = in_force.child(
            h_flex()
                .justify_between()
                .items_center()
                .child(if r.masks > 0 && r.covers == 0 {
                    format!(
                        "#{} masked · {} · {} tool results · ~{} → ~{}",
                        r.entry,
                        crate::backend::trigger_label(r.trigger),
                        r.masks,
                        r.before_tokens,
                        r.after_tokens
                    )
                } else {
                    format!(
                        "#{} {:?} · {} · {} entries · ~{} → ~{}",
                        r.entry,
                        r.mode,
                        crate::backend::trigger_label(r.trigger),
                        r.covers,
                        r.before_tokens,
                        r.after_tokens
                    )
                })
                .child(
                    Button::new(("undo", r.entry as usize))
                        .label("undo")
                        .small()
                        .on_click(on_session(app, id, move |app, _, cx| {
                            app.restore_reduction(entry, cx)
                        })),
                ),
        );
    }
    body.child(in_force).into_any_element()
}
