//! The settings window (SPEC.md §6.2): gpui-kit's `Settings` component in a window of its own.
//!
//! One page per kind of setting — Fonts, Behaviour, Model, Context, Tools — each a list of
//! fields that read and write the app directly, so a change applies at once and is saved; the
//! component supplies the search, the page navigation and the per-page *reset*. This file only
//! describes the pages. The state lives on [`ReactorApp`] and every change goes through its
//! methods, so the window holds nothing of its own.

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings};
use gpui_kit::component::{ActiveTheme as _, Root, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{App, Entity, SharedString, Window, div, px, size};

use reactor_context::settings::CONTEXT_MODES;

use crate::app::ReactorApp;
use crate::layout::LayoutPreset;
use crate::settings::{MAX_SIZE, MIN_SIZE, NOTICE_SECONDS, OUTPUT_CHARS, Slot};

/// What the model page's buttons do with `provider/name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelUse {
    Default,
    Now,
    List,
}

/// The provider and name typed on the model page.
#[derive(Debug, Clone)]
pub struct ModelDraft {
    pub provider: String,
    pub name: String,
}

impl Default for ModelDraft {
    fn default() -> Self {
        ModelDraft { provider: reactor_agent::provider::PROVIDERS[0].to_string(), name: String::new() }
    }
}

/// Which layer of the context settings the Context page edits (ADR-0038).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextScope {
    /// `settings.json`: what every new session starts with.
    Default,
    /// This session only.
    Session,
}

/// Open the settings window, or bring the open one forward.
pub fn open(cx: &mut App, app: Entity<ReactorApp>) {
    if let Some(handle) = app.read(cx).settings_window
        && handle.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return;
    }
    let bounds = gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds::centered(None, size(px(980.), px(720.)), cx));
    let for_window = app.clone();
    let opened = cx.open_window(crate::chrome::window_options(bounds), move |window, cx| {
        crate::theme::install_ayu_dark(cx);
        let view = cx.new(|cx| SettingsWindow::new(for_window, cx));
        let root: gpui_kit::AnyView = view.into();
        cx.new(|cx| Root::new(root, window, cx))
    });
    if let Ok(handle) = opened {
        app.update(cx, |app, _| app.settings_window = Some(handle.into()));
        cx.activate(true);
    }
}

struct SettingsWindow {
    app: Entity<ReactorApp>,
    _watch: gpui_kit::Subscription,
}

