//! The data viewer: a generic viewer for binary formats that have a picture — images today.
//! One [`DataViewer`] view is hosted two ways: as a dockable panel (any number of them) and
//! in a popup ([`open_popup`]) for a quick look.

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{Panel, PanelEvent};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{
    Entity, EventEmitter, FocusHandle, Focusable, ObjectFit, Render, Window, div, img, px,
};

use reactor_fs::FileRef;

use super::data_model::{Decoded, decode};
use super::human_size;
use crate::app::ReactorApp;
use crate::panels::{close_button, panel_title_row};

/// Larger files are not decoded into memory.
const MAX_BYTES: u64 = 64 << 20;
const STEPS: [f32; 11] = [0.1, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 8.0, 16.0];

#[derive(Clone, Copy, PartialEq)]
enum Zoom {
    /// As large as fits the pane.
    Fit,
    Scale(f32),
}

pub struct DataViewer {
    file: FileRef,
    decoded: Result<Decoded, String>,
    bytes: u64,
    zoom: Zoom,
    focus: FocusHandle,
}

impl DataViewer {
    pub fn new(file: FileRef, cx: &mut Context<Self>) -> Self {
        let (decoded, bytes) = match file.read_all(MAX_BYTES) {
            Ok(bytes) => (decode(&bytes, &file.name()), bytes.len() as u64),
            Err(e) => (Err(format!("could not read {}: {e}", file.path)), 0),
        };
        Self {
            file,
            decoded,
            bytes,
            zoom: Zoom::Fit,
            focus: cx.focus_handle(),
        }
    }

    pub fn title(&self) -> String {
        self.file.name()
    }

    fn step(&mut self, direction: i32, cx: &mut Context<Self>) {
        let current = match self.zoom {
            Zoom::Scale(scale) => scale,
            Zoom::Fit => 1.0,
        };
        let next = if direction > 0 {
            STEPS.iter().copied().find(|s| *s > current + 0.001)
        } else {
            STEPS.iter().copied().rev().find(|s| *s < current - 0.001)
        };
        if let Some(next) = next {
            self.zoom = Zoom::Scale(next);
            cx.notify();
        }
    }
}

impl Focusable for DataViewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DataViewer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let decoded = match &self.decoded {
            Ok(decoded) => decoded.clone(),
            Err(message) => {
                return div()
                    .p_3()
                    .track_focus(&self.focus)
                    .text_color(theme.warning)
                    .child(format!(
                        "{message}. The text editor or hex viewer can open this file."
                    ))
                    .into_any_element();
            }
        };

        let zoom = self.zoom;
        let label = match zoom {
            Zoom::Fit => "Fit".to_owned(),
            Zoom::Scale(s) => format!("{}%", (s * 100.).round()),
        };
        let toolbar = h_flex()
            .gap_1()
            .p_1()
            .items_center()
            .child(
                Button::new("data-zoom-out")
                    .icon(IconName::Minus)
                    .ghost()
                    .small()
                    .tooltip("Zoom out")
                    .on_click(cx.listener(|this, _, _, cx| this.step(-1, cx))),
            )
            .child(div().min_w(px(44.)).text_center().child(label))
            .child(
                Button::new("data-zoom-in")
                    .icon(IconName::Plus)
                    .ghost()
                    .small()
                    .tooltip("Zoom in")
                    .on_click(cx.listener(|this, _, _, cx| this.step(1, cx))),
            )
            .child(
                Button::new("data-fit")
                    .label("Fit")
                    .ghost()
                    .small()
                    .disabled(zoom == Zoom::Fit)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.zoom = Zoom::Fit;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("data-actual")
                    .label("100%")
                    .ghost()
                    .small()
                    .disabled(zoom == Zoom::Scale(1.0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.zoom = Zoom::Scale(1.0);
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .child(div().px_2().text_color(muted).child(format!(
                "{} · {}",
                decoded.describe(),
                human_size(self.bytes)
            )));

        let picture: AnyElement = match (&decoded, zoom) {
            (Decoded::Raster { image, .. }, Zoom::Fit) => img(image.clone())
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            (
                Decoded::Raster {
                    image,
                    width,
                    height,
                    ..
                },
                Zoom::Scale(scale),
            ) => img(image.clone())
                .w(px(*width as f32 * scale))
                .h(px(*height as f32 * scale))
                .flex_none()
                .into_any_element(),
            (Decoded::Svg(image), Zoom::Fit) => img(image.clone())
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            // An SVG has no pixel size; scaling it multiplies a nominal 512 px box.
            (Decoded::Svg(image), Zoom::Scale(scale)) => img(image.clone())
                .w(px(512. * scale))
                .h(px(512. * scale))
                .flex_none()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
        };

        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .child(toolbar)
            .child(
                div()
                    .id("data-canvas")
                    .flex_1()
                    .min_h_0()
                    .bg(theme.tokens.table)
                    .overflow_scroll()
                    .child(picture),
            )
            .into_any_element()
    }
}

/// A quick look in a popup, without opening a panel.
pub fn open_popup<T: 'static>(file: FileRef, window: &mut Window, cx: &mut Context<T>) {
    use gpui_kit::component::WindowExt as _;
    let title = file.name();
    let view = cx.new(|cx| DataViewer::new(file, cx));
    window.open_dialog(cx, move |dialog, window, _cx| {
        let view = view.clone();
        let height = window.viewport_size().height * 0.7;
        dialog
            .title(title.clone())
            .w(px(900.))
            .content(move |content, _window, _cx| {
                content.child(div().h(height).w_full().child(view.clone()))
            })
    });
}

/// The dockable form.
pub struct DataPanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    view: Entity<DataViewer>,
}

impl DataPanel {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        file: FileRef,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            view: cx.new(|cx| DataViewer::new(file, cx)),
        }
    }
}

impl Focusable for DataPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.view.focus_handle(cx)
    }
}

impl EventEmitter<PanelEvent> for DataPanel {}

impl gpui_kit::base::dock::Panel for DataPanel {
    fn panel_name(&self) -> &'static str {
        "data"
    }
}

impl Panel for DataPanel {
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_row(IconName::Image, &self.view.read(cx).title())
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        Some(vec![close_button(&self.app, cx.entity())])
    }
}

impl Render for DataPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.view.clone())
    }
}
