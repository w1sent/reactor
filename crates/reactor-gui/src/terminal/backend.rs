//! The emulation engine behind the console. This is the only file that names `alacritty_terminal`
//! (pinned exactly, docs/adr/0068); everything it hands out is one of the types in `types.rs`.
//!
//! The crate's reader thread owns the pty and parses what the program writes into a [`Term`]
//! behind a fair mutex. The panel never reads the pty: it takes the lock for as long as it takes
//! to copy the visible rows ([`Backend::snapshot`]) and paints the copy.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side as TermSide};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::{Cell as TermCell, Flags};
use alacritty_terminal::term::search::{RegexIter, RegexSearch};
use alacritty_terminal::term::{Config, Osc52, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{
    Color as TermColor, CursorShape as TermCursorShape, CursorStyle, NamedColor, Processor, Rgb,
    StdSyncHandler,
};

use super::inbox::Inbox;
use super::types::*;

/// Where the engine's events go.
#[derive(Clone)]
struct Listener {
    inbox: Arc<Inbox>,
}

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let mapped = match event {
            TermEvent::Wakeup => Event::Wakeup,
            TermEvent::Bell => Event::Bell,
            TermEvent::Title(title) => Event::Title(Some(title)),
            TermEvent::ResetTitle => Event::Title(None),
            TermEvent::PtyWrite(text) => Event::PtyWrite(text),
            TermEvent::ColorRequest(index, reply) => Event::ColorRequest {
                index,
                reply: Arc::new(move |rgb| {
                    reply(Rgb {
                        r: rgb[0],
                        g: rgb[1],
                        b: rgb[2],
                    })
                }),
            },
            TermEvent::TextAreaSizeRequest(reply) => {
                Event::SizeRequest(Arc::new(move |size| reply(window_size(size))))
            }
            TermEvent::ClipboardStore(_, text) => Event::ClipboardStore(text),
            TermEvent::CursorBlinkingChange => Event::CursorBlink,
            TermEvent::ChildExit(status) => Event::ChildExit(exit_info(status)),
            TermEvent::Exit => Event::Exit,
            // A program may not read the clipboard (the policy is copy only, so the engine does
            // not send this), and the pointer shape is decided on paint.
            TermEvent::ClipboardLoad(..) | TermEvent::MouseCursorDirty => return,
        };
        self.inbox.push(mapped);
    }
}

fn exit_info(status: std::process::ExitStatus) -> ExitInfo {
    #[cfg(unix)]
    let signal = std::os::unix::process::ExitStatusExt::signal(&status);
    #[cfg(not(unix))]
    let signal = None;
    ExitInfo {
        code: status.code(),
        signal,
    }
}

fn window_size(size: TermSize) -> WindowSize {
    WindowSize {
        num_lines: size.rows,
        num_cols: size.cols,
        cell_width: size.cell_w,
        cell_height: size.cell_h,
    }
}

