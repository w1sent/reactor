//! The binary shell: parse the launch flags, resolve the workdir (chooser
//! when nothing is passed, SPEC.md §3), open the session window.

use std::path::PathBuf;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Root};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{
    App, Entity, KeyBinding, PathPromptOptions, Render, SharedString, Window, div, px, size,
};

use reactor_gui::app::ReactorApp;
use reactor_gui::start::{
    GuiConfig, LaunchArgs, ResolvedLaunch, parse_args,
};
use reactor_gui::backend::{SessionSummary, list_sessions};
use reactor_gui::theme;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let launch = parse_args(&args);

    // `.with_assets(AllAssets)` registers the platform `AssetSource` icon
    // SVGs are resolved through at paint time. Its absence is silent — no
    // panic, no log — every `Icon`/`Button::icon()` simply paints nothing,
    // which is the bug behind "the scroll-to-end button has no icon" (and
    // every other icon in the window). `AllAssets` (the full Lucide
    // catalog), not the narrower default `Assets`, because this app's icons
    // (Wrench, Layers, Server, GitBranch, MessageSquare, LayoutDashboard, …)
    // reach beyond gpui-component's own curated default set.
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            // Initialize the layers first: `init` installs the globals — the theme
            // registry among them — that the Ayu Dark install reads, and wires
            // the input, dock and menu layers the components need (SPEC.md
            // §7's install order). Everything after this line depends on it.
            gpui_kit::init(cx);
            theme::install_ayu_dark(cx);
            cx.bind_keys(vec![KeyBinding::new(
                "escape",
                reactor_gui::app::ComposerEsc,
                None,
            )]);
            reactor_gui::app::bind_keys(cx);
            install_menus(cx);

            match &launch.cwd {
                Some(cwd) => {
                    let resolved = ResolvedLaunch {
                        cwd: cwd.clone(),
                        launch_args: launch.clone(),
                    };
                    if launch.resume {
                        SessionPicker::open(cx, resolved);
                    } else {
                        let _ = ReactorApp::open(cx, &resolved);
                    }
                }
                None => WorkdirChooser::open(cx, launch),
            }
        });
}

/// The application menu: the named layouts, and a switch per dock.
///
/// One definition, two renderings — `cx.set_menus` drives the real menu bar
/// on macOS, and `gpui_kit::component::menu::AppMenuBar` draws these same
/// menus in-window on Windows and Linux (`chrome::install_menus` feeds it;
/// `set_menus` alone only records them there), so neither platform gets a
/// hand-rolled imitation of the other's.
///
/// Menu items dispatch actions rather than calling anything, which is what
/// lets the window answer them: they land on `ReactorApp`'s root, alongside
/// the model and thinking pickers (see its `Render`).
fn install_menus(cx: &mut App) {
    use reactor_gui::app::{ApplyLayoutAction, MainWindow, OpenPalette, OpenSettings, ToggleDockAction};
    use reactor_gui::layout::{DockSide, LayoutPreset};

    // Global handlers, not `.on_action` on some element: a native menu bar
    // is application-wide, and `is_action_available` — what greys a menu
    // item out — walks the *focused window's* dispatch tree. With no
    // element-level handler for these two actions anywhere, every "Layout"
    // item was permanently grey and inert regardless of focus (`MainWindow`'s
    // doc has the rest of it).
    cx.on_action(|action: &ApplyLayoutAction, cx| {
        let Some(main) = cx.try_global::<MainWindow>().cloned() else {
            return;
        };
        let preset = action.preset;
        // Deferred, not an immediate `cx.update_window`: a native menu click
        // reaches this handler from inside that very window's own dispatch
        // (`Window::dispatch_action`'s deferred callback runs global action
        // listeners with the window's slot already taken out of `App` for
        // the duration), so re-entering it here found "window not found"
        // and silently swallowed by the `let _ =` this replaced — every
        // click looked like it did nothing. `cx.defer` queues this for after
        // that borrow ends, the same way gpui's own `dispatch_action` defers
        // itself past the click handler that triggered it.
        cx.defer(move |cx| {
            let _ = cx.update_window(main.handle, move |_, window, cx| {
                if let Some(app) = main.app.upgrade() {
                    app.update(cx, |app, cx| app.apply_layout(preset, window, cx));
                }
            });
        });
    });
    cx.on_action(|action: &ToggleDockAction, cx| {
        let Some(main) = cx.try_global::<MainWindow>().cloned() else {
            return;
        };
        let side = action.side;
        cx.defer(move |cx| {
            let _ = cx.update_window(main.handle, move |_, window, cx| {
                if let Some(app) = main.app.upgrade() {
                    app.update(cx, |app, cx| app.toggle_dock(side, window, cx));
                }
            });
        });
    });

    cx.on_action(|_: &OpenPalette, cx| {
        let Some(main) = cx.try_global::<MainWindow>().cloned() else {
            return;
        };
        cx.defer(move |cx| {
            let _ = cx.update_window(main.handle, move |_, window, cx| {
                if let Some(app) = main.app.upgrade() {
                    app.update(cx, |app, cx| app.toggle_palette(window, cx));
                }
            });
        });
    });

    cx.on_action(|_: &OpenSettings, cx| {
        let Some(main) = cx.try_global::<MainWindow>().cloned() else {
            return;
        };
        cx.defer(move |cx| {
            let _ = cx.update_window(main.handle, move |_, window, cx| {
                if let Some(app) = main.app.upgrade() {
                    app.update(cx, |app, cx| app.toggle_settings(window, cx));
                }
            });
        });
    });

    let mut items: Vec<MenuItem> = LayoutPreset::ALL
        .iter()
        .map(|preset| MenuItem::action(preset.label(), ApplyLayoutAction { preset: *preset }))
        .collect();
    items.push(MenuItem::Separator);
    items.extend(DockSide::ALL.iter().map(|side| {
        MenuItem::action(
            format!("Show {}", side.label()),
            ToggleDockAction { side: *side },
        )
    }));

    reactor_gui::chrome::install_menus(
        cx,
        vec![
            Menu {
                name: "Commands".into(),
                items: vec![MenuItem::action("Command Palette…", OpenPalette), MenuItem::action("Settings…", OpenSettings)],
                disabled: false,
            },
            Menu {
                name: "Layout".into(),
                items,
                disabled: false,
            },
        ],
    );
}

