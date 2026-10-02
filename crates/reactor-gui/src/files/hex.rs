//! The hex viewer: a file as offset, bytes and text, scrolled lazily so a multi-gigabyte
//! image opens as fast as a small file. Select in either column (the other follows), copy
//! the selection, and search for text or hex bytes with every match marked.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::actions;
use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{Panel, PanelEvent};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{
    ClipboardItem, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, MouseButton, Render,
    Window, div, px,
};

use reactor_fs::{FileRef, ReadAt, find_all, parse_hex};

use crate::app::ReactorApp;
use crate::panels::{close_button, panel_title_row};
use crate::settings::Slot;

actions!(
    reactor_hex,
    [
        /// Ctrl/Cmd+C in a hex viewer: copy the selection as the column it was made in.
        CopySelection,
        /// Ctrl/Cmd+A: select the whole file.
        SelectAllBytes
    ]
);

const CONTEXT: &str = "ReactorHex";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-c", CopySelection, Some(CONTEXT)),
        KeyBinding::new("secondary-a", SelectAllBytes, Some(CONTEXT)),
    ]);
}

const PER_ROW: u64 = 16;
const PAGE: u64 = 4096;
/// Pages kept in memory; the rest are read again when scrolled back to.
const CACHED_PAGES: usize = 128;
/// Matches kept for highlighting; past this the count reads "N+".
const MATCH_LIMIT: usize = 100_000;
/// The most a copy will take.
const COPY_LIMIT: u64 = 16 << 20;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Text,
    Hex,
}

/// Which column a selection was made in — what a copy produces.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Column {
    Hex,
    Text,
}

/// The selected bytes, inclusive: where the drag began and where it is.
#[derive(Clone, Copy)]
struct Selection {
    anchor: u64,
    cursor: u64,
    column: Column,
}

impl Selection {
    fn range(&self) -> (u64, u64) {
        (self.anchor.min(self.cursor), self.anchor.max(self.cursor))
    }
}

/// The result of a search: every start offset, and how long the pattern is.
struct Matches {
    pattern: Vec<u8>,
    starts: Vec<u64>,
    truncated: bool,
    /// Index into `starts` of the match shown as current.
    current: Option<usize>,
}

impl Matches {
    /// Whether byte `at` is inside a match, and whether it is the current one.
    fn covers(&self, at: u64) -> (bool, bool) {
        let ix = self.starts.partition_point(|start| *start <= at);
        // Matches can overlap, so the covering one may be a few back.
        for back in 1..=self.pattern.len().min(ix) {
            let m = ix - back;
            if self.starts[m] + self.pattern.len() as u64 > at {
                return (true, self.current == Some(m));
            }
        }
        (false, false)
    }
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
    selection: Option<Selection>,
    dragging: bool,
    matches: Option<Matches>,
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
                    this.step(true, cx);
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
            selection: None,
            dragging: false,
            matches: None,
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

    // -- selection and copy -----------------------------------------------------------

    fn press(
        &mut self,
        at: u64,
        column: Column,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.dragging = true;
        self.selection = match self.selection {
            Some(sel) if extend => Some(Selection {
                cursor: at,
                column,
                ..sel
            }),
            _ => Some(Selection {
                anchor: at,
                cursor: at,
                column,
            }),
        };
        cx.notify();
    }

    fn drag_to(&mut self, at: u64, cx: &mut Context<Self>) {
        if let Some(sel) = &mut self.selection
            && self.dragging
            && sel.cursor != at
        {
            sel.cursor = at;
            cx.notify();
        }
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.len > 0 {
            let column = self.selection.map_or(Column::Hex, |s| s.column);
            self.selection = Some(Selection {
                anchor: 0,
                cursor: self.len - 1,
                column,
            });
            cx.notify();
        }
    }

