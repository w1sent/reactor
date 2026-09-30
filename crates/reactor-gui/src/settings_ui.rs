//! The settings popup (SPEC.md §6.2): fonts and sizes per part of the window, and a few
//! behaviours. Every change applies at once and is saved; there is no OK button.
//!
//! This file only draws. The state lives on [`ReactorApp`] (`ui`, `settings`) and every
//! change goes through its methods, which keep the theme, the file and the window in step.

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{Entity, MouseButton, SharedString, div, px};

use crate::app::ReactorApp;
use crate::layout::LayoutPreset;
use crate::settings::{OUTPUT_CHARS, Slot};

/// The popup's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Fonts,
    Behaviour,
    Model,
    Context,
    Tools,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Fonts, Tab::Behaviour, Tab::Model, Tab::Context, Tab::Tools];

    fn label(self) -> &'static str {
        match self {
            Tab::Fonts => "Fonts",
            Tab::Behaviour => "Behaviour",
            Tab::Model => "Model",
            Tab::Context => "Context",
            Tab::Tools => "Tools",
        }
    }
}

/// The open popup: a tab, one text input per font slot, the model tab's fields, and a line
/// for what went wrong.
pub struct SettingsUi {
    pub tab: Tab,
    pub inputs: Vec<(Slot, Entity<InputState>)>,
    pub message: Option<String>,
    /// A plain confirmation, shown in the accent colour.
    pub notice: Option<String>,
    /// The provider picked on the model tab.
    pub provider: String,
    /// The model name typed after it.
    pub model_input: Entity<InputState>,
    /// What `settings.json` holds now: the default model, and the picker's list.
    pub default_model: Option<String>,
    pub models: Vec<String>,
    /// The context tab: which layer it edits, that layer as stored, and a field per number.
    pub context_scope: ContextScope,
    pub context_layer: reactor_context::settings::ContextSettings,
    pub context_inputs: Vec<(CtxField, Entity<InputState>)>,
}

/// Which layer of the context settings the Context tab edits (ADR-0038).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextScope {
    /// `settings.json`: what every new session starts with.
    Default,
    /// This session only.
    Session,
}

/// The context settings that are typed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtxField {
    Window,
    Reserve,
    Pct,
    Keep,
    Summarizer,
}

impl CtxField {
    pub const ALL: [CtxField; 5] = [CtxField::Window, CtxField::Reserve, CtxField::Pct, CtxField::Keep, CtxField::Summarizer];

    pub fn label(self) -> &'static str {
        match self {
            CtxField::Window => "Context window",
            CtxField::Reserve => "Reserve",
            CtxField::Pct => "Reduce at",
            CtxField::Keep => "Keep",
            CtxField::Summarizer => "Summarizer",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            CtxField::Window => "In tokens: how much the model can take in. Not asked of the provider: set it for your model. Accepts 200000 or 200k.",
            CtxField::Reserve => "In tokens: headroom kept free for the model's reply. Usable context is the window minus this. 8000 or 8k.",
            CtxField::Pct => "In percent of the usable window, 1–100: a reduction starts once the context passes this share.",
            CtxField::Keep => "In percent of the usable window, 1–99: a reduction goes down to this share, keeping the newest work.",
            CtxField::Summarizer => "The model that writes summaries, as provider/name. Empty: the session's own model.",
        }
    }
}

/// The environment variable each provider's key is read from.
fn key_variable(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "ANTHROPIC_API_KEY",
        "openai" => "OPENAI_API_KEY",
        "gemini" => "GEMINI_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        _ => "OLLAMA_API_BASE_URL",
    }
}