impl SettingsWindow {
    fn new(app: Entity<ReactorApp>, cx: &mut Context<Self>) -> Self {
        // The fields show the app's state, so they follow it when it changes (a notification, a
        // model switch made in the main window).
        let _watch = cx.observe(&app, |_, _, cx| cx.notify());
        Self { app, _watch }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.clone();
        // The component's notification layer, so popups (this window gets the same ones) show here.
        let notifications = Root::render_notification_layer(window, cx);
        v_flex()
            .size_full()
            .relative()
            .child(crate::chrome::title_bar("REactor — Settings", None, None, cx))
            .child(div().flex_1().min_h_0().child(
                Settings::new("reactor-settings").sidebar_width(px(220.)).pages(vec![fonts_page(&app, cx), behaviour_page(&app), model_page(&app, cx), context_page(&app), tools_page(&app)]),
            ))
            .children(notifications)
    }
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

type Pages = SettingPage;

fn info(page: Pages, id: &'static str, text: &'static str) -> Pages {
    page.title_suffix(move |_, _| crate::hints::info_button(id, text))
}

fn shared(s: impl Into<String>) -> SharedString {
    SharedString::from(s.into())
}

/// A button on the settings window: its label, its tooltip, and what it does to the app.
type Action = (&'static str, &'static str, fn(&mut ReactorApp, &mut Context<ReactorApp>));

/// A row with buttons, for actions that are not a value. Each button runs `run` on the app.
fn actions(title: &'static str, buttons: Vec<Action>, app: &Entity<ReactorApp>) -> SettingItem {
    let app = app.clone();
    SettingItem::render(move |_, _window, _cx| {
        let mut row = h_flex().gap_2().items_center().flex_wrap().child(div().w(px(160.)).child(title));
        for (label, tip, run) in buttons.clone() {
            let app = app.clone();
            row = row.child(Button::new(SharedString::from(format!("{title}-{label}"))).label(label).small().tooltip(tip).on_click(move |_, _w, cx| {
                app.update(cx, run);
            }));
        }
        row
    })
}

fn sized(min: f64, max: f64, step: f64) -> NumberFieldOptions {
    NumberFieldOptions { min, max, step }
}

// ---------------------------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------------------------

fn fonts_page(app: &Entity<ReactorApp>, cx: &App) -> SettingPage {
    let mut group = SettingGroup::new();
    let ui = app.read(cx).ui.clone();
    for slot in Slot::ALL {
        let default_size = f64::from(crate::settings::default_size(slot, &ui, cx));
        let (a_get, a_set, a_reset_dirty, a_reset) = (app.clone(), app.clone(), app.clone(), app.clone());
        group = group.item(
            SettingItem::new(
                format!("{} — family", slot.label()),
                SettingField::<SharedString>::input(
                    move |cx| shared(a_get.read(cx).family_text(slot)),
                    move |text, cx| {
                        a_set.update(cx, |app, cx| app.set_family_text(slot, &text, cx));
                    },
                )
                .on_reset(move |cx| a_reset_dirty.read(cx).ui.family(slot).is_some(), move |_, cx| {
                    a_reset.update(cx, |app, cx| app.set_family_text(slot, "", cx));
                }),
            )
            .keywords(["font", "typeface"]),
        );
        let (s_get, s_set, s_dirty, s_reset) = (app.clone(), app.clone(), app.clone(), app.clone());
        group = group.item(
            SettingItem::new(
                format!("{} — size", slot.label()),
                SettingField::<f64>::number_input(
                    sized(f64::from(MIN_SIZE), f64::from(MAX_SIZE), 1.0),
                    move |cx| {
                        let app = s_get.read(cx);
                        f64::from(app.ui.size(slot).unwrap_or_else(|| crate::settings::default_size(slot, &app.ui, cx)))
                    },
                    move |size, cx| {
                        s_set.update(cx, |app, cx| {
                            let default = f64::from(crate::settings::default_size(slot, &app.ui, cx));
                            app.set_font_size(slot, size, default, cx);
                        });
                    },
                )
                .default_value(default_size)
                .on_reset(move |cx| s_dirty.read(cx).ui.size(slot).is_some(), move |_, cx| {
                    s_reset.update(cx, |app, cx| app.update_ui(|ui| ui.set_size(slot, None), cx));
                }),
            )
            .keywords(["font", "size", "pixels"]),
        );
    }
    info(
        SettingPage::new("Fonts").icon(IconName::MessageSquare).group(group.title("Fonts and sizes")),
        "fonts-info",
        "A family is an installed font's name; empty is the default. Sizes are in pixels. Interface and Monospace are the theme's base fonts; the rest follow them until set. Saved in ~/.reactor/gui.json for every session.",
    )
}

// ---------------------------------------------------------------------------------------------
// Behaviour
// ---------------------------------------------------------------------------------------------

fn behaviour_page(app: &Entity<ReactorApp>) -> SettingPage {
    let (t_get, t_set) = (app.clone(), app.clone());
    let (d_get, d_set) = (app.clone(), app.clone());
    let (o_get, o_set) = (app.clone(), app.clone());
    let (n_get, n_set) = (app.clone(), app.clone());
    let (l_get, l_set) = (app.clone(), app.clone());
    let standard = crate::settings::UiSettings::default();
    let layouts: Vec<(SharedString, SharedString)> = LayoutPreset::ALL.iter().map(|p| (shared(p.label().to_ascii_lowercase()), shared(p.label()))).collect();
    let group = SettingGroup::new()
        .item(SettingItem::new(
            "Open thinking blocks by default",
            SettingField::<bool>::switch(move |cx| t_get.read(cx).ui.expand_thinking, move |v, cx| {
                t_set.update(cx, |app, cx| app.update_ui(|ui| ui.expand_thinking = v, cx));
            })
            .default_value(standard.expand_thinking),
        ))
        .item(SettingItem::new(
            "Dim what a reduction took out of view",
            SettingField::<bool>::switch(move |cx| d_get.read(cx).ui.dim_reduced, move |v, cx| {
                d_set.update(cx, |app, cx| app.update_ui(|ui| ui.dim_reduced = v, cx));
            })
            .default_value(standard.dim_reduced),
        ))
        .item(SettingItem::new(
            "Tool output shown (characters)",
            SettingField::<f64>::number_input(
                sized(*OUTPUT_CHARS.start() as f64, *OUTPUT_CHARS.end() as f64, 500.0),
                move |cx| o_get.read(cx).ui.tool_output_chars as f64,
                move |v, cx| {
                    o_set.update(cx, |app, cx| app.update_ui(|ui| ui.tool_output_chars = v.round().max(0.0) as usize, cx));
                },
            )
            .default_value(standard.tool_output_chars as f64),
        ))
        .item(SettingItem::new(
            "Notification popups stay (seconds)",
            SettingField::<f64>::number_input(
                sized(f64::from(*NOTICE_SECONDS.start()), f64::from(*NOTICE_SECONDS.end()), 1.0),
                move |cx| f64::from(n_get.read(cx).ui.notice_seconds),
                move |v, cx| {
                    n_set.update(cx, |app, cx| app.update_ui(|ui| ui.notice_seconds = v.round().max(0.0) as u32, cx));
                },
            )
            .default_value(f64::from(standard.notice_seconds)),
        ))
        .item(SettingItem::new(
            "Layout a window opens with",
            SettingField::<SharedString>::dropdown(layouts, move |cx| shared(l_get.read(cx).ui.start_layout.clone()), move |v, cx| {
                l_set.update(cx, |app, cx| app.update_ui(|ui| ui.start_layout = v.to_string(), cx));
            })
            .default_value(shared(standard.start_layout.clone())),
        ));
    info(
        SettingPage::new("Behaviour").icon(IconName::LayoutDashboard).group(group.title("Behaviour")),
        "behaviour-info",
        "Only the transcript is cut to the tool output length; the session log keeps all of it. Every notification is also kept behind the bell in the title bar. Saved in ~/.reactor/gui.json for every session.",
    )
}

// ---------------------------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------------------------

fn key_variable(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "ANTHROPIC_API_KEY",
        "openai" => "OPENAI_API_KEY",
        "gemini" => "GEMINI_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        _ => "OLLAMA_API_BASE_URL",
    }
}