/// The workdir chooser (nothing passed, SPEC.md §3): the left half lists
/// frequently used folders, the right half is a path input plus a native
/// folder-picker button — either one opens the same way (SPEC.md §8).
struct WorkdirChooser {
    recents: Vec<String>,
    path_input: Entity<InputState>,
    launch: LaunchArgs,
}

impl WorkdirChooser {
    fn open(cx: &mut App, launch: LaunchArgs) {
        let bounds = gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds::centered(
            None,
            size(px(780.), px(440.)),
            cx,
        ));
        let _ = cx.open_window(
            reactor_gui::chrome::window_options(bounds),
            |window, cx| {
                // Wrap in `Root`, exactly like `ReactorApp::open`: `Root`'s
                // own render is what actually applies the theme's
                // `text_color`/background to the window (gpui-component's
                // `root.rs`) — skipping it left this window with no ambient
                // text color at all, so typed input text rendered invisible
                // against the near-black background (the bug this fixes).
                theme::install_ayu_dark(cx);
                let path_input =
                    cx.new(|cx| InputState::new(window, cx).placeholder("/path/to/workdir"));
                let chooser: Entity<WorkdirChooser> = cx.new(|_cx| WorkdirChooser {
                    recents: GuiConfig::load().recent_workdirs,
                    path_input,
                    launch,
                });
                let root_view: gpui_kit::AnyView = chooser.into();
                cx.new(|cx| Root::new(root_view, window, cx))
            },
        );
    }

    fn open_chosen(&mut self, cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // Recents are recorded once, canonically, in `ReactorApp::open` —
        // the choke point every launch path (this chooser, the session
        // picker, a positional cwd) funnels through.
        window.remove_window();
        let resolved = ResolvedLaunch {
            cwd,
            launch_args: self.launch.clone(),
        };
        let _ = ReactorApp::open(cx, &resolved);
    }

    /// The native folder dialog (gpui's platform picker), alongside the path
    /// input rather than instead of it — some machines (Linux without a
    /// desktop portal) have none, so typing a path always keeps working
    /// (SPEC.md §8). Picking a folder opens it immediately, the same
    /// one-click behavior as clicking a "frequent" row.
    fn on_browse_click(
        &mut self,
        _: &gpui_kit::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose workdir".into()),
        });
        let weak = cx.weak_entity();
        window
            .spawn(cx, async move |async_cx| {
                let Ok(Ok(Some(mut paths))) = receiver.await else {
                    return;
                };
                let Some(path) = paths.pop() else {
                    return;
                };
                let _ = async_cx.update(|window, cx| {
                    weak.update(cx, |this, cx| this.open_chosen(path, window, cx))
                });
            })
            .detach();
    }
}

impl Focusable for WorkdirChooser {
    fn focus_handle(&self, cx: &App) -> gpui_kit::FocusHandle {
        // The path input owns focus for this window (SPEC.md §3).
        self.path_input.read(cx).focus_handle(cx)
    }
}

impl Render for WorkdirChooser {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let can_open = !self.path_input.read(cx).value().trim().is_empty();

        let mut recents = v_flex().gap_1();
        for path in &self.recents {
            recents = recents.child(
                div()
                    .id(SharedString::from(path.clone()))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .hover(|style| style.bg(theme.list_hover))
                    .on_click({
                        let path = path.clone();
                        cx.listener(move |this, _, window, cx| {
                            this.open_chosen(PathBuf::from(&path), window, cx);
                        })
                    })
                    .child(path.clone()),
            );
        }