/// The grid's dimensions, for creating and resizing a [`Term`].
struct GridSize {
    rows: usize,
    cols: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

fn config(settings: &Settings) -> Config {
    Config {
        scrolling_history: settings.scrollback.min(MAX_SCROLLBACK),
        default_cursor_style: CursorStyle {
            shape: match settings.cursor_shape {
                CursorShape::Block => TermCursorShape::Block,
                CursorShape::Underline => TermCursorShape::Underline,
                CursorShape::Beam => TermCursorShape::Beam,
                CursorShape::Hollow => TermCursorShape::HollowBlock,
                CursorShape::Hidden => TermCursorShape::Hidden,
            },
            blinking: settings.cursor_blink,
        },
        // A program may set the clipboard but not read it.
        osc52: Osc52::OnlyCopy,
        // The keyboard is encoded for xterm. The crate's Kitty mode stack has a known defect
        // with unpopped pushes, and nothing here sends Kitty keys.
        kitty_keyboard: false,
        ..Config::default()
    }
}

#[derive(Default)]
struct SearchState {
    pattern: String,
    regex: Option<RegexSearch>,
    current: Option<std::ops::RangeInclusive<Point>>,
}

/// The grid, locked on the panel's side.
///
/// The lease is taken first and kept as long as the lock is. The reader thread takes it before it
/// reads the pty, so while the panel has the grid the reader waits *before* it reads. Without it
/// the reader can read bytes, find the grid locked, read again, hit the end of the pty because the
/// child has gone, and return with what it read unparsed: the last output of a program that exits
/// while the screen is being copied is lost (a reproduction is in the session tests).
struct Locked<L, G> {
    // Declared first, so the grid is released before the lease.
    term: G,
    _lease: L,
}

impl<L, G: Deref<Target = Term<Listener>>> Deref for Locked<L, G> {
    type Target = Term<Listener>;
    fn deref(&self) -> &Term<Listener> {
        &self.term
    }
}

impl<L, G: DerefMut<Target = Term<Listener>>> DerefMut for Locked<L, G> {
    fn deref_mut(&mut self) -> &mut Term<Listener> {
        &mut self.term
    }
}

/// The engine of one console: the grid, and what is needed to read it and to talk to it.
#[derive(Clone)]
pub struct Backend {
    term: Arc<FairMutex<Term<Listener>>>,
    inbox: Arc<Inbox>,
    /// Feeds bytes that did not come from a pty (a clear, a test).
    parser: Arc<Mutex<Processor<StdSyncHandler>>>,
    search: Arc<Mutex<SearchState>>,
}

impl Backend {
    pub fn new(rows: usize, cols: usize, settings: &Settings) -> Self {
        let inbox = Arc::new(Inbox::default());
        let term = Term::new(
            config(settings),
            &GridSize {
                rows: rows.max(1),
                cols: cols.max(2),
            },
            Listener {
                inbox: inbox.clone(),
            },
        );
        Self {
            term: Arc::new(FairMutex::new(term)),
            inbox,
            parser: Arc::new(Mutex::new(Processor::new())),
            search: Arc::new(Mutex::new(SearchState::default())),
        }
    }

    fn lock(&self) -> Locked<impl Sized + '_, impl DerefMut<Target = Term<Listener>> + '_> {
        let lease = self.term.lease();
        Locked {
            term: self.term.lock_unfair(),
            _lease: lease,
        }
    }

    pub fn inbox(&self) -> &Arc<Inbox> {
        &self.inbox
    }

    /// Apply what a person changed about the grid.
    pub fn set_settings(&self, settings: &Settings) {
        self.lock().set_options(config(settings));
    }

    /// Run bytes through the parser as if the program had written them.
    pub fn feed(&self, bytes: &[u8]) {
        let mut term = self.lock();
        let mut parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        parser.advance(&mut *term, bytes);
    }

    /// What a terminal's reset does to the screen and the history.
    pub fn clear(&self) {
        self.feed(b"\x1b[H\x1b[2J\x1b[3J");
    }

    pub fn resize(&self, rows: usize, cols: usize) {
        self.lock().resize(GridSize {
            rows: rows.max(1),
            cols: cols.max(2),
        });
    }

    pub fn modes(&self) -> Modes {
        modes(*self.lock().mode())
    }

    pub fn set_focused(&self, focused: bool) {
        self.lock().is_focused = focused;
    }

    /// The colour a program changed, by palette index.
    pub fn palette_override(&self, index: usize) -> Option<[u8; 3]> {
        let term = self.lock();
        let colors = term.colors();
        (index < colors_len())
            .then(|| colors[index])
            .flatten()
            .map(|c| [c.r, c.g, c.b])
    }

    pub fn scroll(&self, how: Scrolling) {
        let scroll = match how {
            Scrolling::Lines(n) => Scroll::Delta(n),
            Scrolling::PageUp => Scroll::PageUp,
            Scrolling::PageDown => Scroll::PageDown,
            Scrolling::Top => Scroll::Top,
            Scrolling::Bottom => Scroll::Bottom,
        };
        self.lock().scroll_display(scroll);
    }

