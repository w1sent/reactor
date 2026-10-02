//! The hex viewer: a file as offset, bytes and text, scrolled lazily so a multi-gigabyte
//! image opens as fast as a small file. Searches for text or hex bytes, forward and back.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{Panel, PanelEvent};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{Entity, EventEmitter, FocusHandle, Focusable, Render, Window, div, px};

use reactor_fs::{FileRef, ReadAt, find_bytes, parse_hex};

use crate::app::ReactorApp;
use crate::panels::{close_button, panel_title_row};
use crate::settings::Slot;

const PER_ROW: u64 = 16;
const PAGE: u64 = 4096;
/// Pages kept in memory; the rest are read again when scrolled back to.
const CACHED_PAGES: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Text,
    Hex,
}

pub struct HexPanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    file: FileRef,
    reader: Result<Arc<dyn ReadAt>, String>,
    len: u64,
    pages: RefCell<HashMap<u64, Arc<Vec<u8>>>>,
    scroll: UniformListScrollHandle,
    query: Entity<InputState>,
    mode: Mode,
    /// The match on screen: where it starts and how long it is.
    found: Option<(u64, usize)>,
    status: String,
    searching: Option<Arc<AtomicBool>>,
    focus: FocusHandle,
}

impl HexPanel {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        file: FileRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Find text…"));
        cx.subscribe_in(
            &query,
            window,
            |this, _, event: &InputEvent, _window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.search(true, cx);
                }
            },
        )
        .detach();
        let reader = file
            .open()
            .map(Arc::<dyn ReadAt>::from)
            .map_err(|e| e.to_string());
        let len = reader.as_ref().map(|r| r.len()).unwrap_or(0);
        Self {
            app,
            file,
            reader,
            len,
            pages: RefCell::default(),
            scroll: UniformListScrollHandle::default(),
            query,
            mode: Mode::Text,
            found: None,
            status: String::new(),
            searching: None,
            focus: cx.focus_handle(),
        }
    }

    fn rows(&self) -> usize {
        self.len.div_ceil(PER_ROW) as usize
    }

    /// The bytes of page `index`, read on first use.
    fn page(&self, index: u64) -> Arc<Vec<u8>> {
        if let Some(page) = self.pages.borrow().get(&index) {
            return page.clone();
        }
        let bytes = self
            .reader
            .as_ref()
            .ok()
            .and_then(|r| r.read_range(index * PAGE, PAGE as usize).ok())
            .unwrap_or_default();
        let page = Arc::new(bytes);
        let mut pages = self.pages.borrow_mut();
        if pages.len() >= CACHED_PAGES {
            pages.clear();
        }
        pages.insert(index, page.clone());
        page
    }

    fn row_bytes(&self, row: u64) -> Vec<u8> {
        let start = row * PER_ROW;
        let page = self.page(start / PAGE);
        let at = (start % PAGE) as usize;
        page.get(at..)
            .map(|rest| rest[..rest.len().min(PER_ROW as usize)].to_vec())
            .unwrap_or_default()
    }

    fn pattern(&self, cx: &App) -> Result<Vec<u8>, String> {
        let text = self.query.read(cx).value().to_string();
        match self.mode {
            Mode::Text if text.is_empty() => Err("type something to find".into()),
            Mode::Text => Ok(text.into_bytes()),
            Mode::Hex => parse_hex(&text).ok_or_else(|| "not a whole number of hex bytes".into()),
        }
    }

    fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = mode;
        let placeholder = match mode {
            Mode::Text => "Find text…",
            Mode::Hex => "Find hex bytes, e.g. 7f 45 4c 46",
        };
        self.query.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx)
        });
        cx.notify();
    }

    /// Finds the next or previous match from the current one, in the background.
    fn search(&mut self, forward: bool, cx: &mut Context<Self>) {
        let pattern = match self.pattern(cx) {
            Ok(pattern) => pattern,
            Err(message) => {
                self.status = message;
                cx.notify();
                return;
            }
        };
        let Ok(reader) = self.reader.clone() else {
            return;
        };
        if let Some(previous) = self.searching.take() {
            previous.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.searching = Some(cancel.clone());
        let from = match (self.found, forward) {
            (Some((at, _)), true) => at + 1,
            (Some((at, _)), false) => at,
            (None, true) => 0,
            (None, false) => self.len,
        };
        self.status = "searching…".into();
        let length = pattern.len();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { find_bytes(reader.as_ref(), &pattern, from, forward, &cancel) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.searching = None;
                match result {
                    Ok(Some(found)) => {
                        this.found = Some((found.offset, length));
                        this.status = format!(
                            "0x{:x}{}",
                            found.offset,
                            if found.wrapped { " (wrapped)" } else { "" }
                        );
                        this.scroll.scroll_to_item(
                            (found.offset / PER_ROW) as usize,
                            gpui_kit::ScrollStrategy::Center,
                        );
                    }
                    Ok(None) => this.status = "not found".into(),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => this.status = format!("search failed: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Focusable for HexPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<PanelEvent> for HexPanel {}

impl gpui_kit::base::dock::Panel for HexPanel {
    fn panel_name(&self) -> &'static str {
        "hex"
    }
}

impl Panel for HexPanel {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_row(IconName::Binary, &format!("{} (hex)", self.file.name()))
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        Some(vec![close_button(&self.app, cx.entity())])
    }
}

impl Render for HexPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let ui = self
            .app
            .upgrade()
            .map(|a| a.read(cx).ui.clone())
            .unwrap_or_default();
        let (family, size) = ui.text(Slot::Mono, &theme);

        if let Err(message) = &self.reader {
            return div()
                .p_3()
                .text_color(theme.danger)
                .child(format!("could not open {}: {message}", self.file.path))
                .into_any_element();
        }

        let mode = self.mode;
        let toolbar = h_flex()
            .gap_1()
            .p_1()
            .items_center()
            .child(div().flex_1().child(Input::new(&self.query).small()))
            .child(
                Button::new("hex-mode-text")
                    .label("Text")
                    .small()
                    .when(mode == Mode::Text, |b| b.primary())
                    .when(mode != Mode::Text, |b| b.ghost())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.set_mode(Mode::Text, window, cx)),
                    ),
            )
            .child(
                Button::new("hex-mode-hex")
                    .label("Hex")
                    .small()
                    .when(mode == Mode::Hex, |b| b.primary())
                    .when(mode != Mode::Hex, |b| b.ghost())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.set_mode(Mode::Hex, window, cx)),
                    ),
            )
            .child(
                Button::new("hex-prev")
                    .icon(IconName::ChevronUp)
                    .ghost()
                    .small()
                    .tooltip("Previous match")
                    .on_click(cx.listener(|this, _, _, cx| this.search(false, cx))),
            )
            .child(
                Button::new("hex-next")
                    .icon(IconName::ChevronDown)
                    .ghost()
                    .small()
                    .tooltip("Next match (Enter)")
                    .on_click(cx.listener(|this, _, _, cx| this.search(true, cx))),
            );

        let found = self.found;
        let muted = theme.muted_foreground;
        let highlight = theme.info.opacity(0.35);
        let list = uniform_list(
            "hex-rows",
            self.rows(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, _cx| {
                range
                    .map(|row| {
                        let base = row as u64 * PER_ROW;
                        let bytes = this.row_bytes(row as u64);
                        let marked = |i: usize| {
                            found.is_some_and(|(at, len)| {
                                let pos = base + i as u64;
                                pos >= at && pos < at + len as u64
                            })
                        };
                        let cell = |i: usize| {
                            let text = bytes.get(i).map(|b| format!("{b:02x}")).unwrap_or_default();
                            div()
                                .w(px(size.as_f32() * 1.25))
                                .rounded_sm()
                                .when(marked(i), |d| d.bg(highlight))
                                .child(text)
                        };
                        let ascii: Vec<_> = bytes
                            .iter()
                            .enumerate()
                            .map(|(i, b)| {
                                let c = if (0x20..0x7f).contains(b) {
                                    *b as char
                                } else {
                                    '.'
                                };
                                div()
                                    .w(px(size.as_f32() * 0.62))
                                    .rounded_sm()
                                    .when(marked(i), |d| d.bg(highlight))
                                    .child(c.to_string())
                            })
                            .collect();
                        h_flex()
                            .id(("hex-row", row))
                            .gap_3()
                            .px_2()
                            .child(div().text_color(muted).child(format!("{base:08x}")))
                            .child(
                                h_flex()
                                    .gap(px(size.as_f32() * 0.35))
                                    .children((0..16).map(cell)),
                            )
                            .child(h_flex().children(ascii))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full();

        v_flex()
            .size_full()
            .font_family(family)
            .text_size(size)
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(list)
                    .vertical_scrollbar(&self.scroll),
            )
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_3()
                    .text_color(muted)
                    .child(format!("{} bytes", self.len))
                    .child(self.status.clone()),
            )
            .into_any_element()
    }
}