    /// The selection as text: `de ad be ef`, or the printable characters with `.` for the rest.
    fn selected_text(&self, as_column: Column) -> Result<String, String> {
        let sel = self.selection.ok_or("nothing is selected")?;
        let (from, to) = sel.range();
        let count = to - from + 1;
        if count > COPY_LIMIT {
            return Err(format!(
                "the selection is over the {} MiB copy limit",
                COPY_LIMIT >> 20
            ));
        }
        let reader = self.reader.as_ref().map_err(String::clone)?;
        let bytes = reader
            .read_range(from, count as usize)
            .map_err(|e| e.to_string())?;
        Ok(match as_column {
            Column::Hex => bytes
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" "),
            Column::Text => bytes
                .iter()
                .map(|b| {
                    if (0x20..0x7f).contains(b) {
                        *b as char
                    } else {
                        '.'
                    }
                })
                .collect(),
        })
    }

    fn copy(&mut self, as_column: Option<Column>, cx: &mut Context<Self>) {
        let column = as_column
            .or(self.selection.map(|s| s.column))
            .unwrap_or(Column::Hex);
        match self.selected_text(column) {
            Ok(text) => {
                let bytes = self.selection.map_or(0, |s| s.range().1 - s.range().0 + 1);
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.status = format!("copied {bytes} bytes");
            }
            Err(message) => self.status = message,
        }
        cx.notify();
    }

    // -- search -----------------------------------------------------------------------

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
        self.matches = None;
        let placeholder = match mode {
            Mode::Text => "Find text…",
            Mode::Hex => "Find hex bytes, e.g. 7f 45 4c 46",
        };
        self.query.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx)
        });
        cx.notify();
    }

    /// Where a search starts from: the current match, else the selection, else the top.
    fn origin(&self) -> u64 {
        self.matches
            .as_ref()
            .and_then(|m| m.current.map(|i| m.starts[i]))
            .or(self.selection.map(|s| s.range().0))
            .unwrap_or(0)
    }

    /// Go to the next or previous match. The first call for a pattern finds them all.
    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let pattern = match self.pattern(cx) {
            Ok(pattern) => pattern,
            Err(message) => {
                self.status = message;
                cx.notify();
                return;
            }
        };
        if self.matches.as_ref().is_some_and(|m| m.pattern == pattern) {
            self.move_current(forward, cx);
            return;
        }
        let Ok(reader) = self.reader.clone() else {
            return;
        };
        if let Some(previous) = self.searching.take() {
            previous.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.searching = Some(cancel.clone());
        self.status = "searching…".into();
        // The first match to show is the nearest one from where the user is.
        let origin = self.origin();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let searched = pattern.clone();
            let result = cx
                .background_executor()
                .spawn(async move { find_all(reader.as_ref(), &searched, MATCH_LIMIT, &cancel) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.searching = None;
                match result {
                    Ok((starts, truncated)) => {
                        let first = if forward {
                            starts.iter().position(|s| *s >= origin)
                        } else {
                            starts.iter().rposition(|s| *s < origin)
                        };
                        let current = first.or(if starts.is_empty() {
                            None
                        } else if forward {
                            Some(0)
                        } else {
                            Some(starts.len() - 1)
                        });
                        this.matches = Some(Matches {
                            pattern,
                            starts,
                            truncated,
                            current,
                        });
                        this.show_current(cx);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => {
                        this.status = format!("search failed: {e}");
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn move_current(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some(m) = &mut self.matches
            && !m.starts.is_empty()
        {
            let count = m.starts.len();
            m.current = Some(match (m.current, forward) {
                (Some(i), true) => (i + 1) % count,
                (Some(i), false) => (i + count - 1) % count,
                (None, true) => 0,
                (None, false) => count - 1,
            });
        }
        self.show_current(cx);
    }

    /// Scroll the current match into view and say which one it is.
    fn show_current(&mut self, cx: &mut Context<Self>) {
        let Some(m) = &self.matches else { return };
        match m.current {
            Some(i) => {
                let plus = if m.truncated { "+" } else { "" };
                self.status = format!("match {} of {}{plus}", i + 1, m.starts.len());
                self.scroll.scroll_to_item(
                    (m.starts[i] / PER_ROW) as usize,
                    gpui_kit::ScrollStrategy::Center,
                );
            }
            None => self.status = "not found".into(),
        }
        cx.notify();
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
        let has_selection = self.selection.is_some();
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
                    .on_click(cx.listener(|this, _, _, cx| this.step(false, cx))),
            )
            .child(
                Button::new("hex-next")
                    .icon(IconName::ChevronDown)
                    .ghost()
                    .small()
                    .tooltip("Next match (Enter)")
                    .on_click(cx.listener(|this, _, _, cx| this.step(true, cx))),
            )
            .child(
                Button::new("hex-copy-hex")
                    .label("Copy hex")
                    .ghost()
                    .small()
                    .disabled(!has_selection)
                    .tooltip("Copy the selection as hex bytes")
                    .on_click(cx.listener(|this, _, _, cx| this.copy(Some(Column::Hex), cx))),
            )
            .child(
                Button::new("hex-copy-text")
                    .label("Copy text")
                    .ghost()
                    .small()
                    .disabled(!has_selection)
                    .tooltip("Copy the selection as text")
                    .on_click(cx.listener(|this, _, _, cx| this.copy(Some(Column::Text), cx))),
            );

        let muted = theme.muted_foreground;
        let selected_bg = theme.info.opacity(0.4);
        let match_bg = theme.warning.opacity(0.3);
        let current_bg = theme.warning.opacity(0.8);
        let list = uniform_list(
            "hex-rows",
            self.rows(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let selection = this.selection.map(|s| s.range());
                range
                    .map(|row| {
                        let base = row as u64 * PER_ROW;
                        let bytes = this.row_bytes(row as u64);
                        // Selection beats the current match, which beats the other matches.
                        let background = |i: usize| {
                            let at = base + i as u64;
                            if selection.is_some_and(|(from, to)| at >= from && at <= to) {
                                return Some(selected_bg);
                            }
                            match this.matches.as_ref().map(|m| m.covers(at)) {
                                Some((true, true)) => Some(current_bg),
                                Some((true, false)) => Some(match_bg),
                                _ => None,
                            }
                        };
                        // One cell per byte, in either column; both press and drag select.
                        let cell = |i: usize, column: Column, width: f32, text: String| {
                            let at = base + i as u64;
                            div()
                                .id((
                                    if column == Column::Hex { "hx" } else { "tx" },
                                    row * 16 + i,
                                ))
                                .w(px(width))
                                .rounded_sm()
                                .cursor_text()
                                .when_some(background(i), |d, bg| d.bg(bg))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                        this.press(at, column, event.modifiers.shift, window, cx)
                                    }),
                                )
                                .on_mouse_move(cx.listener(
                                    move |this, event: &MouseMoveEvent, _, cx| {
                                        if event.pressed_button == Some(MouseButton::Left) {
                                            this.drag_to(at, cx)
                                        }
                                    },
                                ))
                                .child(text)
                        };
                        let hex_cells: Vec<_> = (0..16)
                            .map(|i| {
                                let text =
                                    bytes.get(i).map(|b| format!("{b:02x}")).unwrap_or_default();
                                if i < bytes.len() {
                                    cell(i, Column::Hex, size.as_f32() * 1.25, text)
                                        .into_any_element()
                                } else {
                                    div().w(px(size.as_f32() * 1.25)).into_any_element()
                                }
                            })
                            .collect();
                        let text_cells: Vec<_> = bytes
                            .iter()
                            .enumerate()
                            .map(|(i, b)| {
                                let c = if (0x20..0x7f).contains(b) {
                                    *b as char
                                } else {
                                    '.'
                                };
                                cell(i, Column::Text, size.as_f32() * 0.62, c.to_string())
                            })
                            .collect();
                        h_flex()
                            .id(("hex-row", row))
                            .gap_3()
                            .px_2()
                            .child(div().text_color(muted).child(format!("{base:08x}")))
                            .child(h_flex().gap(px(size.as_f32() * 0.35)).children(hex_cells))
                            .child(h_flex().children(text_cells))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full();

        let selection_text = self.selection.map(|s| {
            let (from, to) = s.range();
            format!("selected 0x{from:x}–0x{to:x} ({} bytes)", to - from + 1)
        });

        v_flex()
            .size_full()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &CopySelection, _, cx| this.copy(None, cx)))
            .on_action(cx.listener(|this, _: &SelectAllBytes, _, cx| this.select_all(cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.dragging = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.dragging = false),
            )
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
                    .when_some(selection_text, |row, text| row.child(text))
                    .child(div().flex_1())
                    .child(self.status.clone()),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(starts: &[u64], len: usize, current: Option<usize>) -> Matches {
        Matches {
            pattern: vec![0; len],
            starts: starts.to_vec(),
            truncated: false,
            current,
        }
    }

    #[test]
    fn covers_marks_every_byte_of_every_match_and_the_current_one() {
        let m = matches(&[10, 20], 3, Some(1));
        assert_eq!(m.covers(9), (false, false));
        assert_eq!(m.covers(10), (true, false));
        assert_eq!(m.covers(12), (true, false));
        assert_eq!(m.covers(13), (false, false));
        assert_eq!(m.covers(21), (true, true));
        assert_eq!(m.covers(23), (false, false));
    }

    #[test]
    fn covers_handles_overlapping_matches() {
        let m = matches(&[0, 1, 2], 2, Some(2));
        assert_eq!(m.covers(1), (true, false));
        assert_eq!(m.covers(3), (true, true));
        assert_eq!(m.covers(4), (false, false));
    }

    #[test]
    fn selection_range_is_ordered_whichever_way_it_was_dragged() {
        let s = Selection {
            anchor: 9,
            cursor: 4,
            column: Column::Text,
        };
        assert_eq!(s.range(), (4, 9));
    }
}
