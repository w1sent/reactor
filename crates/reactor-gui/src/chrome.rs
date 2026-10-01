//! Window chrome: the title bar with its controls, and the application menu.
//!
//! Neither comes for free on Linux. gpui asks the compositor for server-side
//! decorations and draws nothing itself, so on a compositor that does not grant
//! them (GNOME's Wayland session is the common one) the window has no title bar,
//! no close/minimise/maximise and no way to be moved or resized. And
//! `cx.set_menus` is a native menu bar on macOS only: elsewhere it merely
//! *records* the menus, and something has to draw them — gpui-kit's
//! [`AppMenuBar`], fed from a registry `set_menus` does not write to.
//!
//! So this app asks for client-side decorations, renders gpui-kit's [`TitleBar`]
//! itself (which draws the controls only when the window really is
//! client-decorated, so a compositor that insists on server-side ones does not
//! produce a second set), and puts the menu bar inside it. On macOS the title
//! bar leaves room for the traffic lights and the native menu bar does the rest.

use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::component::{ActiveTheme as _, TitleBar};
use gpui_kit::base::{GlobalState, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Entity, Menu, SharedString, WindowBounds, WindowDecorations, WindowOptions, div, px,
};

/// The application's id: the Wayland `app_id` and X11 class, and the name of the `.desktop`
/// file (`reactor setup` installs it) that gives launchers and Wayland compositors the icon.
pub const APP_ID: &str = "reactor-gui";

/// The window icon, for the platforms that take pixels (X11). Windows reads the executable's
/// resources (`build.rs`) and macOS the bundle's `icon.icns`.
fn window_icon() -> Option<std::sync::Arc<image::RgbaImage>> {
    let icon = image::load_from_memory_with_format(include_bytes!("../../../assets/icon.ico"), image::ImageFormat::Ico).ok()?;
    Some(std::sync::Arc::new(icon.into_rgba8()))
}

/// Options for any window that renders [`title_bar`]: the given bounds,
/// client-side decorations, and the title bar owning window dragging.
pub fn window_options(bounds: WindowBounds) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(bounds),
        window_decorations: Some(WindowDecorations::Client),
        // Wayland finds the icon through the `.desktop` file this id names; X11 takes pixels.
        app_id: Some(APP_ID.to_owned()),
        icon: window_icon(),
        ..TitleBar::window_options()
    }
}

/// Register the application menus and make them drawable.
///
/// `cx.set_menus` alone is enough for macOS's native bar; the in-window
/// [`AppMenuBar`] reads its own registry, which is filled from what the
/// platform recorded.
pub fn install_menus(cx: &mut App, menus: Vec<Menu>) {
    cx.set_menus(menus);
    GlobalState::init(cx);
    if let Some(owned) = cx.get_menus() {
        GlobalState::global_mut(cx).set_app_menus(owned);
    }
}

/// The in-window menu bar, where there is no native one to use.
pub fn menu_bar(cx: &mut App) -> Option<Entity<AppMenuBar>> {
    if cfg!(target_os = "macos") { None } else { Some(AppMenuBar::new(cx)) }
}

/// The title bar: the menu (if any) on the left, the window's title after it, and `right`
/// (the notification bell) at the far end.
pub fn title_bar(
    title: impl Into<SharedString>,
    menu: Option<&Entity<AppMenuBar>>,
    right: Option<gpui_kit::AnyElement>,
    cx: &App,
) -> TitleBar {
    let title: SharedString = title.into();
    TitleBar::new()
        .child(
            h_flex()
                .items_center()
                .gap_3()
                .children(menu.cloned())
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .text_size(px(12.))
                        .child(title),
                ),
        )
        .children(right)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_window_icon_decodes() {
        let icon = super::window_icon().expect("assets/icon.ico decodes");
        assert!(icon.width() >= 128 && icon.height() >= 128);
    }
}
