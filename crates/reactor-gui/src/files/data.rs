//! The data viewer: one viewer for files that have a better form than their bytes or their
//! text — images, Markdown (rendered), and property lists (binary or XML, as a tree). One
//! [`DataViewer`] view is hosted two ways: as a dockable panel (any number of them) and in a
//! popup ([`open_popup`]) for a quick look.

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{Panel, PanelEvent};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{
    Entity, EventEmitter, FocusHandle, Focusable, ObjectFit, Render, Window, div, img, px,
};

use reactor_fs::FileRef;

use super::data_model::{Decoded, decode};
use super::human_size;
use super::plist;
use crate::app::ReactorApp;
use crate::panels::{close_button, closer, panel_title_closing};

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
    /// Markdown: the source instead of the rendered text. A property list: its XML instead of
    /// the tree.
    as_text: bool,
    /// A property list: the containers that are open, the rows that shows, the one picked
    /// (by its path) and the list's scroll.
    open: std::collections::HashSet<Vec<usize>>,
    rows: Vec<plist::Row>,
    picked: Option<Vec<usize>>,
    scroll: UniformListScrollHandle,
    /// A keyed archive: its table as written, instead of the objects it holds.
    raw: bool,
}

/// What *copy* takes out of the field that is picked.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CopyKind {
    /// The text of a string, a number or date as written, data as hex, a structure as JSON.
    Value,
    /// The field's key (or position).
    Key,
    Json,
    Xml,
    Hex,
    Base64,
}

/// Rows of a property list's tree shown at most, whatever is open.
const MAX_ROWS: usize = 20_000;
/// Characters of a property list's XML shown in the text view.
const MAX_TEXT: usize = 400_000;

impl DataViewer {
    pub fn new(file: FileRef, cx: &mut Context<Self>) -> Self {
        let (decoded, bytes) = match file.read_all(MAX_BYTES) {
            Ok(bytes) => (decode(&bytes, &file.name()), bytes.len() as u64),
            Err(e) => (Err(format!("could not read {}: {e}", file.path)), 0),
        };
        let mut viewer = Self {
            file,
            decoded,
            bytes,
            zoom: Zoom::Fit,
            focus: cx.focus_handle(),
            as_text: false,
            open: Default::default(),
            rows: Vec::new(),
            picked: None,
            scroll: UniformListScrollHandle::new(),
            raw: false,
        };
        viewer.open_first_levels();
        viewer
    }

    /// The property list as it is shown: a keyed archive's objects, or the list as written.
    fn shown(&self) -> Option<&plist::Value> {
        match &self.decoded {
            Ok(Decoded::Plist(doc)) => Some(doc.shown(self.raw)),
            _ => None,
        }
    }

    /// A property list opens two levels down.
    fn open_first_levels(&mut self) {
        self.open = self
            .shown()
            .map(|v| plist::containers(v, 2, MAX_ROWS))
            .unwrap_or_default();
        self.picked = None;
        self.rebuild_rows();
    }

    /// The icon for the panel's tab: what is in it.
    pub fn icon(&self) -> IconName {
        match &self.decoded {
            Ok(Decoded::Markdown(_)) => IconName::FileText,
            Ok(Decoded::Plist(_)) => IconName::ListTree,
            _ => IconName::Image,
        }
    }

    /// The rows the property list's tree has now.
    fn rebuild_rows(&mut self) {
        self.rows = self
            .shown()
            .map(|v| plist::rows(v, &self.open, MAX_ROWS))
            .unwrap_or_default();
    }

    fn toggle(&mut self, path: &[usize], cx: &mut Context<Self>) {
        if !self.open.remove(path) {
            self.open.insert(path.to_vec());
        }
        self.rebuild_rows();
        cx.notify();
    }

    fn open_all(&mut self, all: bool, cx: &mut Context<Self>) {
        self.open = match (self.shown(), all) {
            (Some(value), true) => plist::containers(value, usize::MAX, MAX_ROWS),
            (Some(_), false) => [Vec::new()].into_iter().collect(),
            _ => Default::default(),
        };
        self.picked = None;
        self.rebuild_rows();
        cx.notify();
    }