        v_flex().size_full().child(reactor_gui::chrome::title_bar("REactor", None, None, cx)).child(v_flex()
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_4()
            .child(
                h_flex().justify_between().child(
                    Label::new("REactor — choose the workdir").text_size(theme.font_size * 1.2),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_4()
                    .child(
                        // Left: frequently used folders (SPEC.md §3).
                        // `recents` (built above, one clickable row per
                        // path) — not `self.recents` again: that is the raw
                        // `Vec<String>`, which used to be wired in here by
                        // mistake, rendering lookalike rows with no click
                        // handler at all (the bug behind "clicking a
                        // frequent path does nothing").
                        v_flex()
                            .w(px(300.))
                            .gap_2()
                            .child(Label::new("frequent").text_color(theme.muted_foreground))
                            .child(recents),
                    )
                    .child(
                        // Right: type a path, or pick one with the native
                        // folder dialog — both land in the same input, so
                        // either path opens the same way (SPEC.md §3).
                        v_flex()
                            .flex_1()
                            .gap_2()
                            .child(Label::new("path").text_color(theme.muted_foreground))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(Input::new(&self.path_input).flex_1())
                                    .child(
                                        Button::new("browse")
                                            .label("Browse…")
                                            .on_click(cx.listener(Self::on_browse_click)),
                                    ),
                            )
                            .child(
                                Button::new("open")
                                    .primary()
                                    .label("Open session")
                                    .disabled(!can_open)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let Some(cwd) = this.chosen_path(cx) else {
                                            return;
                                        };
                                        this.open_chosen(cwd, window, cx);
                                    })),
                            ),
                    ),
            ))
    }
}

impl WorkdirChooser {
    /// The chosen workdir, from the path input (SPEC.md §3).
    fn chosen_path(&self, cx: &App) -> Option<PathBuf> {
        let typed = self.path_input.read(cx).value().to_string();
        let trimmed = typed.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(PathBuf::from(trimmed))
        }
    }
}

/// The `-r` picker: the GUI's native session picker, listing the project's sessions from the store
/// (SPEC.md §3).
struct SessionPicker {
    sessions: Vec<SessionSummary>,
    resolved: ResolvedLaunch,
}

impl SessionPicker {
    fn open(cx: &mut App, resolved: ResolvedLaunch) {
        let sessions = list_sessions(&reactor_core::Paths::from_env(), Some(&resolved.cwd));
        let bounds = gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds::centered(
            None,
            size(px(780.), px(440.)),
            cx,
        ));
        let _ = cx.open_window(
            reactor_gui::chrome::window_options(bounds),
            |window, cx| {
                // See `WorkdirChooser::open`'s comment: `Root` is what
                // applies the theme's ambient text color to the window.
                theme::install_ayu_dark(cx);
                let picker: Entity<SessionPicker> =
                    cx.new(|_| SessionPicker { sessions, resolved });
                let root_view: gpui_kit::AnyView = picker.into();
                cx.new(|cx| Root::new(root_view, window, cx))
            },
        );
    }
}

impl Focusable for SessionPicker {
    fn focus_handle(&self, cx: &App) -> gpui_kit::FocusHandle {
        // The list is a plain view in v0.1; the app's handle is the window's
        // (SPEC.md §6's keyboard model starts at the composer).
        cx.focus_handle()
    }
}

impl Render for SessionPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let rows = self
            .sessions
            .iter()
            .map(|session| {
                let prompt = session.first_prompt.clone().unwrap_or_else(|| "(no prompt yet)".into());
                let prompt: String = prompt.lines().next().unwrap_or("").chars().take(90).collect();
                let label = format!("{prompt}  ·  {} entries", session.entries);
                SessionRow {
                    path: session.dir.clone(),
                    label,
                }
            })
            .collect::<Vec<_>>();

        let mut list = v_flex()
            .id("session-picker-list")
            .p_3()
            .gap_1()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if rows.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .text_color(theme.muted_foreground)
                    .child("no sessions for this workdir yet"),
            );
        }
        for row in &rows {
            let session_path = row.path.clone();
            let session_label = row.label.clone();
            list = list.child(
                div()
                    .id(SharedString::from(session_path.display().to_string()))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .hover(|style| style.bg(theme.list_hover))
                    .on_click({
                        let session_path = session_path.clone();
                        let resolved = self.resolved.clone();
                        move |_, window, cx| {
                            window.remove_window();
                            let mut launch_args = resolved.launch_args.clone();
                            launch_args.session = Some(session_path.clone());
                            let resolved = ResolvedLaunch {
                                cwd: resolved.cwd.clone(),
                                launch_args,
                            };
                            let _ = ReactorApp::open(cx, &resolved);
                        }
                    })
                    .child(Label::new(session_label)),
            );
        }
        v_flex()
            .size_full()
            .child(reactor_gui::chrome::title_bar("REactor", None, None, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .child(
                        h_flex().justify_between().child(
                            Label::new("REactor — resume a session")
                                .text_size(theme.font_size * 1.2),
                        ),
                    )
                    .child(div().flex_1().min_h_0().child(list)),
            )
    }
}

/// One row of the session picker, as rendered.
#[derive(Debug, Clone, PartialEq)]
struct SessionRow {
    path: PathBuf,
    label: String,
}