fn model_page(app: &Entity<ReactorApp>, cx: &App) -> SettingPage {
    let (default_model, models) = app.read(cx).model_settings();
    let mut options: Vec<(SharedString, SharedString)> = vec![(shared(""), shared("none"))];
    options.extend(models.iter().map(|m| (shared(m.clone()), shared(m.clone()))));
    if let Some(d) = &default_model
        && !models.contains(d)
    {
        options.push((shared(d.clone()), shared(d.clone())));
    }
    let (d_get, d_set) = (app.clone(), app.clone());
    let (p_get, p_set) = (app.clone(), app.clone());
    let (n_get, n_set) = (app.clone(), app.clone());

    let providers: Vec<(SharedString, SharedString)> = reactor_agent::provider::PROVIDERS.iter().map(|p| (shared(*p), shared(*p))).collect();
    let provider = app.read(cx).model_draft.provider.clone();
    let variable = key_variable(&provider);
    let key_ok = provider == "ollama" || std::env::var_os(variable).is_some();
    let key_text = if provider == "ollama" { "local".to_string() } else if key_ok { format!("{variable} set") } else { format!("{variable} not set") };
    let key_help = if provider == "ollama" {
        format!("Ollama runs on this machine and needs no key. {variable} overrides its address.")
    } else {
        format!("REactor reads the key from {variable} in the environment and never stores it. Set it before starting the GUI.")
    };

    let list_app = app.clone();
    let listed = models.clone();
    let group = SettingGroup::new()
        .title("Default model")
        .item(SettingItem::new(
            "Default model",
            SettingField::<SharedString>::dropdown(
                options,
                move |cx| shared(d_get.read(cx).model_settings().0.unwrap_or_default()),
                move |v, cx| {
                    d_set.update(cx, |app, cx| app.set_default_model((!v.is_empty()).then(|| v.to_string()), cx));
                },
            )
            .default_value(shared("")),
        ))
        .item(actions(
            "This session",
            vec![("Make its model the default", "The model this session is using becomes the default for new sessions", |app, cx| app.promote_model(cx))],
            app,
        ));
    let add = SettingGroup::new()
        .title("Add a model")
        .item(SettingItem::new(
            "Provider",
            SettingField::<SharedString>::dropdown(providers, move |cx| shared(p_get.read(cx).model_draft.provider.clone()), move |v, cx| {
                p_set.update(cx, |app, cx| {
                    app.model_draft.provider = v.to_string();
                    cx.notify();
                });
            })
            .default_value(shared(reactor_agent::provider::PROVIDERS[0])),
        ))
        .item(SettingItem::render(move |_, _w, cx| {
            h_flex().gap_2().child(div().w(px(160.)).child("API key")).child(crate::hints::tip(
                div().id("key-status").text_color(if key_ok { cx.theme().muted_foreground } else { cx.theme().warning }).child(key_text.clone()),
                key_help.clone(),
            ))
        }))
        .item(SettingItem::new(
            "Model name",
            SettingField::<SharedString>::input(move |cx| shared(n_get.read(cx).model_draft.name.clone()), move |v, cx| {
                n_set.update(cx, |app, cx| {
                    app.model_draft.name = v.to_string();
                    cx.notify();
                });
            }),
        ))
        .item(actions(
            "Use it",
            vec![
                ("Set as default", "provider/name becomes the default for new sessions", |app, cx| app.set_typed_model(ModelUse::Default, cx)),
                ("Use in this session", "Switch this session to it now", |app, cx| app.set_typed_model(ModelUse::Now, cx)),
                ("Add to picker list", "Offer it in the status bar's model picker", |app, cx| app.set_typed_model(ModelUse::List, cx)),
            ],
            app,
        ));
    let picker = SettingGroup::new().title("Models in the picker").item(SettingItem::render(move |_, _w, cx| {
        let mut rows = v_flex().gap_1();
        if listed.is_empty() {
            rows = rows.child(div().text_color(cx.theme().muted_foreground).child("none"));
        }
        for spec in &listed {
            let (app, name) = (list_app.clone(), spec.clone());
            let (app_use, name_use) = (list_app.clone(), spec.clone());
            rows = rows.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().font_family(cx.theme().mono_font_family.clone()).text_size(cx.theme().mono_font_size).child(spec.clone()))
                    .child(Button::new(shared(format!("use-{spec}"))).label("use now").small().on_click(move |_, _w, cx| {
                        let spec = name_use.clone();
                        app_use.update(cx, |app, cx| app.set_model(&spec, cx));
                    }))
                    .child(Button::new(shared(format!("remove-{spec}"))).label("remove").small().ghost().on_click(move |_, _w, cx| {
                        let spec = name.clone();
                        app.update(cx, |app, cx| app.unlist_model(&spec, cx));
                    })),
            );
        }
        rows
    }));
    info(
        SettingPage::new("Model").icon(IconName::Server).groups([group, add, picker]),
        "model-info",
        "Saved in ~/.reactor/settings.json, which the agent reads too. The context budget is on the Context page.",
    )
}

