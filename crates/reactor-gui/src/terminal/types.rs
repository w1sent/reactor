//! The terminal's own small types: what the view, the keys and the mouse code see.
//!
//! Nothing here names the emulation crate; `backend.rs` converts into these. Upgrading the crate,
//! or putting another engine under it, touches that one file.

use std::sync::Arc;

/// The grid's size in cells, and the size of a cell in pixels (the program may ask for it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub rows: u16,
    pub cols: u16,
    pub cell_w: u16,
    pub cell_h: u16,
}

impl TermSize {
    pub fn new(rows: u16, cols: u16, cell_w: u16, cell_h: u16) -> Self {
        Self {
            rows: rows.max(1),
            cols: cols.max(1),
            cell_w,
            cell_h,
        }
    }
}

/// Which mouse events a program asked to be told about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseMode {
    #[default]
    Off,
    /// Presses and releases (1000).
    Click,
    /// And motion while a button is down (1002).
    Drag,
    /// And all motion (1003).
    Motion,
}

/// How the coordinates of a mouse report are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseEncoding {
    /// `ESC [ M` and three bytes; coordinates stop at 223.
    #[default]
    Normal,
    /// The same, with the coordinates as UTF-8 (1005).
    Utf8,
    /// `ESC [ < b ; x ; y M` or `m` (1006).
    Sgr,
}

/// The modes of the program that the keys and the mouse depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modes {
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub bracketed_paste: bool,
    pub focus_in_out: bool,
    pub alt_screen: bool,
    /// The wheel sends arrow keys on the alternate screen (1007).
    pub alternate_scroll: bool,
    pub mouse: MouseMode,
    pub mouse_encoding: MouseEncoding,
}

/// A colour as the program set it, before the theme gives it a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    DefaultFg,
    DefaultBg,
    /// 0 to 7 normal, 8 to 15 bright, 16 to 255 the cube and the greys.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// Attributes of a cell, packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Attrs(pub u16);

impl Attrs {
    pub const BOLD: u16 = 1;
    pub const ITALIC: u16 = 1 << 1;
    pub const INVERSE: u16 = 1 << 2;
    pub const DIM: u16 = 1 << 3;
    pub const HIDDEN: u16 = 1 << 4;
    pub const STRIKE: u16 = 1 << 5;
    /// The first half of a double-width character.
    pub const WIDE: u16 = 1 << 6;
    /// The second half of one, or the blank that stands where a wide character did not fit.
    pub const SPACER: u16 = 1 << 7;
    pub const UNDERLINE: u16 = 1 << 8;
    pub const DOUBLE_UNDERLINE: u16 = 1 << 9;
    pub const CURL: u16 = 1 << 10;
    pub const DOTTED: u16 = 1 << 11;
    pub const DASHED: u16 = 1 << 12;
    pub const ANY_UNDERLINE: u16 =
        Self::UNDERLINE | Self::DOUBLE_UNDERLINE | Self::CURL | Self::DOTTED | Self::DASHED;

    pub fn has(self, flag: u16) -> bool {
        self.0 & flag != 0
    }
}

/// What a cell has beyond a character and two colours; most cells have none of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extra {
    /// Combining marks and joiners drawn with the character.
    pub zerowidth: Vec<char>,
    pub underline_color: Option<Color>,
    /// The target of an OSC 8 hyperlink.
    pub link: Option<Arc<str>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub c: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
    pub extra: Option<Box<Extra>>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            c: ' ',
            fg: Color::DefaultFg,
            bg: Color::DefaultBg,
            attrs: Attrs(0),
            extra: None,
        }
    }
}

impl Cell {
    pub fn is_blank(&self) -> bool {
        (self.c == ' ' || self.c == '\0') && self.extra.is_none()
    }

    pub fn link(&self) -> Option<&Arc<str>> {
        self.extra.as_ref().and_then(|e| e.link.as_ref())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Beam,
    Hollow,
    Hidden,
}

/// Where the cursor is in the viewport, and what the program says it looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub shape: CursorShape,
    pub row: usize,
    pub col: usize,
    /// Whether it is inside the viewport (it is not when the history is scrolled).
    pub in_view: bool,
    pub blinking: bool,
}

/// Which cells a selection covers, in viewport rows (negative above the view).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelRange {
    pub start: (i32, usize),
    pub end: (i32, usize),
    pub block: bool,
}

impl SelRange {
    pub fn contains(&self, row: i32, col: usize) -> bool {
        let (sr, sc) = self.start;
        let (er, ec) = self.end;
        if row < sr.min(er) || row > sr.max(er) {
            return false;
        }
        if self.block {
            return col >= sc.min(ec) && col <= sc.max(ec);
        }
        if row == sr && row == er {
            return col >= sc && col <= ec;
        }
        if row == sr {
            return col >= sc;
        }
        if row == er {
            return col <= ec;
        }
        true
    }
}

/// A match of a search, in viewport rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchRange {
    pub start: (i32, usize),
    pub end: (i32, usize),
}

impl MatchRange {
    pub fn contains(&self, row: i32, col: usize) -> bool {
        SelRange {
            start: self.start,
            end: self.end,
            block: false,
        }
        .contains(row, col)
    }
}

