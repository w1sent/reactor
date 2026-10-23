//! The dialog that starts a scenario: what it is for, the question it starts from, and which
//! of its phases to run ([ADR-0065](../../../docs/adr/0065-a-scenario-is-general-starts-from-a-question-and-can-be-written-for-the-case.md)).
//! A gpui-kit dialog whose content is built again on every frame from the app's state, like the
//! goal editor's ([`crate::manifest_ui`]); the state lives on [`ReactorApp`].

use std::collections::BTreeSet;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::label::Label;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, SharedString, div, px};

use crate::app::ReactorApp;

/// The start dialog, while it is open.
pub struct Starter {
    pub id: String,
    pub description: Option<String>,
    pub titles: Vec<String>,
    /// Becomes the case's goal; prefilled with the goal there is.
    pub question: Entity<TextareaState>,
    /// The phases left out, 0-based.
    pub skip: BTreeSet<usize>,
    /// Start it in a new session of the case rather than in this one.
    pub new_session: bool,
    pub error: Option<String>,
}

/// The command that starts scenario `id`: `start <id> [skip=…] [question]`, the phases
/// numbered from 1.
pub fn start_args(id: &str, skip: &BTreeSet<usize>, question: &str) -> String {
    let mut args = format!("start {id}");
    if !skip.is_empty() {
        let list: Vec<String> = skip.iter().map(|i| (i + 1).to_string()).collect();
        args.push_str(&format!(" skip={}", list.join(",")));
    }
    let question = question.trim();
    if !question.is_empty() {
        args.push(' ');
        args.push_str(question);
    }
    args
}

/// The dialog's contents.
pub fn view(weak: &gpui_kit::WeakEntity<ReactorApp>, cx: &mut App) -> AnyElement {
    let Some(app) = weak.upgrade() else {
        return div().into_any_element();
    };
    let app = app.read(cx);
    let Some(st) = app.scenario_starter.as_ref() else {
        return div().into_any_element();
    };
    let theme = cx.theme().clone();
    let (muted, small) = (theme.muted_foreground, theme.font_size * 0.8);
    let mut phases = v_flex().gap_1();
    for (i, title) in st.titles.iter().enumerate() {
        let w = weak.clone();
        phases = phases.child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Checkbox::new(("scenario-start-phase", i))
                        .checked(!st.skip.contains(&i))
                        .on_click(move |_: &bool, window, cx| {
                            let _ = w.update(cx, |app, cx| {
                                if let Some(st) = app.scenario_starter.as_mut()
                                    && !st.skip.remove(&i)
                                {
                                    st.skip.insert(i);
                                }
                                cx.notify();
                            });
                            window.refresh();
                        }),
                )
                .child(
                    div()
                        .w(px(18.))
                        .font_family(theme.mono_font_family.clone())
                        .text_color(muted)
                        .child(format!("{}", i + 1)),
                )
                .child(
                    div()
                        .text_color(if st.skip.contains(&i) {
                            muted
                        } else {
                            theme.foreground
                        })
                        .child(SharedString::from(if title.is_empty() {
                            format!("Phase {}", i + 1)
                        } else {
                            title.clone()
                        })),
                ),
        );
    }
    v_flex()
        .gap_3()
        .children(st.description.clone().map(|d| {
            div()
                .text_color(theme.foreground)
                .child(SharedString::from(d))
        }))
        .child(
            v_flex()
                .gap_1()
                .child(Label::new("Question").text_color(muted).text_size(theme.font_size * 0.9))
                .child(div().text_color(muted).text_size(small).child(
                    "What this run is to answer, and what you already know: the files, hosts or samples in question. It becomes the case's goal; left empty, the goal stays as it is.",
                ))
                .when(st.new_session, |el| {
                    el.child(div().text_color(theme.warning).text_size(small).child(
                        "A new session starts with an empty context: put what the triage found into the question.",
                    ))
                })
                .child(Textarea::new(&st.question).h(px(130.))),
        )
        .child(
            v_flex()
                .gap_1()
                .child(Label::new("Phases").text_color(muted).text_size(theme.font_size * 0.9))
                .child(div().text_color(muted).text_size(small).child(
                    "Untick the phases that do not apply. They are passed over and shown as skipped; you can still go to one from the stepper.",
                ))
                .child(phases),
        )
        .children(st.error.clone().map(|e| div().text_color(theme.danger).child(SharedString::from(e))))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_names_the_phases_left_out_one_based_and_keeps_the_question() {
        assert_eq!(start_args("inv", &BTreeSet::new(), "  "), "start inv");
        let skip = BTreeSet::from([2, 0]);
        assert_eq!(
            start_args("inv", &skip, "Was it\nthe VPN?"),
            "start inv skip=1,3 Was it\nthe VPN?"
        );
    }
}
