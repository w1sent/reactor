//! The rail and its drawer.
//!
//! The rail is the strip of icons down the window's left edge: the panels one picks from
//! ([`PanelKind::RAIL`]), one of which at a time is shown in the drawer beside it. Below
//! them, while the case's documents are being embedded, a ring that fills as it goes, and
//! the settings.
//!
//! A rail panel is the same view wherever it is shown: in the drawer, or as a tab of the
//! workspace when it is wanted in view for longer (the drawer's own button, or a right click
//! on the icon). It is in one place at a time, so the icon of a panel that is a tab brings
//! that tab forward rather than opening the drawer.

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::Panel as _;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::progress::{Progress, ProgressCircle};
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{Entity, Render, SharedString, Window, div, px};

use crate::app::ReactorApp;
use crate::layout::PanelKind;

/// How wide the rail is.
const RAIL_WIDTH: f32 = 44.;

impl ReactorApp {
    /// Whether the panel is a tab of the workspace now.
    pub(crate) fn docked(&self, kind: PanelKind, cx: &App) -> bool {
        self.panels.is_open(kind, self.dock_area.read(cx), cx)
    }

    /// The drawer's panel, if the drawer is open: never one that is a tab of the workspace,
    /// which would be the same view drawn twice.
    pub(crate) fn drawer_kind(&self, cx: &App) -> Option<PanelKind> {
        self.drawer.filter(|kind| !self.docked(*kind, cx))
    }

    /// A click on a rail icon: open its drawer, or close it if it is the one open. A panel
    /// that is a tab of the workspace is brought forward there instead.
    pub fn rail_click(&mut self, kind: PanelKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.docked(kind, cx) {
            let id = self.panels.view(kind).panel_id(cx);
            self.dock_area
                .update(cx, |area, cx| area.select_panel(id, window, cx));
        } else if self.drawer == Some(kind) {
            self.drawer = None;
        } else {
            self.drawer = Some(kind);
        }
        cx.notify();
    }

    /// Show a panel wherever it is kept: its tab if it has one, else the drawer for a rail
    /// panel, else a new tab where the layout keeps it.
    pub fn reveal_panel(&mut self, kind: PanelKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.docked(kind, cx) {
            let id = self.panels.view(kind).panel_id(cx);
            self.dock_area
                .update(cx, |area, cx| area.select_panel(id, window, cx));
        } else if kind.on_rail() {
            self.drawer = Some(kind);
        } else {
            crate::layout::toggle_panel(
                kind,
                self.layout,
                &self.panels,
                &self.dock_area,
                window,
                cx,
            );
        }
        cx.notify();
    }

    /// Make a rail panel a tab of the workspace, where it stays in view.
    pub fn dock_rail_panel(
        &mut self,
        kind: PanelKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.drawer == Some(kind) {
            self.drawer = None;
        }
        if !self.docked(kind, cx) {
            crate::layout::toggle_panel(
                kind,
                self.layout,
                &self.panels,
                &self.dock_area,
                window,
                cx,
            );
        }
        cx.notify();
    }

    /// Take a rail panel out of the workspace and show it in the drawer again.
    pub fn undock_rail_panel(
        &mut self,
        kind: PanelKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.docked(kind, cx) {
            crate::layout::toggle_panel(
                kind,
                self.layout,
                &self.panels,
                &self.dock_area,
                window,
                cx,
            );
        }
        self.drawer = Some(kind);
        cx.notify();
    }