/// The popup, over a scrim. `weak` is the app's handle, for the buttons.
pub fn overlay(app: &ReactorApp, weak: gpui_kit::WeakEntity<ReactorApp>, cx: &mut Context<ReactorApp>) -> Option<impl IntoElement> {
    let state = app.settings.as_ref()?;
    let ui = app.ui.clone();
    let theme = cx.theme().clone();
    let muted = theme.muted_foreground;
    let small = theme.font_size * 0.85;

    // -- fonts --
    let mut fonts = v_flex().gap_2();
    for (slot, input) in &state.inputs {
        let slot = *slot;
        let (_, effective) = ui.text(slot, &theme);
        let chosen = ui.size(slot).is_some();
        let (w_minus, w_plus, w_reset) = (weak.clone(), weak.clone(), weak.clone());
        let step = move |delta: f32, weak: gpui_kit::WeakEntity<ReactorApp>| {
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                weak.update(cx, |app, cx| {
                    let now = f32::from(app.ui.text(slot, cx.theme()).1);
                    app.update_ui(|ui| ui.set_size(slot, Some((now + delta).round())), cx);
                })
                .ok();
            }
        };
        fonts = fonts.child(
            h_flex()
                .gap_2()
                .items_center()
                .child(div().w(px(190.)).child(Label::new(slot.label())))
                .child(crate::hints::tip(
                    div().id(("font-family", slot as usize)).flex_1().child(Input::new(input)),
                    "An installed font's name, then Enter. Empty is the default.",
                ))
                .child(Button::new(("size-minus", slot as usize)).label("−").small().on_click(step(-1.0, w_minus)))
                .child(crate::hints::tip(
                    div()
                        .id(("font-size", slot as usize))
                        .w(px(34.))
                        .flex()
                        .justify_center()
                        .text_color(if chosen { theme.foreground } else { muted })
                        .child(format!("{}", f32::from(effective).round())),
                    if chosen { "Size in pixels" } else { "Size in pixels — the default" },
                ))
                .child(Button::new(("size-plus", slot as usize)).label("+").small().on_click(step(1.0, w_plus)))
                .child(Button::new(("font-reset", slot as usize)).label("reset").small().ghost().on_click(move |_, window, cx| {
                    w_reset.update(cx, |app, cx| app.reset_font(slot, window, cx)).ok();
                })),
        );
    }
    if let Some(message) = &state.message {
        fonts = fonts.child(Label::new(message.clone()).text_color(theme.warning).text_size(small));
    }

    // -- behaviour --
    let switch = |id: &'static str, label: &'static str, on: bool, weak: gpui_kit::WeakEntity<ReactorApp>, set: fn(&mut crate::settings::UiSettings, bool)| {
        Switch::new(id).checked(on).label(label).on_click(move |value, _window, cx| {
            let value = *value;
            weak.update(cx, |app, cx| app.update_ui(|ui| set(ui, value), cx)).ok();
        })
    };
    let output = ui.tool_output_chars;
    let (w_less, w_more, w_layouts) = (weak.clone(), weak.clone(), weak.clone());
    let output_step = move |up: bool, weak: gpui_kit::WeakEntity<ReactorApp>| {
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            weak.update(cx, |app, cx| {
                let now = app.ui.tool_output_chars;
                let step = if now < 5000 { 500 } else { 2500 };
                let next = if up { now + step } else { now.saturating_sub(step) };
                app.update_ui(|ui| ui.tool_output_chars = next.clamp(*OUTPUT_CHARS.start(), *OUTPUT_CHARS.end()), cx);
            })
            .ok();
        }
    };
    let (w_notice_less, w_notice_more) = (weak.clone(), weak.clone());
    let notice_step = move |up: bool, weak: gpui_kit::WeakEntity<ReactorApp>| {
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            weak.update(cx, |app, cx| {
                let now = app.ui.notice_seconds;
                let step = if now < 10 { 1 } else { 5 };
                let next = if up { now + step } else { now.saturating_sub(step) };
                app.update_ui(|ui| ui.notice_seconds = next, cx);
            })
            .ok();
        }
    };
    let mut layouts = h_flex().gap_1();
    for preset in LayoutPreset::ALL {
        let active = ui.start_layout() == preset;
        let weak = w_layouts.clone();
        layouts = layouts.child(
            Button::new(SharedString::from(format!("start-{}", preset.label())))
                .label(preset.label())
                .small()
                .when(active, |b| b.primary())
                .on_click(move |_, _window, cx| {
                    weak.update(cx, |app, cx| app.update_ui(|ui| ui.start_layout = preset.label().to_ascii_lowercase(), cx)).ok();
                }),
        );
    }
    let behaviour = v_flex()
        .gap_2()
        .child(switch("expand-thinking", "Open thinking blocks by default", ui.expand_thinking, weak.clone(), |ui, v| ui.expand_thinking = v))
        .child(switch("dim-reduced", "Dim what a context reduction took out of the model's view", ui.dim_reduced, weak.clone(), |ui, v| ui.dim_reduced = v))
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(crate::hints::tip(
                    div().id("output-label").w(px(260.)).child(Label::new("Tool output shown in the transcript (characters)")),
                    "Only what the transcript shows is cut. The session log keeps all of it.",
                ))
                .child(Button::new("output-less").label("−").small().on_click(output_step(false, w_less)))
                .child(div().w(px(56.)).flex().justify_center().child(format!("{output}")))
                .child(Button::new("output-more").label("+").small().on_click(output_step(true, w_more))),
        )
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(crate::hints::tip(
                    div().id("notice-label").w(px(260.)).child(Label::new("Notification popups stay (seconds)")),
                    "Every notification is also kept behind the bell in the title bar.",
                ))
                .child(Button::new("notice-less").label("−").small().on_click(notice_step(false, w_notice_less)))
                .child(div().w(px(56.)).flex().justify_center().child(format!("{}", ui.notice_seconds)))
                .child(Button::new("notice-more").label("+").small().on_click(notice_step(true, w_notice_more))),
        )
        .child(h_flex().gap_2().items_center().child(div().w(px(260.)).child(Label::new("Layout a window opens with"))).child(layouts));

    let model = model_tab(state, &weak, &theme, cx);
    let mut tabs = h_flex().gap_1();
    for tab in Tab::ALL {
        let weak = weak.clone();
        tabs = tabs.child(
            Button::new(SharedString::from(format!("settings-tab-{}", tab.label())))
                .label(tab.label())
                .small()
                .when(state.tab == tab, |b| b.primary())
                .on_click(move |_, _window, cx| {
                    weak.update(cx, |app, cx| app.settings_tab(tab, cx)).ok();
                }),
        );
    }
    let (w_fonts, w_behaviour) = (weak.clone(), weak.clone());
    let fonts = fonts.child(
        Button::new("fonts-reset")
            .label("Reset fonts")
            .small()
            .tooltip("Every font and size on this tab goes back to the theme's own")
            .on_click(move |_, window, cx| {
                w_fonts.update(cx, |app, cx| app.reset_fonts(window, cx)).ok();
            }),
    );
    let behaviour = behaviour.child(
        Button::new("behaviour-reset")
            .label("Reset behaviour")
            .small()
            .tooltip("Every option on this tab goes back to its standard value")
            .on_click(move |_, _window, cx| {
                w_behaviour.update(cx, |app, cx| app.reset_behaviour(cx)).ok();
            }),
    );
    let body = match state.tab {
        Tab::Fonts => fonts.into_any_element(),
        Tab::Behaviour => behaviour.into_any_element(),
        Tab::Tools => tools_tab(app, &weak),
        Tab::Model => model,
        Tab::Context => context_tab(app, state, &weak, &theme),
    };

    let (w_close, w_scrim) = (weak.clone(), weak.clone());
    let card = v_flex()
        .id("settings-card")
        .w(px(720.))
        .max_w_full()
        .max_h(relative(0.88))
        .overflow_y_scroll()
        .gap_4()
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(
                    h_flex().gap_1().items_center().child(Label::new("Settings").text_size(theme.font_size * 1.3)).child(crate::hints::info_button(
                        "settings-info",
                        match state.tab {
                            Tab::Model => "Saved in ~/.reactor/settings.json, which the agent reads too. The context budget is on the Context tab.",
                Tab::Context => "The default is saved in ~/.reactor/settings.json; a session's own values are kept in that session. Each value is used from the first place that sets it: this session, then the default, then a built-in guess.",
                            Tab::Tools => "Activation is kept per session, with the machine default in ~/.reactor/state.json.",
                            _ => "Saved in ~/.reactor/gui.json, as you change them, for every session. The agent's own settings are on the Model and Context tabs.",
                        },
                    )),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(Button::new("settings-close").label("Close").small().primary().on_click(move |_, window, cx| {
                            w_close.update(cx, |app, cx| app.close_settings(window, cx)).ok();
                        })),
                ),
        )
        .child(tabs)
        .child(body)
        .children(state.notice.clone().map(|n| Label::new(n).text_color(theme.accent).text_size(small)));

    Some(
        div()
            .id("settings-scrim")
            .key_context("ReactorSettings")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(theme.background.opacity(0.6))
            .flex()
            .justify_center()
            .items_start()
            .pt(px(40.))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                w_scrim.update(cx, |app, cx| app.close_settings(window, cx)).ok();
            })
            .child(card),
    )
}

