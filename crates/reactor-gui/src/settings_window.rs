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
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::setting::{
    NumberFieldOptions, SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::{ActiveTheme as _, Root, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{App, Entity, Focusable as _, SharedString, Window, div, px, size};

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
        ModelDraft {
            provider: reactor_agent::provider::PROVIDERS[0].to_string(),
            name: String::new(),
        }
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

/// The pages of the settings window, in the order it lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Fonts,
    Behaviour,
    Shortcuts,
    Model,
    Identities,
    Documents,
    Context,
    Manifest,
    Reporting,
    Tools,
    Mcp,
}

impl Page {
    pub fn index(self) -> usize {
        match self {
            Page::Fonts => 0,
            Page::Behaviour => 1,
            Page::Shortcuts => 2,
            Page::Model => 3,
            Page::Identities => 4,
            Page::Documents => 5,
            Page::Context => 6,
            Page::Manifest => 7,
            Page::Reporting => 8,
            Page::Tools => 9,
            Page::Mcp => 10,
        }
    }
}

/// Open the settings window, or bring the open one forward.
pub fn open(cx: &mut App, app: Entity<ReactorApp>) {
    if let Some(handle) = app.read(cx).settings_window
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    open_at(cx, app, Page::default());
}

/// Open the settings window on a page. The caller has closed any that is open.
pub fn open_at(cx: &mut App, app: Entity<ReactorApp>, page: Page) {
    let bounds = gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds::centered(
        None,
        size(px(980.), px(720.)),
        cx,
    ));
    let for_window = app.clone();
    let opened = cx.open_window(crate::chrome::window_options(bounds), move |window, cx| {
        crate::theme::install_ayu_dark(cx);
        let view = cx.new(|cx| SettingsWindow::new(for_window, page, window, cx));
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
    /// The Model page: its fields and what each provider's endpoint offered live here, so they
    /// outlast a redraw of the page.
    cards: Entity<crate::model_cards::ModelCards>,
    embedding_cards: Entity<crate::model_cards::EmbeddingCards>,
    /// The page it opened on.
    page: Page,
    _watch: gpui_kit::Subscription,
}

impl SettingsWindow {
    fn new(
        app: Entity<ReactorApp>,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let cards = {
            let app = app.clone();
            cx.new(|cx| crate::model_cards::ModelCards::new(app, window, cx))
        };
        let embedding_cards = {
            let cards = cards.clone();
            cx.new(|_| crate::model_cards::EmbeddingCards::new(cards))
        };
        // The fields show the app's state, so they follow it when it changes (a notification, a
        // model switch made in the main window).
        let _watch = cx.observe(&app, |_, _, cx| cx.notify());
        Self {
            app,
            page,
            cards,
            embedding_cards,
            _watch,
        }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.clone();
        v_flex()
            .size_full()
            .relative()
            .child(crate::chrome::title_bar(
                "REactor — Settings",
                None,
                None,
                cx,
            ))
            .child(
                div().flex_1().min_h_0().child(
                    Settings::new("reactor-settings")
                        .sidebar_width(px(220.))
                        .default_selected_index(SelectIndex {
                            page_ix: self.page.index(),
                            group_ix: None,
                        })
                        .pages(vec![
                            fonts_page(&app, cx),
                            behaviour_page(&app),
                            shortcuts_page(&app, cx),
                            model_page(&app, &self.cards),
                            identities_page(&app, cx),
                            documents_page(&app, &self.embedding_cards, cx),
                            context_page(&app),
                            manifest_page(&app),
                            reporting_page(&app),
                            tools_page(&app),
                            mcp_page(&app, cx),
                        ]),
                ),
            )
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
type Action = (
    &'static str,
    &'static str,
    fn(&mut ReactorApp, &mut Context<ReactorApp>),
);

/// A row with buttons, for actions that are not a value. Each button runs `run` on the app.
fn actions(title: &'static str, buttons: Vec<Action>, app: &Entity<ReactorApp>) -> SettingItem {
    let app = app.clone();
    SettingItem::render(move |_, _window, _cx| {
        let mut row = h_flex()
            .gap_2()
            .items_center()
            .flex_wrap()
            .child(div().w(px(160.)).child(title));
        for (label, tip, run) in buttons.clone() {
            let app = app.clone();
            row = row.child(
                Button::new(SharedString::from(format!("{title}-{label}")))
                    .label(label)
                    .small()
                    .tooltip(tip)
                    .on_click(move |_, _w, cx| {
                        app.update(cx, run);
                    }),
            );
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
        let (a_get, a_set, a_reset_dirty, a_reset) =
            (app.clone(), app.clone(), app.clone(), app.clone());
        group = group.item(
            SettingItem::new(
                format!("{} — family", slot.label()),
                SettingField::<SharedString>::input(
                    move |cx| shared(a_get.read(cx).family_text(slot)),
                    move |text, cx| {
                        a_set.update(cx, |app, cx| app.set_family_text(slot, &text, cx));
                    },
                )
                .on_reset(
                    move |cx| a_reset_dirty.read(cx).ui.family(slot).is_some(),
                    move |_, cx| {
                        a_reset.update(cx, |app, cx| app.set_family_text(slot, "", cx));
                    },
                ),
            )
            .keywords(["font", "typeface"]),
        );
        let (s_get, s_set, s_dirty, s_reset) = (app.clone(), app.clone(), app.clone(), app.clone());
        group =
            group.item(
                SettingItem::new(
                    format!("{} — size", slot.label()),
                    SettingField::<f64>::number_input(
                        sized(f64::from(MIN_SIZE), f64::from(MAX_SIZE), 1.0),
                        move |cx| {
                            let app = s_get.read(cx);
                            f64::from(app.ui.size(slot).unwrap_or_else(|| {
                                crate::settings::default_size(slot, &app.ui, cx)
                            }))
                        },
                        move |size, cx| {
                            s_set.update(cx, |app, cx| {
                                let default =
                                    f64::from(crate::settings::default_size(slot, &app.ui, cx));
                                app.set_font_size(slot, size, default, cx);
                            });
                        },
                    )
                    .default_value(default_size)
                    .on_reset(
                        move |cx| s_dirty.read(cx).ui.size(slot).is_some(),
                        move |_, cx| {
                            s_reset.update(cx, |app, cx| {
                                app.update_ui(|ui| ui.set_size(slot, None), cx)
                            });
                        },
                    ),
                )
                .keywords(["font", "size", "pixels"]),
            );
    }
    info(
        SettingPage::new("Fonts")
            .icon(IconName::MessageSquare)
            .group(group.title("Fonts and sizes")),
        "fonts-info",
        "A family is an installed font's name; empty is the default. Sizes are in pixels. Interface and Monospace are the theme's base fonts; the rest follow them until set. Saved in ~/.reactor/gui.json for every session.",
    )
}

// ---------------------------------------------------------------------------------------------
// Shortcuts
// ---------------------------------------------------------------------------------------------

/// A shortcut's field: an input that applies what is in it on Enter or when it loses the
/// focus — never while it is typed, when `c` on the way to `ctrl-k` would be bound — and
/// otherwise shows the keys in force.
fn shortcut_field(
    app: Entity<ReactorApp>,
    shortcut: crate::shortcuts::Shortcut,
) -> SettingField<SharedString> {
    struct State {
        input: Entity<InputState>,
        _subscription: gpui_kit::Subscription,
    }
    SettingField::render(move |options, window, cx| {
        let keys = shared(shortcut.keys(&app.read(cx).ui.shortcuts));
        let state = window.use_keyed_state(
            SharedString::from(format!("shortcut-{}", shortcut.id)),
            cx,
            {
                let (app, keys) = (app.clone(), keys.clone());
                move |window, cx| {
                    let input = cx.new(|cx| InputState::new(window, cx).default_value(keys));
                    let _subscription =
                        cx.subscribe(&input, move |_, input, event: &InputEvent, cx| {
                            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                                let text = input.read(cx).value().to_string();
                                app.update(cx, |app, cx| app.commit_shortcut(shortcut, &text, cx));
                            }
                        });
                    State {
                        input,
                        _subscription,
                    }
                }
            },
        );
        // Follow the setting (a reset, a clash resolved elsewhere) unless it is being typed
        // in, or holds keys that were refused and are still to be corrected.
        let input = state.read(cx).input.clone();
        let typing = input.read(cx).focus_handle(cx).is_focused(window);
        let refused = app.read(cx).shortcut_refused(shortcut);
        if !typing && !refused && input.read(cx).value() != keys {
            input.update(cx, |input, cx| input.set_value(keys, window, cx));
        }
        Input::new(&input).with_size(options.size()).w_64()
    })
}

fn shortcuts_page(app: &Entity<ReactorApp>, cx: &App) -> SettingPage {
    let (mut panels, mut commands) = (
        SettingGroup::new().title("Panels"),
        SettingGroup::new().title("Commands"),
    );
    for shortcut in crate::shortcuts::ALL {
        let (dirty, reset) = (app.clone(), app.clone());
        let mut item = SettingItem::new(
            shortcut.label,
            shortcut_field(app.clone(), shortcut).on_reset(
                move |cx| dirty.read(cx).ui.shortcuts.contains_key(shortcut.id),
                move |_, cx| reset.update(cx, |app, cx| app.reset_shortcut(shortcut, cx)),
            ),
        )
        .keywords(["shortcut", "key", "keyboard", "binding"]);
        if let Some(problem) = app.read(cx).shortcut_problem(shortcut) {
            item = item.description(problem);
        }
        if shortcut.id.starts_with("panel.") {
            panels = panels.item(item);
        } else {
            commands = commands.item(item);
        }
    }
    info(
        SettingPage::new("Shortcuts")
            .icon(IconName::Keyboard)
            .group(panels)
            .group(commands),
        "shortcuts-info",
        "Keys are written like ctrl-shift-k: modifiers (ctrl, alt, shift, cmd, fn, or secondary for Cmd on macOS and Ctrl elsewhere) joined to a key by dashes, and a sequence separated by spaces; Enter applies them. A shortcut needs ctrl, alt, cmd or fn, or a function key (f1 to f24), so it does not take a key from text fields. Empty means no shortcut. A panel's shortcut shows it, or closes it when it is already in view. Saved in ~/.reactor/gui.json.",
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
    let (a_get, a_set) = (app.clone(), app.clone());
    let (r_get, r_set) = (app.clone(), app.clone());
    let standard = crate::settings::UiSettings::default();
    let renderings: Vec<(SharedString, SharedString)> = crate::settings::Rendering::ALL
        .iter()
        .map(|r| (shared(r.key()), shared(r.label())))
        .collect();
    let layouts: Vec<(SharedString, SharedString)> = LayoutPreset::ALL
        .iter()
        .map(|p| (shared(p.label().to_ascii_lowercase()), shared(p.label())))
        .collect();
    let group = SettingGroup::new()
        .item(SettingItem::new(
            "Open thinking blocks by default",
            SettingField::<bool>::switch(
                move |cx| t_get.read(cx).ui.expand_thinking,
                move |v, cx| {
                    t_set.update(cx, |app, cx| app.update_ui(|ui| ui.expand_thinking = v, cx));
                },
            )
            .default_value(standard.expand_thinking),
        ))
        .item(SettingItem::new(
            "Dim what a reduction took out of view",
            SettingField::<bool>::switch(
                move |cx| d_get.read(cx).ui.dim_reduced,
                move |v, cx| {
                    d_set.update(cx, |app, cx| app.update_ui(|ui| ui.dim_reduced = v, cx));
                },
            )
            .default_value(standard.dim_reduced),
        ))
        .item(SettingItem::new(
            "Tool output shown (characters)",
            SettingField::<f64>::number_input(
                sized(
                    *OUTPUT_CHARS.start() as f64,
                    *OUTPUT_CHARS.end() as f64,
                    500.0,
                ),
                move |cx| o_get.read(cx).ui.tool_output_chars as f64,
                move |v, cx| {
                    o_set.update(cx, |app, cx| {
                        app.update_ui(|ui| ui.tool_output_chars = v.round().max(0.0) as usize, cx)
                    });
                },
            )
            .default_value(standard.tool_output_chars as f64),
        ))
        .item(SettingItem::new(
            "Notification popups stay (seconds)",
            SettingField::<f64>::number_input(
                sized(
                    f64::from(*NOTICE_SECONDS.start()),
                    f64::from(*NOTICE_SECONDS.end()),
                    1.0,
                ),
                move |cx| f64::from(n_get.read(cx).ui.notice_seconds),
                move |v, cx| {
                    n_set.update(cx, |app, cx| {
                        app.update_ui(|ui| ui.notice_seconds = v.round().max(0.0) as u32, cx)
                    });
                },
            )
            .default_value(f64::from(standard.notice_seconds)),
        ))
        .item(SettingItem::new(
            "Animate dialogs, menus and popups",
            SettingField::<bool>::switch(
                move |cx| a_get.read(cx).ui.animations,
                move |v, cx| {
                    a_set.update(cx, |app, cx| app.update_ui(|ui| ui.animations = v, cx));
                },
            )
            .default_value(standard.animations),
        ))
        .item(SettingItem::new(
            "Rendering",
            SettingField::<SharedString>::dropdown(
                renderings,
                move |cx| shared(r_get.read(cx).ui.rendering.key()),
                move |v, cx| {
                    r_set.update(cx, |app, cx| {
                        app.update_ui(
                            |ui| ui.rendering = crate::settings::Rendering::from_key(&v),
                            cx,
                        )
                    });
                },
            )
            .default_value(shared(standard.rendering.key())),
        ))
        .item(SettingItem::new(
            "Layout a window opens with",
            SettingField::<SharedString>::dropdown(
                layouts,
                move |cx| shared(l_get.read(cx).ui.start_layout.clone()),
                move |v, cx| {
                    l_set.update(cx, |app, cx| {
                        app.update_ui(|ui| ui.start_layout = v.to_string(), cx)
                    });
                },
            )
            .default_value(shared(standard.start_layout.clone())),
        ));
    info(
        SettingPage::new("Behaviour")
            .icon(IconName::LayoutDashboard)
            .group(group.title("Behaviour")),
        "behaviour-info",
        "Only the transcript is cut to the tool output length; the session log keeps all of it. Every notification is also kept behind the bell in the title bar. Animations are off by default so the next key press never waits for a transition; the working indicators keep moving either way. Rendering: Light drops shadows and redraws the working indicators less often, which is what a machine without graphics acceleration (a VM, mostly) wants; Automatic turns it on when the renderer is a software one. Saved in ~/.reactor/gui.json for every session.",
    )
}

// ---------------------------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------------------------

fn model_page(
    app: &Entity<ReactorApp>,
    cards: &Entity<crate::model_cards::ModelCards>,
) -> SettingPage {
    let cards = cards.clone();
    let (t_get, t_set) = (app.clone(), app.clone());
    let thinking = SettingGroup::new().title("Thinking").item(SettingItem::new(
        "Default thinking level",
        SettingField::<SharedString>::dropdown(
            vec![
                (shared(""), shared("Default (the model decides)")),
                (shared("off"), shared("Off")),
                (shared("on"), shared("On")),
                (shared("low"), shared("Low")),
                (shared("medium"), shared("Medium")),
                (shared("high"), shared("High")),
            ],
            move |cx| shared(t_get.read(cx).default_thinking()),
            move |v, cx| {
                t_set.update(cx, |app, cx| app.set_default_thinking(v.as_ref(), cx));
            },
        )
        .default_value(shared("")),
    ));
    info(
        SettingPage::new("Model")
            .icon(IconName::Server)
            .group(thinking)
            .group(SettingGroup::new().item(SettingItem::render(move |_, _w, _cx| cards.clone()))),
        "model-info",
        "One card per provider. The endpoint and key are environment variables (the card's tooltips name them); typed values can also be kept in ~/.reactor/env. Models are listed by the endpoint itself; a click puts one in the picker, which is saved in ~/.reactor/settings.json with the default model. The context budget is on the Context page.",
    )
}

// ---------------------------------------------------------------------------------------------
// Documents: the embedding model (ADR-0046)
// ---------------------------------------------------------------------------------------------

/// Identities: the working personas, built in and saved. Each row can be used for this
/// session, set as the default for new ones, viewed or edited in the editor window, and a
/// saved one removed (after a second press).
fn identities_page(app: &Entity<ReactorApp>, cx: &App) -> SettingPage {
    let rows = app.read(cx).identity_rows();
    let armed = app.read(cx).identity_remove_armed.clone();
    let (d_get, d_set) = (app.clone(), app.clone());
    let mut options: Vec<(SharedString, SharedString)> = vec![(shared(""), shared("none"))];
    options.extend(
        rows.iter()
            .map(|r| (shared(r.name.clone()), shared(r.name.clone()))),
    );

    let default = SettingGroup::new().title("Default").item(SettingItem::new(
        "Identity for new sessions",
        SettingField::<SharedString>::dropdown(
            options,
            move |cx| {
                shared(
                    d_get
                        .read(cx)
                        .identity_rows()
                        .iter()
                        .find(|r| r.default)
                        .map(|r| r.name.clone())
                        .unwrap_or_default(),
                )
            },
            move |v, cx| {
                d_set.update(cx, |app, cx| app.set_default_identity(v.as_ref(), cx));
            },
        )
        .default_value(shared("")),
    ));

    let list_app = app.clone();
    let list = SettingGroup::new()
        .title("Identities")
        .item(actions(
            "Add",
            vec![(
                "New identity…",
                "Write one in the editor window",
                |app, cx| {
                    app.open_identity_editor(crate::identity_editor::Start::New(String::new()), cx)
                },
            )],
            app,
        ))
        .item(SettingItem::render(move |_, _w, cx| {
            let theme = cx.theme().clone();
            let mut column = v_flex().gap_1().w_full();
            for row in &rows {
                let (name, builtin) = (row.name.clone(), row.builtin);
                let first_line: String = row
                    .text
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .chars()
                    .take(120)
                    .collect();
                let pending = armed.as_deref() == Some(row.name.as_str());
                let button = |id: String, label: &'static str, tip: &'static str| {
                    Button::new(SharedString::from(id))
                        .label(label)
                        .small()
                        .tooltip(tip)
                };
                let mut line = h_flex()
                    .gap_2()
                    .items_center()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .when(row.active, |r| r.bg(theme.info.opacity(0.15)))
                    .child(
                        div()
                            .w(px(150.))
                            .flex_none()
                            .text_color(theme.foreground)
                            .child(SharedString::from(row.name.clone())),
                    )
                    .child(
                        div()
                            .w(px(60.))
                            .flex_none()
                            .text_size(theme.font_size * 0.8)
                            .text_color(theme.muted_foreground)
                            .child(if builtin { "built-in" } else { "saved" }),
                    )
                    .child(crate::hints::tip(
                        div()
                            .id(SharedString::from(format!("identity-text-{name}")))
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(first_line)),
                        SharedString::from(row.text.chars().take(1200).collect::<String>()),
                    ))
                    .when(row.default, |r| {
                        r.child(
                            div()
                                .text_size(theme.font_size * 0.8)
                                .text_color(theme.accent)
                                .child("default"),
                        )
                    })
                    .when(row.active, |r| {
                        r.child(
                            div()
                                .text_size(theme.font_size * 0.8)
                                .text_color(theme.info)
                                .child("in use"),
                        )
                    });
                let a = list_app.clone();
                let n = name.clone();
                line = line.child(
                    button(format!("use-{name}"), "Use", "Select it for this session").on_click(
                        move |_, _w, cx| {
                            a.update(cx, |app, cx| app.identity_use(&n, cx));
                        },
                    ),
                );
                let a = list_app.clone();
                let n = name.clone();
                line = line.child(
                    button(
                        format!("edit-{name}"),
                        if builtin { "View" } else { "Edit" },
                        if builtin {
                            "Read it; saving makes a copy under a name of your own"
                        } else {
                            "Change it in the editor window"
                        },
                    )
                    .on_click(move |_, _w, cx| {
                        a.update(cx, |app, cx| {
                            app.open_identity_editor(
                                if builtin {
                                    crate::identity_editor::Start::Copy(n.clone())
                                } else {
                                    crate::identity_editor::Start::Edit(n.clone())
                                },
                                cx,
                            )
                        });
                    }),
                );
                if !builtin {
                    let a = list_app.clone();
                    let n = name.clone();
                    let remove = button(
                        format!("remove-{name}"),
                        if pending { "Yes, remove" } else { "Remove" },
                        "Delete this saved identity. It asks once more: there is no undo",
                    );
                    let remove = if pending {
                        remove.danger()
                    } else {
                        remove.ghost()
                    };
                    line = line.child(remove.on_click(move |_, _w, cx| {
                        a.update(cx, |app, cx| app.identity_remove(&n, cx));
                    }));
                    if pending {
                        let a = list_app.clone();
                        line = line.child(
                            button(format!("keep-{name}"), "Keep", "Do not remove it")
                                .ghost()
                                .on_click(move |_, _w, cx| {
                                    a.update(cx, |app, cx| app.identity_remove_cancel(cx));
                                }),
                        );
                    }
                }
                column = column.child(line);
            }
            column.into_any_element()
        }));

    info(
        SettingPage::new("Identities")
            .icon(IconName::User)
            .groups([default, list]),
        "identities-info",
        "An identity is a working persona: its text is added to the system prompt. Built-in ones ship with REactor; saved ones are yours, kept in ~/.reactor/settings.json. Use selects one for this session (the same as /identity <name>); the default is what a new session starts with. A built-in cannot be changed: View opens it as a copy to save under your own name. /identity new opens the editor, and /identity manage opens this page.",
    )
}

fn documents_page(
    app: &Entity<ReactorApp>,
    chooser: &Entity<crate::model_cards::EmbeddingCards>,
    cx: &App,
) -> SettingPage {
    let chooser = chooser.clone();
    let chosen = app.read(cx).embedding_model();
    let status = app.read(cx).corpus_status();
    let running = app.read(cx).embedding_progress();
    let armed = app.read(cx).reembed_armed;

    let state =
        SettingGroup::new()
            .title("State")
            .item(SettingItem::render(move |_, _w, cx| {
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().w(px(160.)).child("Embedding model"))
                            .child(
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(cx.theme().mono_font_size)
                                    .child(chosen.clone().unwrap_or_else(|| {
                                        "none: searching by keyword only".into()
                                    })),
                            ),
                    )
                    .children(status.lines().map(|l| {
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .text_size(cx.theme().font_size * 0.9)
                            .child(l.to_string())
                    }))
            }))
            // The run that is going: how far it has got. The page is drawn again as each
            // batch is stored.
            .item(SettingItem::render(move |_, _w, cx| match running {
                Some(p) => {
                    let percent = if p.total == 0 {
                        0.
                    } else {
                        p.embedded as f32 * 100. / p.total as f32
                    };
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().w(px(160.)).child("Making vectors"))
                                .child(format!(
                                    "{} of {} passages · {:.0}%",
                                    p.embedded, p.total, percent
                                )),
                        )
                        .child(
                            div()
                                .pl(px(168.))
                                .w(px(460.))
                                .child(Progress::new("embedding-progress").value(percent)),
                        )
                        .into_any_element()
                }
                None => div()
                    .text_color(cx.theme().muted_foreground)
                    .text_size(cx.theme().font_size * 0.9)
                    .child("No run is going")
                    .into_any_element(),
            }))
            .item(actions(
                "Run",
                if armed {
                    vec![
                        (
                            "Yes, re-embed everything",
                            "Forget this model's vectors and make them all again. Every passage is sent to the provider again, and the documents cannot be searched by meaning until it has finished",
                            |app, cx| app.reembed_confirm(cx),
                        ),
                        ("Cancel", "Keep the vectors as they are", |app, cx| {
                            app.reembed_cancel(cx)
                        }),
                    ]
                } else {
                    vec![
                        (
                            "Embed now",
                            "Make the vectors any passage is missing, or finish an interrupted run",
                            |app, cx| app.embedding_now(cx),
                        ),
                        (
                            "Re-embed everything",
                            "Forget the chosen model's vectors and make them all again: after the documents were stored differently, or the model was changed under the same name. Asks for a confirmation",
                            |app, cx| app.reembed_arm(cx),
                        ),
                        (
                            "Turn off",
                            "Search by keyword only. Vectors already made are kept",
                            |app, cx| app.embedding_off(cx),
                        ),
                    ]
                },
                app,
            ));
    let (t_get, t_set) = (app.clone(), app.clone());
    let matching = SettingGroup::new().title("Matching").item(SettingItem::new(
        "Meaning threshold",
        SettingField::<f64>::number_input(
            sized(0.0, 1.0, 0.05),
            move |cx| f64::from(t_get.read(cx).min_similarity()),
            move |v, cx| {
                t_set.update(cx, |app, cx| app.set_min_similarity(v as f32, cx));
            },
        )
        .default_value(0.0),
    ));
    let choose = SettingGroup::new()
        .title("Choose the model")
        .item(SettingItem::render(move |_, _w, _cx| chooser.clone()));
    info(
        SettingPage::new("Documents")
            .icon(IconName::BookOpen)
            .groups([state, matching, choose]),
        "documents-info",
        "The model that lets the agent search imported documents by meaning. There is no default. The cards are the Model page's: OpenAI, Gemini and Ollama, each with its endpoint and key, and the embedding models the endpoint lists (every model, with the eye button); a click chooses one and Use, at the top, starts making the vectors, because that sends every passage of the documents to the endpoint shown. With Ollama at its own address the text stays on this machine; with another endpoint it does not. Changing the model re-embeds in the background. The meaning threshold is how close in meaning (cosine similarity, 0 to 1, larger is closer) a passage must be to a question to count as a match; 0 means none, and a good value depends on the model: look at the similarity shown for a result in the Documents panel and set it just under what the results you wanted had. Saved in ~/.reactor/settings.json. Import documents with /import or Case > Import Documents.",
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
            vec![
                (shared("default"), shared("Default for new sessions")),
                (shared("session"), shared("This session only")),
            ],
            move |cx| {
                shared(if s_get.read(cx).context_scope == ContextScope::Session {
                    "session"
                } else {
                    "default"
                })
            },
            move |v, cx| {
                s_set.update(cx, |app, cx| {
                    app.context_scope = if v.as_ref() == "session" {
                        ContextScope::Session
                    } else {
                        ContextScope::Default
                    };
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
                        st.update(cx, |app, cx| {
                            app.context_edit(|l| l.$field = Some(($to_layer)(v)), cx)
                        });
                    },
                )
                .on_reset(
                    move |cx| layer(&d, cx).$field.is_some(),
                    move |_, cx| {
                        r.update(cx, |app, cx| app.context_edit(|l| l.$field = None, cx));
                    },
                ),
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
        SettingPage::new("Context")
            .icon(IconName::Layers)
            .group(scope_group)
            .group(budget),
        "context-info",
        "The context window is not asked of the provider: set it for your model (REactor guesses 200,000 for anthropic, 1,000,000 for gemini, 32,000 for ollama, 128,000 otherwise). Usable context is the window minus the reserve. A reduction starts once the context passes 'Reduce at' of that and goes down to 'Keep', keeping the newest work. A field that is not set here shows what this session uses now; reset clears it, so the session uses the default and the default the built-in value. Default values are saved in ~/.reactor/settings.json; a session's own are kept in the session.",
    )
}

// ---------------------------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------
// Manifest and reporting
// ---------------------------------------------------------------------------------------------

/// A whole-number setting: a number field that reads and writes one field of the global
/// settings, with the built-in value as the reset.
fn count_field(
    app: &Entity<ReactorApp>,
    title: &'static str,
    keywords: &'static [&'static str],
    range: (f64, f64),
    get: fn(&reactor_context::settings::Settings) -> usize,
    set: fn(&mut reactor_context::settings::Settings, usize),
) -> SettingItem {
    let default = get(&reactor_context::settings::Settings::default()) as f64;
    let (g, s, d, r) = (app.clone(), app.clone(), app.clone(), app.clone());
    SettingItem::new(
        title,
        SettingField::<f64>::number_input(
            sized(range.0, range.1, 1.0),
            move |cx| get(&g.read(cx).global_settings()) as f64,
            move |v, cx| {
                s.update(cx, |app, cx| {
                    app.edit_settings(|st| set(st, v.max(0.0) as usize), cx)
                });
            },
        )
        .default_value(default)
        .on_reset(
            move |cx| get(&d.read(cx).global_settings()) as f64 != default,
            move |_, cx| {
                r.update(cx, |app, cx| {
                    app.edit_settings(|st| set(st, default as usize), cx)
                });
            },
        ),
    )
    .keywords(keywords.iter().copied())
}

fn manifest_page(app: &Entity<ReactorApp>) -> SettingPage {
    let group = SettingGroup::new()
        .item(count_field(
            app,
            "Soft step limit",
            &["steps"],
            (1.0, 200.0),
            |s| s.manifest.soft_step_limit,
            |s, n| s.manifest.soft_step_limit = n.max(1),
        ))
        .item(count_field(
            app,
            "Longest step description (characters)",
            &["steps"],
            (20.0, 500.0),
            |s| s.manifest.max_description,
            |s, n| s.manifest.max_description = n.max(1),
        ))
        .item(count_field(
            app,
            "Words in a step's status",
            &["steps"],
            (1.0, 10.0),
            |s| s.manifest.status_words,
            |s, n| s.manifest.status_words = n.max(1),
        ))
        .item(count_field(
            app,
            "Session text a derived goal reads (characters)",
            &["derive"],
            (1_000.0, 200_000.0),
            |s| s.manifest.derive_context_chars,
            |s, n| s.manifest.derive_context_chars = n.max(1),
        ));
    info(
        SettingPage::new("Manifest")
            .icon(IconName::Target)
            .group(group),
        "manifest-info",
        "The manifest is the goal, the guidelines and the steps the agent keeps against the goal with update_steps. The goal and guidelines are written from the goal line above the transcript. The soft step limit is the number of steps past which the agent is asked to consolidate them; a step's description and status are cut to the lengths here. Saved in ~/.reactor/settings.json; they apply to the next request.",
    )
}

fn reporting_page(app: &Entity<ReactorApp>) -> SettingPage {
    let (l_get, l_set) = (app.clone(), app.clone());
    let (f_get, f_set, f_dirty, f_reset) = (app.clone(), app.clone(), app.clone(), app.clone());
    let (t_get, t_set, t_dirty, t_reset) = (app.clone(), app.clone(), app.clone(), app.clone());
    let levels: Vec<(SharedString, SharedString)> = [
        ("0", "0 — the instructions in the prompt only"),
        ("1", "1 — and a reminder while the folder is unchanged"),
        ("2", "2 — and a turn that left it undocumented is reverted"),
    ]
    .iter()
    .map(|(v, l)| (shared(*v), shared(*l)))
    .collect();
    let default_folder = reactor_context::settings::ReportingSettings::default().folder;
    let group = SettingGroup::new()
        .item(
            SettingItem::new(
                "Level when reporting is switched on",
                SettingField::<SharedString>::dropdown(
                    levels,
                    move |cx| {
                        shared(
                            l_get
                                .read(cx)
                                .global_settings()
                                .reporting
                                .level
                                .get()
                                .to_string(),
                        )
                    },
                    move |v, cx| {
                        if let Some(level) = v
                            .parse::<u8>()
                            .ok()
                            .and_then(reactor_context::settings::Enforcement::new)
                        {
                            l_set.update(cx, |app, cx| {
                                app.edit_settings(|s| s.reporting.level = level, cx)
                            });
                        }
                    },
                )
                .default_value(shared("0")),
            )
            .keywords(["enforcement"]),
        )
        .item(
            SettingItem::new(
                "Folder (in the case)",
                SettingField::<SharedString>::input(
                    move |cx| shared(f_get.read(cx).global_settings().reporting.folder),
                    move |v, cx| {
                        let v = v.trim().to_string();
                        if !v.is_empty() {
                            f_set.update(cx, |app, cx| {
                                app.edit_settings(|s| s.reporting.folder = v, cx)
                            });
                        }
                    },
                )
                .on_reset(
                    {
                        let default = default_folder.clone();
                        move |cx| f_dirty.read(cx).global_settings().reporting.folder != default
                    },
                    move |_, cx| {
                        let default = default_folder.clone();
                        f_reset.update(cx, |app, cx| {
                            app.edit_settings(|s| s.reporting.folder = default, cx)
                        });
                    },
                ),
            )
            .keywords(["report", "directory"]),
        )
        .item(count_field(
            app,
            "Steps without a change before a reminder",
            &["nag", "threshold"],
            (1.0, 100.0),
            |s| s.reporting.step_threshold,
            |s, n| s.reporting.step_threshold = n.max(1),
        ))
        .item(count_field(
            app,
            "Reverts in a row before giving up (level 2)",
            &["strict", "revert"],
            (0.0, 20.0),
            |s| s.reporting.max_reverts,
            |s, n| s.reporting.max_reverts = n,
        ))
        .item(
            SettingItem::new(
                "Report template (a file; empty for the built-in)",
                SettingField::<SharedString>::input(
                    move |cx| {
                        shared(
                            t_get
                                .read(cx)
                                .global_settings()
                                .reporting
                                .template_path
                                .unwrap_or_default(),
                        )
                    },
                    move |v, cx| {
                        let v = v.trim().to_string();
                        t_set.update(cx, |app, cx| {
                            app.edit_settings(
                                |s| s.reporting.template_path = (!v.is_empty()).then_some(v),
                                cx,
                            )
                        });
                    },
                )
                .on_reset(
                    move |cx| {
                        t_dirty
                            .read(cx)
                            .global_settings()
                            .reporting
                            .template_path
                            .is_some()
                    },
                    move |_, cx| {
                        t_reset.update(cx, |app, cx| {
                            app.edit_settings(|s| s.reporting.template_path = None, cx)
                        });
                    },
                ),
            )
            .keywords(["template"]),
        );
    info(
        SettingPage::new("Reporting")
            .icon(IconName::File)
            .group(group),
        "reporting-settings-info",
        "Reporting makes the agent write its findings up as it works. It is switched on and its level chosen per session with the Reporting chip under the prompt or with /report; what is here is what a session starts from, and the folder, thresholds and template. The folder is a path in the case: the agent is checked against whether anything in it changed. Saved in ~/.reactor/settings.json.",
    )
}

// ---------------------------------------------------------------------------------------------
// MCP servers
// ---------------------------------------------------------------------------------------------

fn mcp_page(app: &Entity<ReactorApp>, cx: &App) -> SettingPage {
    use reactor_agent::mcp::Standing;
    let rows = app.read(cx).mcp_rows();
    let armed = app.read(cx).mcp_remove_armed.clone();
    let list_app = app.clone();
    let (p_get, p_set) = (app.clone(), app.clone());

    let names = SettingGroup::new().title("Names").item(SettingItem::new(
        "Prefix tool names with mcp__<server>__",
        SettingField::<bool>::switch(
            move |cx| p_get.read(cx).mcp_prefix(),
            move |v, cx| {
                p_set.update(cx, |app, cx| app.mcp_set_prefix(v, cx));
            },
        )
        .default_value(false),
    ));

    let list = SettingGroup::new()
        .title("Servers")
        .item(actions(
            "Add",
            vec![(
                "New server…",
                "Add an MCP server in the editor window",
                |app, cx| app.open_mcp_editor(crate::mcp_editor::Start::New, cx),
            )],
            app,
        ))
        .item(SettingItem::render(move |_, _w, cx| {
            let theme = cx.theme().clone();
            let rows = match &rows {
                Ok(rows) => rows.clone(),
                Err(e) => {
                    return div()
                        .text_color(theme.danger)
                        .child(shared(format!(
                            "mcp.json cannot be read, so nothing here can be saved over it: {e}"
                        )))
                        .into_any_element();
                }
            };
            if rows.is_empty() {
                return div()
                    .text_color(theme.muted_foreground)
                    .child("No MCP servers yet.")
                    .into_any_element();
            }
            let mut column = v_flex().gap_1().w_full();
            for row in &rows {
                let name = row.server.name.clone();
                let pending = armed.as_deref() == Some(name.as_str());
                let (status, colour, why) = match &row.standing {
                    _ if !row.server.enabled => ("off".to_string(), theme.muted_foreground, None),
                    Some(Standing::Connecting) => {
                        ("connecting…".to_string(), theme.muted_foreground, None)
                    }
                    Some(Standing::Connected) => (
                        format!("{} tool{} · ~{} tok if all active", row.tools, if row.tools == 1 { "" } else { "s" }, row.tokens),
                        theme.success,
                        None,
                    ),
                    Some(Standing::Failed(why)) => (
                        format!("failed: {}", why.lines().next().unwrap_or("")),
                        theme.danger,
                        Some(why.clone()),
                    ),
                    Some(Standing::Off) | None => ("not connected".to_string(), theme.muted_foreground, None),
                };
                let button = |id: String, label: &'static str, tip: &'static str| {
                    Button::new(SharedString::from(id)).label(label).small().tooltip(tip)
                };
                let target = row.server.target();
                let mut line = h_flex()
                    .gap_2()
                    .items_center()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .child(
                        div()
                            .w(px(110.))
                            .flex_none()
                            .text_color(theme.foreground)
                            .child(shared(name.clone())),
                    )
                    .child(
                        div()
                            .w(px(50.))
                            .flex_none()
                            .text_size(theme.font_size * 0.8)
                            .text_color(theme.muted_foreground)
                            .child(if row.server.is_http() { "remote" } else { "local" }),
                    )
                    .child(crate::hints::tip(
                        div()
                            .id(SharedString::from(format!("mcp-target-{name}")))
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(theme.muted_foreground)
                            .child(shared(format!(
                                "{target}{}",
                                if row.server.tags.is_empty() {
                                    String::new()
                                } else {
                                    format!("   [{}]", row.server.tags.join(", "))
                                }
                            ))),
                        shared(format!(
                            "{target}\ntags: {}",
                            if row.server.tags.is_empty() { "none".to_string() } else { row.server.tags.join(", ") }
                        )),
                    ))
                    .child(crate::hints::tip(
                        div()
                            .id(SharedString::from(format!("mcp-status-{name}")))
                            .w(px(230.))
                            .flex_none()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(theme.font_size * 0.85)
                            .text_color(colour)
                            .child(shared(status)),
                        shared(why.unwrap_or_else(|| {
                            "Whether REactor could start or reach the server, and what its tools would cost.".to_string()
                        })),
                    ));
                let (a, n) = (list_app.clone(), name.clone());
                line = line.child(button(format!("mcp-edit-{name}"), "Edit", "Change it in the editor window").on_click(
                    move |_, _w, cx| {
                        a.update(cx, |app, cx| {
                            app.open_mcp_editor(crate::mcp_editor::Start::Edit(n.clone()), cx)
                        });
                    },
                ));
                if row.server.enabled {
                    let (a, n) = (list_app.clone(), name.clone());
                    line = line.child(
                        button(format!("mcp-retry-{name}"), "Reconnect", "Start or reach it again").on_click(
                            move |_, _w, cx| {
                                a.update(cx, |app, cx| app.mcp_reconnect(&n, cx));
                            },
                        ),
                    );
                }
                let (a, n, on) = (list_app.clone(), name.clone(), row.server.enabled);
                line = line.child(
                    button(
                        format!("mcp-toggle-{name}"),
                        if on { "Turn off" } else { "Turn on" },
                        "A server that is off is not connected to, and lists no tools",
                    )
                    .on_click(move |_, _w, cx| {
                        a.update(cx, |app, cx| app.mcp_set_enabled(&n, !on, cx));
                    }),
                );
                let (a, n) = (list_app.clone(), name.clone());
                let remove = button(
                    format!("mcp-remove-{name}"),
                    if pending { "Yes, remove" } else { "Remove" },
                    "Take it out of mcp.json. It asks once more",
                );
                let remove = if pending { remove.danger() } else { remove.ghost() };
                line = line.child(remove.on_click(move |_, _w, cx| {
                    a.update(cx, |app, cx| app.mcp_remove(&n, cx));
                }));
                if pending {
                    let a = list_app.clone();
                    line = line.child(
                        button(format!("mcp-keep-{name}"), "Keep", "Do not remove it")
                            .ghost()
                            .on_click(move |_, _w, cx| {
                                a.update(cx, |app, cx| app.mcp_remove_cancel(cx));
                            }),
                    );
                }
                column = column.child(line);
            }
            column.into_any_element()
        }));

    info(
        SettingPage::new("MCP servers")
            .icon(IconName::Wrench)
            .groups([names, list]),
        "mcp-info",
        "An MCP server gives the agent tools of its own. Add a local one (a command REactor starts and talks to) or a remote one (a URL). Each of its tools is listed in the Tools panel, off until you activate it, and tagged with what it costs: an active MCP tool puts its whole definition into every request, where an ordinary tool costs a line in the system prompt — unless the server's tools together cost more than the limit under Tools, when they are not sent and the agent loads the ones it needs on demand (`tool_search`). By default a tool is called by the name its server gives it; with the prefix on it is `mcp__<server>__<tool>`, which keeps servers apart. A name that another tool already has keeps the prefix either way, so nothing is shadowed, and changing the setting renames every tool, so tools switched on one by one need switching on again. A tag you give a server is on all its tools, so a toolset with that tag includes them; every MCP tool also has the tag `mcp`. Servers are kept in ~/.reactor/mcp.json (the `mcpServers` form other clients use) and are connected to once for the window. A tool list is read when the server connects: use Reconnect after a server changes. REactor does not sandbox a local server — it runs as you.",
    )
}

fn tools_page(app: &Entity<ReactorApp>) -> SettingPage {
    let group = SettingGroup::new()
        .item(count_field(
            app,
            "Tool output shown before it is cut (KiB)",
            &["truncate", "limit", "output"],
            (1.0, 1024.0),
            |s| s.tool_output.max_inline_bytes / 1024,
            |s, n| s.tool_output.max_inline_bytes = n.clamp(1, 1024) * 1024,
        ))
        .item(count_field(
            app,
            "Load a group of tools on demand above (tokens)",
            &[
                "tool",
                "search",
                "defer",
                "load",
                "demand",
                "mcp",
                "graph",
                "definitions",
            ],
            (0.0, 100_000.0),
            |s| s.tool_search.defer_over_tokens,
            |s, n| {
                s.tool_search.defer_over_tokens =
                    n.min(reactor_context::settings::ToolSearchSettings::MAX_TOKENS)
            },
        ))
        .item(actions(
            "Activation",
            vec![
                (
                    "Make session's the default",
                    "This session's tools and toolsets become the default for new sessions",
                    |app, cx| app.tools_promote(cx),
                ),
                (
                    "Use the default",
                    "Drop this session's override: it uses the machine default again",
                    |app, cx| app.tools_inherit(cx),
                ),
            ],
            app,
        ));
    info(
        SettingPage::new("Tools")
            .icon(IconName::Wrench)
            .group(group),
        "tools-info",
        "Toggling a tool or toolset starts a session-only override of the machine's default (~/.reactor/state.json). Individual tools are toggled in the Tools panel or with /tool. Longer tool output is cut to its head and tail and kept whole for history_read; a call may ask for another limit with max_output_bytes.",
    )
}