/// A copy of what is on screen, taken under the lock and painted without it.
#[derive(Debug, Clone)]
pub struct Content {
    pub rows: usize,
    pub cols: usize,
    /// `rows * cols` cells, row by row.
    pub cells: Vec<Cell>,
    pub cursor: Cursor,
    pub selection: Option<SelRange>,
    pub matches: Vec<MatchRange>,
    /// How many rows the view is scrolled up into the history; 0 is the bottom.
    pub display_offset: usize,
    /// Rows of history above the screen.
    pub history: usize,
    pub modes: Modes,
    /// Colours the program changed (OSC 4, 10, 11, 12), by palette index; see `PALETTE_*`.
    pub palette: Vec<Option<[u8; 3]>>,
}

/// The palette index the foreground, the background and the cursor have.
pub const PALETTE_FG: usize = 256;
pub const PALETTE_BG: usize = 257;
pub const PALETTE_CURSOR: usize = 258;

impl Content {
    pub fn empty(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            cells: vec![Cell::default(); rows * cols],
            cursor: Cursor {
                shape: CursorShape::Hidden,
                row: 0,
                col: 0,
                in_view: false,
                blinking: false,
            },
            selection: None,
            matches: Vec::new(),
            display_offset: 0,
            history: 0,
            modes: Modes::default(),
            palette: Vec::new(),
        }
    }

    pub fn row(&self, row: usize) -> &[Cell] {
        &self.cells[row * self.cols..(row + 1) * self.cols]
    }

    pub fn cell(&self, row: usize, col: usize) -> &Cell {
        &self.cells[row * self.cols + col]
    }

    /// The text of a row, a blank for a cell with none, and nothing for the second half of a
    /// wide character.
    pub fn row_text(&self, row: usize) -> String {
        let mut text = String::with_capacity(self.cols);
        for cell in self.row(row) {
            if cell.attrs.has(Attrs::SPACER) {
                continue;
            }
            text.push(if cell.c == '\0' { ' ' } else { cell.c });
            if let Some(extra) = &cell.extra {
                text.extend(extra.zerowidth.iter());
            }
        }
        text
    }

    pub fn palette_at(&self, index: usize) -> Option<[u8; 3]> {
        self.palette.get(index).copied().flatten()
    }
}

/// How a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

impl ExitInfo {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// A reply the program waits for; written to the pty in the order the questions came.
pub type ColorReply = Arc<dyn Fn([u8; 3]) -> String + Send + Sync>;
pub type SizeReply = Arc<dyn Fn(TermSize) -> String + Send + Sync>;

/// What the engine tells the panel.
#[derive(Clone)]
pub enum Event {
    /// There is something new to draw.
    Wakeup,
    Bell,
    /// The program's title, or `None` when it reset it.
    Title(Option<String>),
    /// An answer to a query (cursor position, device attributes, keyboard mode).
    PtyWrite(String),
    ColorRequest {
        index: usize,
        reply: ColorReply,
    },
    SizeRequest(SizeReply),
    /// OSC 52: the program set the clipboard.
    ClipboardStore(String),
    CursorBlink,
    ChildExit(ExitInfo),
    /// The loop ended; nothing more can be written.
    Exit,
    /// The loop's thread is gone; `panicked` when it did not end by itself.
    Closed {
        panicked: bool,
    },
}

impl std::fmt::Debug for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Event::Wakeup => write!(f, "Wakeup"),
            Event::Bell => write!(f, "Bell"),
            Event::Title(t) => write!(f, "Title({t:?})"),
            Event::PtyWrite(s) => write!(f, "PtyWrite({s:?})"),
            Event::ColorRequest { index, .. } => write!(f, "ColorRequest({index})"),
            Event::SizeRequest(_) => write!(f, "SizeRequest"),
            Event::ClipboardStore(s) => write!(f, "ClipboardStore({s:?})"),
            Event::CursorBlink => write!(f, "CursorBlink"),
            Event::ChildExit(e) => write!(f, "ChildExit({e:?})"),
            Event::Exit => write!(f, "Exit"),
            Event::Closed { panicked } => write!(f, "Closed({panicked})"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelKind {
    Simple,
    /// A word, by the engine's idea of one.
    Word,
    Line,
    Block,
}

/// Which half of a cell a pointer is in; it decides whether the cell is in the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scrolling {
    /// Positive towards the history.
    Lines(i32),
    PageUp,
    PageDown,
    Top,
    Bottom,
}

/// What a person can set about the grid itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Rows of history kept.
    pub scrollback: usize,
    pub cursor_shape: CursorShape,
    /// Whether the cursor blinks until a program says otherwise.
    pub cursor_blink: bool,
}

/// The most history a console may keep; it is held in memory for every console that ran.
pub const MAX_SCROLLBACK: usize = 100_000;
pub const DEFAULT_SCROLLBACK: usize = 10_000;

impl Default for Settings {
    fn default() -> Self {
        Self {
            scrollback: DEFAULT_SCROLLBACK,
            cursor_shape: CursorShape::Block,
            cursor_blink: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_covers_whole_rows_between_its_ends() {
        let s = SelRange {
            start: (1, 4),
            end: (3, 2),
            block: false,
        };
        assert!(!s.contains(0, 9));
        assert!(!s.contains(1, 3));
        assert!(s.contains(1, 4));
        assert!(s.contains(2, 0));
        assert!(s.contains(3, 2));
        assert!(!s.contains(3, 3));
    }

    #[test]
    fn a_block_selection_is_a_rectangle() {
        let s = SelRange {
            start: (1, 6),
            end: (3, 2),
            block: true,
        };
        assert!(s.contains(2, 4));
        assert!(!s.contains(2, 7));
        assert!(!s.contains(0, 4));
    }
}