// ---------------------------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------------------------

fn context_page(app: &Entity<ReactorApp>) -> SettingPage {
    // The layer being edited, and the effective values behind an unset field.
    let layer = |app: &Entity<ReactorApp>, cx: &App| {
        let a = app.read(cx);
        a.context_layer(a.context_scope)
    };
    let now = |app: &Entity<ReactorApp>, cx: &App| app.read(cx).context.clone();

    let (s_get, s_set) = (app.clone(), app.clone());
    let scope_group = SettingGroup::new().title("Editing").item(SettingItem::new(
        "Edit",
        SettingField::<SharedString>::dropdown(
            vec![(shared("default"), shared("Default for new sessions")), (shared("session"), shared("This session only"))],
            move |cx| shared(if s_get.read(cx).context_scope == ContextScope::Session { "session" } else { "default" }),
            move |v, cx| {
                s_set.update(cx, |app, cx| {
                    app.context_scope = if v.as_ref() == "session" { ContextScope::Session } else { ContextScope::Default };
                    cx.notify();
                });
            },
        )
        .default_value(shared("default")),
    ));

    // One numeric field: its stored value when the layer has one, else what is in effect now.
    macro_rules! number {
        ($title:expr, $min:expr, $max:expr, $step:expr, $field:ident, $effective:expr, $to_layer:expr, $from_layer:expr) => {{
            let (g, st, d, r) = (app.clone(), app.clone(), app.clone(), app.clone());
            SettingItem::new(
                $title,
                SettingField::<f64>::number_input(
                    sized($min, $max, $step),
                    move |cx| match layer(&g, cx).$field {
                        Some(v) => ($from_layer)(v),
                        None => now(&g, cx).map($effective).unwrap_or(0.0),
                    },
                    move |v, cx| {
                        st.update(cx, |app, cx| app.context_edit(|l| l.$field = Some(($to_layer)(v)), cx));
                    },
                )
                .on_reset(move |cx| layer(&d, cx).$field.is_some(), move |_, cx| {
                    r.update(cx, |app, cx| app.context_edit(|l| l.$field = None, cx));
                }),
            )
        }};
    }

    let (m_get, m_set, m_dirty, m_reset) = (app.clone(), app.clone(), app.clone(), app.clone());
    let (u_get, u_set, u_dirty, u_reset) = (app.clone(), app.clone(), app.clone(), app.clone());
    let budget = SettingGroup::new()
        .title("Budget")
        .item(SettingItem::new(
            "Reduction mode",
            SettingField::<SharedString>::dropdown(
                CONTEXT_MODES.iter().map(|m| (shared(*m), shared(*m))).collect(),
                move |cx| {
                    let a = m_get.read(cx);
                    shared(a.context_layer(a.context_scope).mode.or_else(|| a.context.as_ref().map(|c| c.mode.clone())).unwrap_or_default())
                },
                move |v, cx| {
                    m_set.update(cx, |app, cx| app.context_edit(|l| l.mode = Some(v.to_string()), cx));
                },
            )
            .on_reset(move |cx| layer(&m_dirty, cx).mode.is_some(), move |_, cx| {
                m_reset.update(cx, |app, cx| app.context_edit(|l| l.mode = None, cx));
            }),
        ))
        .item(number!("Context window (tokens)", 1000.0, 10_000_000.0, 1000.0, window, |c: crate::backend::ContextView| c.window as f64, |v: f64| v.round() as u64, |v: u64| v as f64))
        .item(number!("Reserve (tokens)", 0.0, 1_000_000.0, 500.0, reserve, |c: crate::backend::ContextView| c.reserve as f64, |v: f64| v.round() as u64, |v: u64| v as f64))
        .item(number!("Reduce at (%)", 1.0, 100.0, 1.0, pct, |c: crate::backend::ContextView| (c.pct * 100.0).round(), |v: f64| v / 100.0, |v: f64| (v * 100.0).round()))
        .item(number!("Keep after reducing (%)", 1.0, 99.0, 1.0, keep, |c: crate::backend::ContextView| (c.keep * 100.0).round(), |v: f64| v / 100.0, |v: f64| (v * 100.0).round()))
        .item(SettingItem::new(
            "Summarizer (provider/name)",
            SettingField::<SharedString>::input(
                move |cx| {
                    let a = u_get.read(cx);
                    shared(a.context_layer(a.context_scope).summarizer.unwrap_or_default())
                },
                move |v, cx| {
                    // Half a name is not a model: only provider/name is stored; empty clears.
                    let v = v.trim().to_string();
                    if v.is_empty() {
                        u_set.update(cx, |app, cx| app.context_edit(|l| l.summarizer = None, cx));
                    } else if v.contains('/') && !v.ends_with('/') {
                        u_set.update(cx, |app, cx| app.context_edit(|l| l.summarizer = Some(v), cx));
                    }
                },
            )
            .on_reset(move |cx| layer(&u_dirty, cx).summarizer.is_some(), move |_, cx| {
                u_reset.update(cx, |app, cx| app.context_edit(|l| l.summarizer = None, cx));
            }),
        ))
        .item(actions(
            "This session",
            vec![("Make its settings the default", "This session's context values become the default for new sessions, and the session goes back to inheriting", |app, cx| app.context_promote(cx))],
            app,
        ));
    info(
        SettingPage::new("Context").icon(IconName::Layers).group(scope_group).group(budget),
        "context-info",
        "The context window is not asked of the provider: set it for your model (REactor guesses 200,000 for anthropic, 1,000,000 for gemini, 32,000 for ollama, 128,000 otherwise). Usable context is the window minus the reserve. A reduction starts once the context passes 'Reduce at' of that and goes down to 'Keep', keeping the newest work. A field that is not set here shows what this session uses now; reset clears it, so the session uses the default and the default the built-in value. Default values are saved in ~/.reactor/settings.json; a session's own are kept in the session.",
    )
}

// ---------------------------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------------------------

fn tools_page(app: &Entity<ReactorApp>) -> SettingPage {
    let group = SettingGroup::new()
        .item(actions(
            "Activation",
            vec![
                ("Make session's the default", "This session's tools and toolsets become the default for new sessions", |app, cx| app.tools_promote(cx)),
                ("Use the default", "Drop this session's override: it uses the machine default again", |app, cx| app.tools_inherit(cx)),
            ],
            app,
        ));
    info(
        SettingPage::new("Tools").icon(IconName::Wrench).group(group),
        "tools-info",
        "Toggling a tool or toolset starts a session-only override of the machine's default (~/.reactor/state.json). Individual tools are toggled in the Tools panel or with /tool.",
    )
}