    pub(crate) fn render_rail(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let mut rail = v_flex()
            .id("rail")
            .flex_none()
            .w(px(RAIL_WIDTH))
            .h_full()
            .py_1()
            .gap_1()
            .items_center()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border);
        for kind in PanelKind::RAIL {
            let docked = self.docked(kind, cx);
            let open = self.drawer == Some(kind) && !docked;
            let weak = cx.weak_entity();
            rail = rail.child(
                div()
                    .id(SharedString::from(format!("rail-slot-{}", kind.slug())))
                    .child(
                        Button::new(SharedString::from(format!("rail-{}", kind.slug())))
                            .icon(kind.icon())
                            .ghost()
                            .selected(open)
                            .tooltip(if docked {
                                format!("{}: a tab of the workspace", kind.label())
                            } else {
                                kind.label().to_string()
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.rail_click(kind, window, cx)
                            })),
                    )
                    .context_menu(move |menu, _window, _cx| {
                        let weak = weak.clone();
                        menu.item(
                            PopupMenuItem::new(if docked {
                                "Move back to the rail"
                            } else {
                                "Open as a workspace tab"
                            })
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |app, cx| {
                                    if docked {
                                        app.undock_rail_panel(kind, window, cx)
                                    } else {
                                        app.dock_rail_panel(kind, window, cx)
                                    }
                                });
                            }),
                        )
                    }),
            );
        }
        rail = rail.child(div().flex_1());
        // Vectors are being made for the case's documents: how far along, as a ring; its
        // numbers on a click. Nothing at all when nothing is being embedded.
        if let Some(progress) = self.embedding {
            let percent = if progress.total == 0 {
                0.
            } else {
                progress.embedded as f32 * 100. / progress.total as f32
            };
            rail = rail.child(
                Popover::new("embedding-popover")
                    .anchor(Anchor::BottomLeft)
                    .trigger(
                        Button::new("rail-embedding")
                            .ghost()
                            .tooltip("The case's documents are being embedded")
                            .child(
                                ProgressCircle::new("embedding-ring")
                                    .small()
                                    .value(percent)
                                    .accessibility_label("Embedding progress"),
                            ),
                    )
                    .content(move |_, _, cx| {
                        let theme = cx.theme().clone();
                        v_flex()
                            .w(px(220.))
                            .gap_2()
                            .child("Embedding documents")
                            .child(Progress::new("embedding-bar").value(percent))
                            .child(
                                h_flex()
                                    .justify_between()
                                    .text_size(theme.font_size * 0.85)
                                    .child(
                                        div().text_color(theme.muted_foreground).child("embedded"),
                                    )
                                    .child(
                                        div().font_family(theme.mono_font_family.clone()).child(
                                            format!("{} / {}", progress.embedded, progress.total),
                                        ),
                                    ),
                            )
                    }),
            );
        }
        rail.child(
            Button::new("rail-settings")
                .icon(IconName::Settings)
                .ghost()
                .tooltip("Settings")
                .on_click(cx.listener(|this, _, window, cx| this.toggle_settings(window, cx))),
        )
        .into_any_element()
    }
}

/// The drawer: the rail panel that is open, under a header with the panel's own toolbar.
///
/// A view of its own rather than part of the app's: a panel's toolbar reads the app (is it
/// loading, what is listed), which the app cannot be asked for while it is drawing itself.
pub struct Drawer {
    app: gpui_kit::WeakEntity<ReactorApp>,
    tree: Entity<crate::panels::TreePanel>,
    toolbox: Entity<crate::toolbox::ToolboxPanel>,
    services: Entity<crate::panels::ServicesPanel>,
    files: Entity<crate::files::explorer::FilesPanel>,
}

impl Drawer {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        tree: Entity<crate::panels::TreePanel>,
        toolbox: Entity<crate::toolbox::ToolboxPanel>,
        services: Entity<crate::panels::ServicesPanel>,
        files: Entity<crate::files::explorer::FilesPanel>,
    ) -> Self {
        Self {
            app,
            tree,
            toolbox,
            services,
            files,
        }
    }

    /// The buttons a panel puts in its tab bar when it is a tab: its refresh, its own
    /// switches, and its close (which closes the drawer: [`ReactorApp::close_panel`]).
    fn toolbar(&self, kind: PanelKind, window: &mut Window, cx: &mut App) -> Vec<Button> {
        match kind {
            PanelKind::Tree => self.tree.update(cx, |p, cx| p.toolbar_buttons(window, cx)),
            PanelKind::Toolbox => self
                .toolbox
                .update(cx, |p, cx| p.toolbar_buttons(window, cx)),
            PanelKind::Services => self
                .services
                .update(cx, |p, cx| p.toolbar_buttons(window, cx)),
            PanelKind::Files => self.files.update(cx, |p, cx| p.toolbar_buttons(window, cx)),
            _ => None,
        }
        .unwrap_or_default()
    }
}

impl Render for Drawer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.app.upgrade().and_then(|app| {
            let app = app.read(cx);
            let kind = app.drawer_kind(cx)?;
            Some((kind, app.panels.view(kind).view()))
        });
        let Some((kind, view)) = open else {
            return div().into_any_element();
        };
        let theme = cx.theme().clone();
        let buttons = self.toolbar(kind, window, cx);
        let app = self.app.clone();
        // As wide as the app's split gives it (`ReactorApp::render`): the drawer is resized
        // by dragging its edge.
        v_flex()
            .id("drawer")
            .size_full()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex()
                    .h(px(32.))
                    .flex_none()
                    .pl_3()
                    .pr_1()
                    .gap_1()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex_1()
                            .text_size(theme.font_size * 0.75)
                            .text_color(theme.muted_foreground)
                            .child(kind.label().to_uppercase()),
                    )
                    .child(
                        Button::new("drawer-dock")
                            .icon(IconName::PanelRight)
                            .ghost()
                            .small()
                            .tooltip("Open as a tab of the workspace, where it stays in view")
                            .on_click(move |_, window, cx| {
                                let _ =
                                    app.update(cx, |app, cx| app.dock_rail_panel(kind, window, cx));
                            }),
                    )
                    .children(buttons),
            )
            .child(div().flex_1().min_h_0().child(view))
            .into_any_element()
    }
}