    pub fn display_offset(&self) -> usize {
        self.lock().grid().display_offset()
    }

    /// One row of the viewport, for looking at what is under the pointer.
    pub fn row(&self, row: usize) -> Vec<Cell> {
        let term = self.lock();
        let line = Line(row as i32 - term.grid().display_offset() as i32);
        if line < term.topmost_line() || line > term.bottommost_line() {
            return Vec::new();
        }
        let grid_row = &term.grid()[line];
        (0..term.columns())
            .map(|c| convert(&grid_row[Column(c)]))
            .collect()
    }

    pub fn history_size(&self) -> usize {
        self.lock().history_size()
    }

    /// The cursor's row and column in the viewport.
    pub fn cursor_cell(&self) -> (usize, usize) {
        let term = self.lock();
        let p = term.grid().cursor.point;
        let row = (p.line.0 + term.grid().display_offset() as i32).max(0) as usize;
        (row, p.column.0)
    }

    /// Whether the program asked for a blinking cursor.
    pub fn cursor_blinking(&self) -> bool {
        self.lock().cursor_style().blinking
    }

    /// Copy what is on screen.
    pub fn snapshot(&self) -> Content {
        let mut search = self.search.lock().unwrap_or_else(|e| e.into_inner());
        let term = self.lock();
        let rows = term.screen_lines();
        let cols = term.columns();
        let content = term.renderable_content();
        let offset = content.display_offset;

        let mut cells = vec![Cell::default(); rows * cols];
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + offset as i32;
            let col = indexed.point.column.0;
            if row < 0 || row as usize >= rows || col >= cols {
                continue;
            }
            cells[row as usize * cols + col] = convert(indexed.cell);
        }

        let cursor_point = content.cursor.point;
        let cursor_row = cursor_point.line.0 + offset as i32;
        let in_view =
            cursor_row >= 0 && (cursor_row as usize) < rows && cursor_point.column.0 < cols;
        let cursor = Cursor {
            shape: match content.cursor.shape {
                TermCursorShape::Block => CursorShape::Block,
                TermCursorShape::Underline => CursorShape::Underline,
                TermCursorShape::Beam => CursorShape::Beam,
                TermCursorShape::HollowBlock => CursorShape::Hollow,
                TermCursorShape::Hidden => CursorShape::Hidden,
            },
            row: cursor_row.max(0) as usize,
            col: cursor_point.column.0.min(cols.saturating_sub(1)),
            in_view,
            blinking: term.cursor_style().blinking,
        };

        let selection = content.selection.map(|range| SelRange {
            start: (range.start.line.0 + offset as i32, range.start.column.0),
            end: (range.end.line.0 + offset as i32, range.end.column.0),
            block: range.is_block,
        });

        let palette: Vec<Option<[u8; 3]>> = (0..colors_len())
            .map(|i| content.colors[i].map(|c| [c.r, c.g, c.b]))
            .collect();
        let modes = modes(content.mode);
        let history = term.history_size();

        let mut matches = Vec::new();
        if let Some(regex) = search.regex.as_mut() {
            let top = Point::new(Line(-(offset as i32)), Column(0));
            let bottom = Point::new(Line(rows as i32 - 1 - offset as i32), Column(cols - 1));
            for found in RegexIter::new(top, bottom, Direction::Right, &*term, regex).take(2_000) {
                matches.push(MatchRange {
                    start: (found.start().line.0 + offset as i32, found.start().column.0),
                    end: (found.end().line.0 + offset as i32, found.end().column.0),
                });
            }
        }

        Content {
            rows,
            cols,
            cells,
            cursor,
            selection,
            matches,
            display_offset: offset,
            history,
            modes,
            palette,
        }
    }

    // ----- selection -----

    fn point_at(term: &Term<Listener>, row: i32, col: usize) -> Point {
        let offset = term.grid().display_offset() as i32;
        let line = (row - offset).clamp(term.topmost_line().0, term.bottommost_line().0);
        Point::new(Line(line), Column(col.min(term.columns() - 1)))
    }