/// The model tab: which model new sessions start on, which providers have a key, and the list
/// the status bar's picker offers.
fn model_tab(state: &SettingsUi, weak: &gpui_kit::WeakEntity<ReactorApp>, theme: &gpui_kit::component::Theme, _cx: &mut Context<ReactorApp>) -> gpui_kit::AnyElement {
    let muted = theme.muted_foreground;
    let small = theme.font_size * 0.85;

    let mut providers = h_flex().gap_1();
    for provider in reactor_agent::provider::PROVIDERS {
        let weak = weak.clone();
        providers = providers.child(
            Button::new(SharedString::from(format!("provider-{provider}")))
                .label(provider)
                .small()
                .when(state.provider == provider, |b| b.primary())
                .on_click(move |_, _window, cx| {
                    weak.update(cx, |app, cx| app.settings_provider(provider, cx)).ok();
                }),
        );
    }
    let variable = key_variable(&state.provider);
    let (key_text, key_ok, key_help) = if state.provider == "ollama" {
        ("local".to_string(), true, format!("Ollama runs on this machine and needs no key. {variable} overrides its address."))
    } else if std::env::var_os(variable).is_some() {
        (format!("{variable} set"), true, format!("REactor reads the key from {variable} in the environment; it is never stored."))
    } else {
        (format!("{variable} not set"), false, format!("REactor reads the key from {variable} in the environment and never stores it. Set it before starting the GUI."))
    };

    let action = |id: &'static str, label: &'static str, weak: gpui_kit::WeakEntity<ReactorApp>, run: fn(&mut ReactorApp, &mut Context<ReactorApp>)| {
        Button::new(id).label(label).small().on_click(move |_, _window, cx| {
            weak.update(cx, |app, cx| run(app, cx)).ok();
        })
    };

    let mut listed = v_flex().gap_1();
    if state.models.is_empty() {
        listed = listed.child(Label::new("none").text_color(muted).text_size(small));
    }
    for spec in &state.models {
        let (w_use, w_default, w_remove) = (weak.clone(), weak.clone(), weak.clone());
        let (s_use, s_default, s_remove) = (spec.clone(), spec.clone(), spec.clone());
        let is_default = state.default_model.as_deref() == Some(spec.as_str());
        listed = listed.child(
            h_flex()
                .gap_2()
                .items_center()
                .child(div().flex_1().font_family(theme.mono_font_family.clone()).text_size(theme.mono_font_size).child(spec.clone()))
                .child(Button::new(SharedString::from(format!("use-{spec}"))).label("use now").small().on_click(move |_, _w, cx| {
                    let spec = s_use.clone();
                    w_use.update(cx, |app, cx| app.set_model(&spec, cx)).ok();
                }))
                .child(
                    Button::new(SharedString::from(format!("default-{spec}")))
                        .label(if is_default { "default ✓" } else { "make default" })
                        .small()
                        .when(is_default, |b| b.primary())
                        .on_click(move |_, _w, cx| {
                            let spec = s_default.clone();
                            w_default.update(cx, |app, cx| app.set_default_model(Some(spec), cx)).ok();
                        }),
                )
                .child(Button::new(SharedString::from(format!("remove-{spec}"))).label("remove").small().ghost().on_click(move |_, _w, cx| {
                    let spec = s_remove.clone();
                    w_remove.update(cx, |app, cx| app.unlist_model(&spec, cx)).ok();
                })),
        );
    }

    v_flex()
        .gap_3()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(Label::new("Default model").text_color(muted))
                .child(div().font_family(theme.mono_font_family.clone()).text_size(theme.mono_font_size).child(state.default_model.clone().unwrap_or_else(|| "none".into()))),
        )
        .child(Label::new("Provider").text_color(muted).text_size(small))
        .child(providers)
        .child(crate::hints::tip(
            div().id("key-status").text_color(if key_ok { muted } else { theme.warning }).text_size(small).child(key_text),
            key_help,
        ))
        .child(Label::new("Model name").text_color(muted).text_size(small))
        .child(Input::new(&state.model_input))
        .child(
            h_flex()
                .gap_2()
                .child(action("model-default", "Set as default", weak.clone(), |app, cx| app.set_typed_model(ModelUse::Default, cx)))
                .child(action("model-now", "Use in this session", weak.clone(), |app, cx| app.set_typed_model(ModelUse::Now, cx)))
                .child(action("model-list", "Add to picker list", weak.clone(), |app, cx| app.set_typed_model(ModelUse::List, cx)))
                .child(action("model-clear", "Clear default", weak.clone(), |app, cx| app.set_default_model(None, cx)))
                .child(
                    Button::new("model-promote")
                        .label("Make session model the default")
                        .small()
                        .tooltip("The model this session is using now becomes the default for new sessions")
                        .on_click({
                            let weak = weak.clone();
                            move |_, _window, cx| {
                                weak.update(cx, |app, cx| app.promote_model(cx)).ok();
                            }
                        }),
                ),
        )
        .child(Label::new("Models in the picker").text_color(muted).text_size(small))
        .child(listed)
        .into_any_element()
}