    /// The field that is picked — or, with nothing picked, the whole list.
    fn chosen(&self) -> Option<&plist::Value> {
        let root = self.shown()?;
        Some(
            self.picked
                .as_deref()
                .and_then(|p| root.at(p))
                .unwrap_or(root),
        )
    }

    /// Put something taken out of the picked field on the clipboard.
    fn copy(&self, kind: CopyKind, cx: &mut Context<Self>) {
        let Some(value) = self.chosen() else { return };
        let text = match kind {
            CopyKind::Value => plist::copy_text(value),
            CopyKind::Json => plist::to_json(value),
            CopyKind::Xml => plist::to_xml(value),
            CopyKind::Hex => plist::data_bytes(value).map(plist::hex).unwrap_or_default(),
            CopyKind::Base64 => plist::data_bytes(value)
                .map(plist::base64)
                .unwrap_or_default(),
            CopyKind::Key => {
                // The key of the row: the last step of the path.
                let (root, path) = (self.shown(), self.picked.as_deref().unwrap_or(&[]));
                match (root, path.split_last()) {
                    (Some(root), Some((last, parent))) => root
                        .at(parent)
                        .and_then(|p| p.children().get(*last).map(|(k, _)| k.map(str::to_string)))
                        .flatten()
                        .unwrap_or_else(|| last.to_string()),
                    _ => String::new(),
                }
            }
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    /// Write the picked data field's bytes to a file the person names.
    fn save(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bytes) = self
            .chosen()
            .and_then(plist::data_bytes)
            .map(<[u8]>::to_vec)
        else {
            return;
        };
        let name = {
            let stem = self.file.name();
            let stem = stem.rsplit_once('.').map_or(stem.as_str(), |(s, _)| s);
            format!("{stem}-field.bin")
        };
        let directory = std::env::current_dir().unwrap_or_default();
        let receiver = cx.prompt_for_new_path(&directory, Some(&name));
        window
            .spawn(cx, async move |_| {
                if let Ok(Ok(Some(path))) = receiver.await {
                    let _ = std::fs::write(path, bytes);
                }
            })
            .detach();
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

        match &decoded {
            Decoded::Markdown(text) => return self.render_markdown(text.clone(), &theme, cx),
            Decoded::Plist(doc) => return self.render_plist(doc.clone(), &theme, cx),
            _ => {}
        }

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
            // Drawn by their own renderers above.
            (Decoded::Markdown(_) | Decoded::Plist(_), _) => div().into_any_element(),
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

impl DataViewer {
    /// Two buttons that choose between two views of the same thing.
    fn switch(
        &self,
        ids: [&'static str; 2],
        labels: [&'static str; 2],
        tips: [&'static str; 2],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let one = |ix: usize, text_view: bool| {
            let b = Button::new(ids[ix]).label(labels[ix]).small();
            let b = if self.as_text == text_view {
                b.primary()
            } else {
                b.ghost()
            };
            b.tooltip(tips[ix])
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.as_text = text_view;
                    cx.notify();
                }))
        };
        h_flex().gap_1().child(one(0, false)).child(one(1, true))
    }

    /// Markdown, rendered — or its source.
    fn render_markdown(
        &mut self,
        text: std::sync::Arc<String>,
        theme: &gpui_kit::component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let describe = Decoded::Markdown(text.clone()).describe();
        let toolbar = h_flex()
            .gap_1()
            .p_1()
            .items_center()
            .child(self.switch(
                ["md-rendered", "md-source"],
                ["Rendered", "Source"],
                ["Formatted text", "The Markdown as written"],
                cx,
            ))
            .child(div().flex_1())
            .child(
                div()
                    .px_2()
                    .text_color(theme.muted_foreground)
                    .child(format!("{describe} · {}", human_size(self.bytes))),
            );
        let body: AnyElement = if self.as_text {
            div()
                .p_3()
                .font_family(theme.mono_font_family.clone())
                .text_size(theme.mono_font_size)
                .child(gpui_kit::base::SelectableText::new(
                    SharedString::from(format!("data-md-source-{}", self.file.path)),
                    text.to_string(),
                ))
                .into_any_element()
        } else {
            div()
                .p_4()
                .max_w(px(900.))
                .child(
                    TextView::markdown(format!("data-md-{}", self.file.path), text.to_string())
                        .style(crate::theme::text_view_style(theme))
                        .on_link_click(|url, _, _, cx| {
                            // A page opens in the browser; a relative link has nowhere to go
                            // from here.
                            if url.starts_with("http://") || url.starts_with("https://") {
                                cx.open_url(url);
                            }
                        }),
                )
                .into_any_element()
        };
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .child(toolbar)
            .child(
                div()
                    .id("data-markdown")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .into_any_element()
    }

    /// A property list: a tree to open and close, or its XML.
    fn render_plist(
        &mut self,
        doc: std::sync::Arc<plist::Document>,
        theme: &gpui_kit::component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = theme.muted_foreground;
        let value = doc.shown(self.raw).clone();
        let is_archive = doc.archive.is_some();
        let toolbar = h_flex()
            .gap_1()
            .p_1()
            .items_center()
            .child(self.switch(
                ["plist-tree", "plist-text"],
                ["Tree", "XML"],
                [
                    "Open and close the list's levels",
                    "The list as an XML property list",
                ],
                cx,
            ))
            .when(!self.as_text, |row| {
                row.child(
                    Button::new("plist-open-all")
                        .label("Open all")
                        .ghost()
                        .small()
                        .tooltip("Open every level (up to a few thousand rows)")
                        .on_click(cx.listener(|this, _, _, cx| this.open_all(true, cx))),
                )
                .child(
                    Button::new("plist-close-all")
                        .label("Close all")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.open_all(false, cx))),
                )
            })
            // A keyed archive: the objects it holds, or the table it is kept in.
            .when(is_archive, |row| {
                let one = |id: &'static str, label: &'static str, tip: &'static str, raw: bool| {
                    let b = Button::new(id).label(label).small();
                    let b = if self.raw == raw {
                        b.primary()
                    } else {
                        b.ghost()
                    };
                    b.tooltip(tip).on_click(cx.listener(move |this, _, _, cx| {
                        this.raw = raw;
                        this.open_first_levels();
                        cx.notify();
                    }))
                };
                row.child(div().w(px(8.)))
                    .child(one(
                        "plist-objects",
                        "Objects",
                        "What the keyed archive holds: its objects put back together",
                        false,
                    ))
                    .child(one(
                        "plist-raw",
                        "Raw",
                        "The archive as written: the $objects table, with references by number",
                        true,
                    ))
            })
            .child(div().flex_1())
            .child(div().px_2().text_color(muted).child(format!(
                "{} · {}",
                Decoded::Plist(doc.clone()).describe(),
                human_size(self.bytes)
            )));
        // What is picked, and what can be taken out of it.
        let picked_bar = self.picked.clone().map(|path| {
            let chosen = value.at(&path);
            let is_data = chosen.and_then(plist::data_bytes).is_some();
            let has_key = path.last().is_some();
            let button = |id: &'static str, label: &'static str, tip: &'static str| {
                Button::new(id).label(label).ghost().small().tooltip(tip)
            };
            h_flex()
                .gap_1()
                .px_1()
                .pb_1()
                .items_center()
                .flex_wrap()
                .child(
                    div()
                        .px_1()
                        .max_w(px(360.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(theme.info)
                        .child(SharedString::from(plist::path_label(&value, &path))),
                )
                .child(
                    button(
                        "copy-value",
                        "Copy value",
                        "A string as it is, a number or date as written, data as hex, a structure as JSON",
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.copy(CopyKind::Value, cx))),
                )
                .when(has_key, |r| {
                    r.child(
                        button("copy-key", "Copy key", "The key, or the position in a list")
                            .on_click(cx.listener(|this, _, _, cx| this.copy(CopyKind::Key, cx))),
                    )
                })
                .child(
                    button("copy-json", "JSON", "Copy as JSON (data as {\"$data\": base64})")
                        .on_click(cx.listener(|this, _, _, cx| this.copy(CopyKind::Json, cx))),
                )
                .child(
                    button("copy-xml", "XML", "Copy as an XML property list")
                        .on_click(cx.listener(|this, _, _, cx| this.copy(CopyKind::Xml, cx))),
                )
                .when(is_data, |r| {
                    r.child(
                        button("copy-hex", "Hex", "Copy the bytes as hex")
                            .on_click(cx.listener(|this, _, _, cx| this.copy(CopyKind::Hex, cx))),
                    )
                    .child(
                        button("copy-base64", "Base64", "Copy the bytes as base64")
                            .on_click(cx.listener(|this, _, _, cx| this.copy(CopyKind::Base64, cx))),
                    )
                    .child(
                        button("save-data", "Save…", "Write the bytes to a file")
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                })
        });

        let body: AnyElement = if self.as_text {
            let mut xml = plist::to_xml(&value);
            if xml.chars().count() > MAX_TEXT {
                xml = format!(
                    "{}\n… cut here: the whole list is more than {MAX_TEXT} characters",
                    xml.chars().take(MAX_TEXT).collect::<String>()
                );
            }
            div()
                .id("data-plist-xml")
                .size_full()
                .overflow_y_scroll()
                .p_3()
                .font_family(theme.mono_font_family.clone())
                .text_size(theme.mono_font_size)
                .child(gpui_kit::base::SelectableText::new(
                    SharedString::from(format!("data-plist-xml-{}", self.file.path)),
                    xml,
                ))
                .into_any_element()
        } else {
            let (info, accent, foreground, hover, selected_bg) = (
                theme.info,
                theme.accent,
                theme.foreground,
                theme.list_hover,
                theme.info.opacity(0.25),
            );
            let list = uniform_list(
                "plist-rows",
                self.rows.len(),
                cx.processor(move |viewer, range: std::ops::Range<usize>, _window, cx| {
                    range
                        .filter_map(|ix| viewer.rows.get(ix).cloned().map(|row| (ix, row)))
                        .map(|(ix, row)| {
                            let picked = viewer.picked.as_deref() == Some(row.path.as_slice());
                            let (path, container) = (row.path.clone(), row.container);
                            let chevron_path = row.path.clone();
                            h_flex()
                                .id(("plist-row", ix))
                                .w_full()
                                .gap_2()
                                .items_center()
                                .pl(px(8. + 16. * row.depth as f32))
                                .pr_2()
                                .cursor_pointer()
                                .when(picked, |r| r.bg(selected_bg))
                                .when(!picked, |r| r.hover(|s| s.bg(hover)))
                                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                                    this.picked = Some(path.clone());
                                    // A double click opens or closes a container.
                                    if container && event.click_count() >= 2 {
                                        this.toggle(&path, cx);
                                    }
                                    cx.notify();
                                }))
                                .child(div().id(("plist-chevron", ix)).flex_none().w(px(14.)).when(
                                    container,
                                    |d| {
                                        d.child(
                                            Icon::new(if row.open {
                                                IconName::ChevronDown
                                            } else {
                                                IconName::ChevronRight
                                            })
                                            .small(),
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.toggle(&chevron_path, cx);
                                            }),
                                        )
                                    },
                                ))
                                .when_some(row.key.clone(), |r, key| {
                                    r.child(
                                        div()
                                            .flex_none()
                                            .text_color(info)
                                            .child(SharedString::from(format!("{key}:"))),
                                    )
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_color(if row.container { muted } else { foreground })
                                        .child(SharedString::from(row.summary.clone())),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(px(11.))
                                        .text_color(accent.opacity(0.8))
                                        .child(row.kind),
                                )
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .size_full();
            div()
                .size_full()
                .relative()
                .child(list)
                .vertical_scrollbar(&self.scroll)
                .into_any_element()
        };
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .child(toolbar)
            .children(picked_bar)
            .child(div().flex_1().min_h_0().bg(theme.tokens.table).child(body))
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
        panel_title_closing(
            self.view.read(cx).icon(),
            &self.view.read(cx).title(),
            closer(&self.app, cx.entity()),
        )
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