    pub fn select_start(&self, kind: SelKind, row: i32, col: usize, side: Side) {
        let mut term = self.lock();
        let ty = match kind {
            SelKind::Simple => SelectionType::Simple,
            SelKind::Word => SelectionType::Semantic,
            SelKind::Line => SelectionType::Lines,
            SelKind::Block => SelectionType::Block,
        };
        let point = Self::point_at(&term, row, col);
        term.selection = Some(Selection::new(ty, point, term_side(side)));
        self.forget_current_match();
    }

    pub fn select_update(&self, row: i32, col: usize, side: Side) {
        let mut term = self.lock();
        let point = Self::point_at(&term, row, col);
        if let Some(selection) = term.selection.as_mut() {
            selection.update(point, term_side(side));
        }
    }

    pub fn select_all(&self) {
        let mut term = self.lock();
        let start = Point::new(term.topmost_line(), Column(0));
        let end = Point::new(term.bottommost_line(), Column(term.columns() - 1));
        let mut selection = Selection::new(SelectionType::Simple, start, TermSide::Left);
        selection.update(end, TermSide::Right);
        term.selection = Some(selection);
    }

    pub fn select_clear(&self) {
        self.lock().selection = None;
        self.forget_current_match();
    }

    pub fn has_selection(&self) -> bool {
        self.lock()
            .selection
            .as_ref()
            .is_some_and(|s| !s.is_empty())
    }

    pub fn selected_text(&self) -> Option<String> {
        self.lock().selection_to_string().filter(|s| !s.is_empty())
    }

    /// The whole screen and history as text.
    pub fn all_text(&self) -> String {
        let term = self.lock();
        let start = Point::new(term.topmost_line(), Column(0));
        let end = Point::new(term.bottommost_line(), Column(term.columns() - 1));
        term.bounds_to_string(start, end)
    }

    // ----- search -----

    /// Look for a regular expression; an empty pattern ends the search. Returns what is wrong
    /// with a pattern that does not parse.
    pub fn set_search(&self, pattern: &str) -> Result<(), String> {
        let mut search = self.search.lock().unwrap_or_else(|e| e.into_inner());
        search.current = None;
        search.pattern = pattern.to_owned();
        if pattern.is_empty() {
            search.regex = None;
            return Ok(());
        }
        match RegexSearch::new(pattern) {
            Ok(regex) => {
                search.regex = Some(regex);
                Ok(())
            }
            Err(e) => {
                search.regex = None;
                Err(e.to_string())
            }
        }
    }

    fn forget_current_match(&self) {
        if let Ok(mut search) = self.search.lock() {
            search.current = None;
        }
    }

    /// Select the next (or previous) match and bring it into view. False when there is none.
    pub fn search_step(&self, forward: bool) -> bool {
        let mut search = self.search.lock().unwrap_or_else(|e| e.into_inner());
        let mut term = self.lock();
        let current = search.current.clone();
        let Some(regex) = search.regex.as_mut() else {
            return false;
        };
        let offset = term.grid().display_offset() as i32;
        let rows = term.screen_lines() as i32;
        let origin = match (&current, forward) {
            (Some(m), true) => m.end().add(&*term, Boundary::None, 1),
            (Some(m), false) => m.start().sub(&*term, Boundary::None, 1),
            (None, true) => Point::new(Line(-offset), Column(0)),
            (None, false) => Point::new(Line(rows - 1 - offset), Column(term.columns() - 1)),
        };
        let (direction, side) = if forward {
            (Direction::Right, TermSide::Left)
        } else {
            (Direction::Left, TermSide::Right)
        };
        let Some(found) = term.search_next(regex, origin, direction, side, None) else {
            return false;
        };
        let mut selection = Selection::new(SelectionType::Simple, *found.start(), TermSide::Left);
        selection.update(*found.end(), TermSide::Right);
        term.selection = Some(selection);
        term.scroll_to_point(*found.start());
        search.current = Some(found);
        true
    }