/// What the model tab's "typed model" buttons do with `provider/name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelUse {
    Default,
    Now,
    List,
}

/// The Context tab: the budget settings of the agent, for the default or for this session.
fn context_tab(app: &ReactorApp, state: &SettingsUi, weak: &gpui_kit::WeakEntity<ReactorApp>, theme: &gpui_kit::component::Theme) -> gpui_kit::AnyElement {
    let muted = theme.muted_foreground;
    let layer = &state.context_layer;
    let session = state.context_scope == ContextScope::Session;

    let mut scope = h_flex().gap_1().items_center().child(Label::new("Edit").text_color(muted)).child(crate::hints::info_button(
        "ctx-info",
        if session {
            "These values apply to this session only. An empty field uses the default; the grey value in it is what this session uses now."
        } else {
            "These values are the default for new sessions. An empty field uses a built-in guess for the provider; the grey value in it is what this session uses now."
        },
    ));
    for (label, which) in [("Default for new sessions", ContextScope::Default), ("This session only", ContextScope::Session)] {
        let weak = weak.clone();
        scope = scope.child(
            Button::new(SharedString::from(format!("ctx-scope-{label}")))
                .label(label)
                .small()
                .when(state.context_scope == which, |b| b.primary())
                .on_click(move |_, window, cx| {
                    weak.update(cx, |app, cx| app.context_scope(which, window, cx)).ok();
                }),
        );
    }

    // -- mode --
    let mut modes = h_flex().gap_1().items_center();
    for mode in reactor_context::settings::CONTEXT_MODES {
        let weak = weak.clone();
        modes = modes.child(
            Button::new(SharedString::from(format!("ctx-mode-{mode}")))
                .label(mode)
                .small()
                .when(layer.mode.as_deref() == Some(mode), |b| b.primary())
                .on_click(move |_, _w, cx| {
                    weak.update(cx, |app, cx| app.context_set_mode(Some(mode), cx)).ok();
                }),
        );
    }
    let w_mode = weak.clone();
    let effective = app.context.as_ref().map(|c| c.mode.clone()).unwrap_or_default();
    let mode_row = h_flex()
        .gap_2()
        .items_center()
        .child(crate::hints::tip(
            div().id("ctx-mode-label").w(px(250.)).child(Label::new("Reduction mode")),
            "auto: summarize, and drop what can be recovered. fade: drop old messages, leaving stubs. compact: summarize old messages.",
        ))
        .child(modes)
        .child(
            Button::new("ctx-mode-reset")
                .label("inherit")
                .small()
                .ghost()
                .tooltip(if layer.mode.is_none() { format!("Not set here; this session uses {effective}") } else { format!("Clear it; this session would use {effective} or the default") })
                .on_click(move |_, _w, cx| {
                    w_mode.update(cx, |app, cx| app.context_set_mode(None, cx)).ok();
                }),
        );

    let mut body = v_flex().gap_2().child(scope).child(mode_row);
    for (field, input) in &state.context_inputs {
        let field = *field;
        let weak = weak.clone();
        body = body.child(
            h_flex()
                .gap_2()
                .items_center()
                .child(crate::hints::tip(div().id(("ctx-label", field as usize)).w(px(250.)).child(Label::new(field.label())), field.help()))
                .child(div().flex_1().child(Input::new(input)))
                .child(Button::new(("ctx-reset", field as usize)).label("inherit").small().ghost().on_click(move |_, window, cx| {
                    weak.update(cx, |app, cx| app.context_reset(field, window, cx)).ok();
                })),
        );
    }
    let (w_all, w_promote) = (weak.clone(), weak.clone());
    body.child(
        h_flex().gap_2().items_center().child(
            Button::new("ctx-promote")
                .label("Make session settings the default")
                .small()
                .tooltip("This session's context values become the default for new sessions, and the session goes back to inheriting")
                .on_click(move |_, window, cx| {
                    w_promote.update(cx, |app, cx| app.context_promote(window, cx)).ok();
                }),
        ).child(
            Button::new("ctx-reset-all")
                .label(if session { "Use the defaults" } else { "Reset to built-in defaults" })
                .small()
                .tooltip(if session {
                    "Drop every value this session set: it uses the default again."
                } else {
                    "Clear the default's own values: new sessions use the built-in guess for their provider, the standard thresholds and the auto mode."
                })
                .on_click(move |_, window, cx| {
                    w_all.update(cx, |app, cx| app.context_reset_all(window, cx)).ok();
                }),
        ),
    )
    .into_any_element()
}

/// The Tools tab: which tools and toolsets the agent is told about, for this session or for the
/// machine. (Toggling individual tools is in the Tools panel and `/tool`.)
fn tools_tab(app: &ReactorApp, weak: &gpui_kit::WeakEntity<ReactorApp>) -> gpui_kit::AnyElement {
    let session = app.activation_scope.as_deref() == Some("session");
    let (w_promote, w_inherit) = (weak.clone(), weak.clone());
    v_flex()
        .gap_3()
        .child(
            h_flex().gap_1().items_center().child(Label::new(if session { "This session has its own activation" } else { "Using the machine default" })).child(crate::hints::info_button(
                "tools-info",
                "Toggling a tool or toolset starts a session-only override of the machine's default. Make it the default for new sessions, or drop it to use the default again. Individual tools are toggled in the Tools panel or with /tool.",
            )),
        )
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("tools-promote")
                        .label("Make session activation the default")
                        .small()
                        .disabled(!session)
                        .tooltip("This session's tools and toolsets become the default for new sessions")
                        .on_click(move |_, _window, cx| {
                            w_promote.update(cx, |app, cx| app.tools_promote(cx)).ok();
                        }),
                )
                .child(
                    Button::new("tools-inherit")
                        .label("Use the default activation")
                        .small()
                        .disabled(!session)
                        .tooltip("Drop this session's override: it uses the machine default again")
                        .on_click(move |_, _window, cx| {
                            w_inherit.update(cx, |app, cx| app.tools_inherit(cx)).ok();
                        }),
                ),
        )
        .into_any_element()
}