    // ----- the pty -----

    /// Start the program on a pty of `opts.size`, parsed into this grid.
    pub fn spawn(&self, opts: SpawnOptions) -> io::Result<Pty> {
        let mut env: HashMap<String, String> = opts.env.into_iter().collect();
        env.entry("TERM".into())
            .or_insert_with(|| "xterm-256color".into());
        let options = tty::Options {
            shell: opts
                .command
                .map(|(program, args)| tty::Shell::new(program, args)),
            working_directory: opts.cwd,
            drain_on_exit: true,
            env,
            #[cfg(windows)]
            escape_args: true,
        };
        let pty = tty::new(&options, window_size(opts.size), 0)?;
        let event_loop = EventLoop::new(
            self.term.clone(),
            Listener {
                inbox: self.inbox.clone(),
            },
            pty,
            true,
            false,
        )?;
        let sender = event_loop.channel();
        let handle = event_loop.spawn();

        // The loop hands the pty back when it ends, and dropping a pty hangs up on the child and
        // waits for it. That is done here, on a thread of its own, never where the panel runs.
        // It is also how a panic in the engine is noticed: the join fails.
        let inbox = self.inbox.clone();
        std::thread::Builder::new()
            .name("PTY watcher".into())
            .spawn(move || {
                let ended = handle.join();
                inbox.push(Event::Closed {
                    panicked: ended.is_err(),
                });
                drop(ended);
            })?;
        Ok(Pty { sender })
    }
}

fn term_side(side: Side) -> TermSide {
    match side {
        Side::Left => TermSide::Left,
        Side::Right => TermSide::Right,
    }
}

fn colors_len() -> usize {
    269
}

fn modes(mode: TermMode) -> Modes {
    Modes {
        app_cursor: mode.contains(TermMode::APP_CURSOR),
        app_keypad: mode.contains(TermMode::APP_KEYPAD),
        bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
        focus_in_out: mode.contains(TermMode::FOCUS_IN_OUT),
        alt_screen: mode.contains(TermMode::ALT_SCREEN),
        alternate_scroll: mode.contains(TermMode::ALTERNATE_SCROLL),
        mouse: if mode.contains(TermMode::MOUSE_MOTION) {
            MouseMode::Motion
        } else if mode.contains(TermMode::MOUSE_DRAG) {
            MouseMode::Drag
        } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseMode::Click
        } else {
            MouseMode::Off
        },
        mouse_encoding: if mode.contains(TermMode::SGR_MOUSE) {
            MouseEncoding::Sgr
        } else if mode.contains(TermMode::UTF8_MOUSE) {
            MouseEncoding::Utf8
        } else {
            MouseEncoding::Normal
        },
    }
}

fn color(c: TermColor) -> Color {
    match c {
        TermColor::Spec(Rgb { r, g, b }) => Color::Rgb(r, g, b),
        TermColor::Indexed(i) => Color::Indexed(i),
        TermColor::Named(named) => match named {
            NamedColor::Foreground
            | NamedColor::BrightForeground
            | NamedColor::DimForeground
            | NamedColor::Cursor => Color::DefaultFg,
            NamedColor::Background => Color::DefaultBg,
            other => {
                let n = other as usize;
                if n < 16 {
                    Color::Indexed(n as u8)
                } else {
                    // The dim variants follow the cursor's slot, in the order of the eight colours.
                    Color::Indexed(n.saturating_sub(NamedColor::DimBlack as usize).min(7) as u8)
                }
            }
        },
    }
}

fn attrs(flags: Flags) -> Attrs {
    const MAP: [(Flags, u16); 13] = [
        (Flags::BOLD, Attrs::BOLD),
        (Flags::ITALIC, Attrs::ITALIC),
        (Flags::INVERSE, Attrs::INVERSE),
        (Flags::DIM, Attrs::DIM),
        (Flags::HIDDEN, Attrs::HIDDEN),
        (Flags::STRIKEOUT, Attrs::STRIKE),
        (Flags::WIDE_CHAR, Attrs::WIDE),
        (Flags::WIDE_CHAR_SPACER, Attrs::SPACER),
        (Flags::UNDERLINE, Attrs::UNDERLINE),
        (Flags::DOUBLE_UNDERLINE, Attrs::DOUBLE_UNDERLINE),
        (Flags::UNDERCURL, Attrs::CURL),
        (Flags::DOTTED_UNDERLINE, Attrs::DOTTED),
        (Flags::DASHED_UNDERLINE, Attrs::DASHED),
    ];
    let mut bits = 0;
    for (flag, bit) in MAP {
        if flags.contains(flag) {
            bits |= bit;
        }
    }
    // The blank that stands where a wide character did not fit at the end of a row.
    if flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
        bits |= Attrs::SPACER;
    }
    Attrs(bits)
}

fn convert(cell: &TermCell) -> Cell {
    let zerowidth = cell.zerowidth().filter(|z| !z.is_empty());
    let link = cell.hyperlink();
    let underline_color = cell.underline_color();
    let extra = if zerowidth.is_some() || link.is_some() || underline_color.is_some() {
        Some(Box::new(Extra {
            zerowidth: zerowidth.map(<[char]>::to_vec).unwrap_or_default(),
            underline_color: underline_color.map(color),
            link: link.map(|l| Arc::<str>::from(l.uri())),
        }))
    } else {
        None
    };
    Cell {
        c: cell.c,
        fg: color(cell.fg),
        bg: color(cell.bg),
        attrs: attrs(cell.flags),
        extra,
    }
}

/// What to run, where, and with which variables added to the inherited ones.
pub struct SpawnOptions {
    /// `None` is the person's shell; otherwise a program and its arguments.
    pub command: Option<(String, Vec<String>)>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub size: TermSize,
}

/// The way to talk to a running session. Cheap to clone; every call is a message to the loop's
/// thread and none of them waits for it.
#[derive(Clone)]
pub struct Pty {
    sender: EventLoopSender,
}

impl Pty {
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        if !bytes.is_empty() {
            let _ = self.sender.send(Msg::Input(bytes));
        }
    }

    /// Tell the program its window changed (the ioctl and the signal). Only for a live session:
    /// the crate ends the whole process if the ioctl fails.
    pub fn resize(&self, size: TermSize) {
        let _ = self.sender.send(Msg::Resize(window_size(size)));
    }

    /// End the loop; the child is hung up on from the thread that owns the pty.
    pub fn shutdown(&self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(rows: usize, cols: usize) -> Backend {
        Backend::new(rows, cols, &Settings::default())
    }

    fn text(content: &Content, row: usize) -> String {
        content.row_text(row).trim_end().to_owned()
    }

    #[test]
    fn colours_are_the_programs_own() {
        let b = term(3, 20);
        // red, bright blue, 256-colour 200, truecolour, and back to default.
        b.feed(b"\x1b[31ma\x1b[94mb\x1b[38;5;200mc\x1b[38;2;1;2;3md\x1b[0me");
        let c = b.snapshot();
        assert_eq!(c.cell(0, 0).fg, Color::Indexed(1));
        assert_eq!(c.cell(0, 1).fg, Color::Indexed(12));
        assert_eq!(c.cell(0, 2).fg, Color::Indexed(200));
        assert_eq!(c.cell(0, 3).fg, Color::Rgb(1, 2, 3));
        assert_eq!(c.cell(0, 4).fg, Color::DefaultFg);
        b.feed(b"\x1b[0;41;1mX");
        let c = b.snapshot();
        assert_eq!(c.cell(0, 5).bg, Color::Indexed(1));
        assert!(c.cell(0, 5).attrs.has(Attrs::BOLD));
    }

    #[test]
    fn the_cursor_can_be_addressed() {
        let b = term(5, 20);
        b.feed(b"\x1b[3;7Hx");
        let c = b.snapshot();
        assert_eq!(c.cell(2, 6).c, 'x');
        assert_eq!((c.cursor.row, c.cursor.col), (2, 7));
        assert!(c.cursor.in_view);
    }

    #[test]
    fn the_alternate_screen_comes_and_goes_with_the_primary_kept() {
        let b = term(4, 20);
        b.feed(b"primary");
        b.feed(b"\x1b[?1049h");
        assert!(b.modes().alt_screen);
        b.feed(b"\x1b[2J\x1b[Halt");
        let c = b.snapshot();
        assert_eq!(text(&c, 0), "alt");
        b.feed(b"\x1b[?1049l");
        assert!(!b.modes().alt_screen);
        assert_eq!(text(&b.snapshot(), 0), "primary");
    }

    #[test]
    fn a_scroll_region_scrolls_only_inside_itself() {
        let b = term(5, 10);
        b.feed(b"\x1b[2J\x1b[H");
        for row in 1..=5 {
            b.feed(format!("\x1b[{row};1Hr{row}").as_bytes());
        }
        // Region rows 2 to 4; a line feed at the bottom of it scrolls them, not row 1 or 5.
        b.feed(b"\x1b[2;4r\x1b[4;1H\n");
        let c = b.snapshot();
        assert_eq!(text(&c, 0), "r1");
        assert_eq!(text(&c, 1), "r3");
        assert_eq!(text(&c, 2), "r4");
        assert_eq!(text(&c, 3), "");
        assert_eq!(text(&c, 4), "r5");
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let b = term(2, 10);
        b.feed("a日b".as_bytes());
        let c = b.snapshot();
        assert_eq!(c.cell(0, 1).c, '日');
        assert!(c.cell(0, 1).attrs.has(Attrs::WIDE));
        assert!(c.cell(0, 2).attrs.has(Attrs::SPACER));
        assert_eq!(c.cell(0, 3).c, 'b');
        assert_eq!(c.row_text(0).trim_end(), "a日b");
    }

    #[test]
    fn a_carriage_return_rewrites_the_line_like_a_progress_bar() {
        let b = term(2, 20);
        b.feed(b"10%\r50%\r100%");
        assert_eq!(text(&b.snapshot(), 0), "100%");
        b.feed(b"\r\x1b[Kdone");
        assert_eq!(text(&b.snapshot(), 0), "done");
    }

    #[test]
    fn a_title_is_reported_and_reset() {
        let b = term(2, 20);
        b.feed(b"\x1b]2;vim\x07");
        b.feed(b"\x1b]2;\x07");
        let events = b.inbox().take(16);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Title(Some(t)) if t == "vim"))
        );
    }

    #[test]
    fn a_program_may_set_the_clipboard_but_not_read_it() {
        let b = term(2, 20);
        // "hello" in base64.
        b.feed(b"\x1b]52;c;aGVsbG8=\x07");
        b.feed(b"\x1b]52;c;?\x07");
        let events = b.inbox().take(16);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::ClipboardStore(t) if t == "hello"))
        );
        assert!(
            !events.iter().any(|e| matches!(e, Event::PtyWrite(_))),
            "a query for the clipboard is not answered: {events:?}"
        );
    }

    #[test]
    fn queries_are_answered_in_the_order_they_were_asked() {
        let b = term(5, 20);
        b.feed(b"\x1b[2;6H\x1b[6n\x1b[c");
        let writes: Vec<String> = b
            .inbox()
            .take(16)
            .into_iter()
            .filter_map(|e| match e {
                Event::PtyWrite(s) => Some(s),
                _ => None,
            })
            .collect();
        assert_eq!(writes.len(), 2, "{writes:?}");
        assert_eq!(writes[0], "\x1b[2;6R");
        assert!(writes[1].starts_with("\x1b[?"), "{writes:?}");
    }

    #[test]
    fn the_modes_programs_set_are_visible() {
        let b = term(2, 10);
        b.feed(b"\x1b[?1h\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[?1004h");
        let m = b.modes();
        assert!(m.app_cursor && m.bracketed_paste && m.focus_in_out);
        assert_eq!(m.mouse, MouseMode::Click);
        assert_eq!(m.mouse_encoding, MouseEncoding::Sgr);
        b.feed(b"\x1b[?1003h");
        assert_eq!(b.modes().mouse, MouseMode::Motion);
    }

    #[test]
    fn history_is_scrolled_and_a_selection_is_copied() {
        let b = term(3, 10);
        for i in 0..10 {
            b.feed(format!("line{i}\r\n").as_bytes());
        }
        let c = b.snapshot();
        assert!(c.history >= 8);
        b.scroll(Scrolling::Lines(2));
        let c = b.snapshot();
        assert_eq!(c.display_offset, 2);
        b.scroll(Scrolling::Bottom);
        assert_eq!(b.display_offset(), 0);

        b.select_start(SelKind::Simple, 0, 0, Side::Left);
        b.select_update(0, 4, Side::Right);
        let copied = b.selected_text().unwrap();
        assert!(copied.starts_with("line"), "{copied:?}");
        b.select_clear();
        assert!(b.selected_text().is_none());
    }

    #[test]
    fn search_finds_every_visible_match_and_steps_through_them() {
        let b = term(4, 20);
        b.feed(b"foo bar\r\nbar foo\r\nfoo");
        b.set_search("foo").unwrap();
        let c = b.snapshot();
        assert_eq!(c.matches.len(), 3);
        assert!(b.search_step(true));
        assert!(b.search_step(true));
        assert!(b.set_search("(").is_err());
    }

    #[test]
    fn resizing_keeps_the_content() {
        let b = term(4, 20);
        b.feed(b"hello");
        b.resize(6, 12);
        let c = b.snapshot();
        assert_eq!((c.rows, c.cols), (6, 12));
        assert_eq!(text(&c, 0), "hello");
    }

    #[test]
    fn clear_empties_the_screen_and_the_history() {
        let b = term(3, 10);
        for i in 0..10 {
            b.feed(format!("l{i}\r\n").as_bytes());
        }
        b.clear();
        let c = b.snapshot();
        assert_eq!(c.history, 0);
        assert!((0..c.rows).all(|r| text(&c, r).is_empty()));
    }

    // ----- what the published crate does that the regression tests of the handover ask about -----

    /// 1. Kitty keyboard mode stack: with the mode off, as it is here, thousands of pushes are
    ///    ignored and nothing panics.
    #[test]
    fn thousands_of_keyboard_mode_pushes_do_not_panic() {
        let b = term(2, 10);
        for _ in 0..4_100 {
            b.feed(b"\x1b[>1u");
        }
        b.feed(b"ok");
        assert_eq!(text(&b.snapshot(), 0), "ok");
    }

    /// 2. An emoji presentation selector after a character that is narrow by default.
    #[test]
    #[ignore = "alacritty_terminal 0.26.0 keeps base + U+FE0F in one cell; it is drawn two wide (docs/adr/0068)"]
    fn emoji_presentation_takes_two_columns() {
        let b = term(2, 10);
        b.feed("\u{2764}\u{fe0f}x".as_bytes());
        let c = b.snapshot();
        let x_at = (0..c.cols).find(|&i| c.cell(0, i).c == 'x').unwrap();
        assert_eq!(
            x_at,
            2,
            "the heart takes a cell and a spacer: {:?}",
            text(&c, 0)
        );
    }

    /// 3. A long run of combining marks must not grow without bound.
    #[test]
    #[ignore = "alacritty_terminal 0.26.0 appends every combining mark to the cell's list (docs/adr/0068)"]
    fn combining_marks_are_bounded() {
        let b = term(2, 10);
        let mut s = String::from("a");
        s.extend(std::iter::repeat_n('\u{0301}', 200_000));
        b.feed(s.as_bytes());
        let c = b.snapshot();
        let n = c.cell(0, 0).extra.as_ref().map_or(0, |e| e.zerowidth.len());
        assert!(n < 1_000, "one cell holds {n} combining marks");
    }
}
