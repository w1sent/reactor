//! The terminal as a gpui view: the focus, the session's life, what the engine says, and what the
//! person does with the keyboard, the mouse, the clipboard and the find bar.
//!
//! The painting is `element.rs`; the bytes for a key are `keys.rs`, for the mouse `mouse.rs`.

use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::backend::Backend;
use super::element::{TerminalElement, palette_from_theme};
use super::keys;
use super::links::{Link, link_at};
use super::metrics::Metrics;
use super::mouse::{self, Action as MouseAction, Button as MouseBtn, Mods, Wheel};
use super::session::{Kind, Session};
use super::types::*;
use super::{
    CloseFind, CopySelection, FindNext, FindPrevious, OpenFind, PasteClipboard, SelectAllText,
};

/// How long events are gathered after the first one before the screen is redrawn. A keystroke's
/// echo is drawn at once (the first event is not delayed); this bounds how often a flood redraws.
const COALESCE: Duration = Duration::from_millis(4);
/// Most events taken in one go, so a flood cannot hold the thread.
const MAX_BATCH: usize = 256;
const BLINK: Duration = Duration::from_millis(530);
const AUTOSCROLL: Duration = Duration::from_millis(50);
/// Lines for one notch of a wheel that counts in lines.
const WHEEL_LINES: f32 = 3.0;
/// How soon after the last refresh a lost focus asks for another.
const IDLE_REFRESH: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blink {
    Off,
    On,
    /// As the program asks (DECSCUSR).
    Terminal,
}

/// What the app decides about how a terminal looks and behaves.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub family: SharedString,
    pub size: Pixels,
    pub min_contrast: f32,
    pub cursor: CursorShape,
    pub blink: Blink,
    pub option_as_alt: bool,
    /// The shell to run instead of `$SHELL`.
    pub shell: Option<String>,
    pub scrollback: usize,
    pub copy_on_select: bool,
}

impl Look {
    pub fn settings(&self) -> Settings {
        Settings {
            scrollback: self.scrollback,
            cursor_shape: self.cursor,
            cursor_blink: false,
        }
    }
}

/// What a terminal tells its panel.
#[derive(Debug, Clone)]
pub enum TerminalEvent {
    /// The program set (or reset) its title.
    Title,
    Bell,
    /// The program ended.
    Finished(Option<ExitInfo>),
    /// The terminal lost the focus after printing something, at most once in a while.
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    Select,
    Scrollbar,
}

struct FindBar {
    input: Entity<InputState>,
    error: Option<String>,
    _subscription: Subscription,
}

pub struct TerminalView {
    pub(crate) focus: FocusHandle,
    pub(crate) backend: Backend,
    pub(super) session: Option<Session>,
    kind: Kind,
    cwd: Option<PathBuf>,
    pub(crate) look: Look,
    pub(crate) metrics: Metrics,
    /// The grid last given to the engine.
    pub(super) size: TermSize,
    title: Option<String>,
    exit: Option<ExitInfo>,
    ended: bool,
    error: Option<String>,
    engine_stopped: bool,

    // input
    marked: Option<String>,
    wheel_carry: f32,
    pressed: Option<MouseBtn>,
    drag: Option<Drag>,
    last_cell: Option<(usize, i32)>,
    pointer_col: usize,
    autoscroll_dir: i32,
    pub(crate) hover: Option<(usize, Link)>,
    pub(crate) link_modifier: bool,
    pub(crate) focused: bool,

    // cursor
    pub(crate) blink_on: bool,
    term_blinks: bool,

    // find
    find: Option<FindBar>,

    // idle refresh
    output_since_focus: bool,
    last_idle: Instant,

    _events: Task<()>,
    _blink: Option<Task<()>>,
    _autoscroll: Option<Task<()>>,
    _focus_in: Subscription,
    _focus_out: Subscription,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn mouse_button(b: MouseButton) -> Option<MouseBtn> {
    match b {
        MouseButton::Left => Some(MouseBtn::Left),
        MouseButton::Middle => Some(MouseBtn::Middle),
        MouseButton::Right => Some(MouseBtn::Right),
        _ => None,
    }
}

fn mods_of(m: Modifiers) -> Mods {
    Mods {
        shift: m.shift,
        alt: m.alt,
        ctrl: m.control,
    }
}

/// The key that turns a hovered link into one that follows a click.
fn link_modifier_held(m: Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        m.platform
    } else {
        m.control
    }
}

impl TerminalView {
    pub fn new(
        kind: Kind,
        cwd: Option<PathBuf>,
        look: Look,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let focus_in = cx.on_focus_in(&focus, window, |this, _window, cx| {
            this.focus_changed(true, cx)
        });
        let focus_out = cx.on_focus_out(&focus, window, |this, _event, _window, cx| {
            this.focus_changed(false, cx)
        });
        let backend = Backend::new(24, 80, &look.settings());
        let mut this = Self {
            focus,
            backend,
            session: None,
            kind,
            cwd,
            look,
            metrics: Metrics::default(),
            size: TermSize::new(24, 80, 8, 16),
            title: None,
            exit: None,
            ended: false,
            error: None,
            engine_stopped: false,
            marked: None,
            wheel_carry: 0.0,
            pressed: None,
            drag: None,
            last_cell: None,
            pointer_col: 0,
            autoscroll_dir: 0,
            hover: None,
            link_modifier: false,
            focused: false,
            blink_on: true,
            term_blinks: false,
            find: None,
            output_since_focus: false,
            last_idle: Instant::now(),
            _events: Task::ready(()),
            _blink: None,
            _autoscroll: None,
            _focus_in: focus_in,
            _focus_out: focus_out,
        };
        this.watch(cx);
        this
    }

    // ----- state the panel shows -----

    pub fn kind(&self) -> &Kind {
        &self.kind
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The program is running.
    pub fn running(&self) -> bool {
        self.session.as_ref().is_some_and(Session::alive)
    }

    pub fn exit(&self) -> Option<ExitInfo> {
        self.exit
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn engine_stopped(&self) -> bool {
        self.engine_stopped
    }

    pub fn focused(&self) -> bool {
        self.focused
    }

    pub fn set_look(&mut self, look: Look, cx: &mut Context<Self>) {
        if self.look == look {
            return;
        }
        let settings_changed = self.look.settings() != look.settings();
        self.look = look;
        if settings_changed {
            self.backend.set_settings(&self.look.settings());
        }
        self.restart_blink(cx);
        cx.notify();
    }

    // ----- the session -----

    fn watch(&mut self, cx: &mut Context<Self>) {
        let inbox = self.backend.inbox().clone();
        self._events = cx.spawn(async move |this, cx| {
            loop {
                inbox.ready().await;
                if this.update(cx, |this, cx| this.drain(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(COALESCE).await;
            }
        });
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        self.backend
            .resize(self.size.rows as usize, self.size.cols as usize);
        match Session::start(
            &self.backend,
            &self.kind,
            self.cwd.clone(),
            self.size,
            self.look.shell.as_deref(),
        ) {
            Ok(session) => {
                self.session = Some(session);
                self.error = None;
                self.exit = None;
                self.ended = false;
            }
            Err(e) => self.error = Some(format!("could not run: {e}")),
        }
        cx.notify();
    }

    /// Run the program again in the same pane: the old output goes, as in a terminal's reset.
    pub fn rerun(&mut self, cx: &mut Context<Self>) {
        self.session = None;
        self.backend.clear();
        self.backend.select_clear();
        self.exit = None;
        self.title = None;
        self.start(cx);
    }

    /// After the engine's thread stopped: a fresh grid and a fresh session.
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        self.session = None;
        self.backend = Backend::new(
            self.size.rows as usize,
            self.size.cols as usize,
            &self.look.settings(),
        );
        self.engine_stopped = false;
        self.watch(cx);
        self.exit = None;
        self.title = None;
        self.start(cx);
    }

    /// Called from the element when it knows how big the grid can be.
    pub(crate) fn layout(&mut self, m: Metrics, cx: &mut Context<Self>) {
        self.metrics = m;
        let size = TermSize::new(
            m.rows as u16,
            m.cols as u16,
            m.cell_w.round() as u16,
            m.line_h.round() as u16,
        );
        let resized = size != self.size;
        self.size = size;
        match self.session.as_mut() {
            None if self.error.is_none() && !self.engine_stopped => self.start(cx),
            None => {
                if resized {
                    self.backend.resize(size.rows as usize, size.cols as usize);
                }
            }
            Some(session) => {
                if session.resize(size) || resized {
                    self.backend.resize(size.rows as usize, size.cols as usize);
                }
            }
        }
    }

    /// Everything the engine has said, in the order it said it.
    fn drain(&mut self, cx: &mut Context<Self>) {
        let mut repaint = false;
        for event in self.backend.inbox().take(MAX_BATCH) {
            match event {
                Event::Wakeup => {
                    repaint = true;
                    self.output_since_focus = true;
                }
                Event::Bell => cx.emit(TerminalEvent::Bell),
                Event::Title(title) => {
                    self.title = title;
                    cx.emit(TerminalEvent::Title);
                    repaint = true;
                }
                Event::PtyWrite(text) => self.write(text.into_bytes()),
                Event::ColorRequest { index, reply } => {
                    let rgb = self.colour_of(index, cx);
                    self.write(reply(rgb).into_bytes());
                }
                Event::SizeRequest(reply) => {
                    let size = self.size;
                    self.write(reply(size).into_bytes());
                }
                Event::ClipboardStore(text) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text))
                }
                Event::CursorBlink => {
                    self.term_blinks = self.backend.cursor_blinking();
                    self.restart_blink(cx);
                }
                Event::ChildExit(info) => {
                    self.exit = Some(info);
                    if let Some(session) = self.session.as_mut() {
                        session.ended();
                    }
                    cx.emit(TerminalEvent::Finished(Some(info)));
                    repaint = true;
                }
                Event::Exit => {
                    self.ended = true;
                    if let Some(session) = self.session.as_mut() {
                        session.ended();
                    }
                    repaint = true;
                }
                Event::Closed { panicked } => {
                    if panicked {
                        self.engine_stopped = true;
                        cx.emit(TerminalEvent::Finished(None));
                    }
                    repaint = true;
                }
            }
        }
        if repaint {
            cx.notify();
        }
    }

    fn colour_of(&self, index: usize, cx: &App) -> [u8; 3] {
        if let Some(rgb) = self.backend.palette_override(index) {
            return rgb;
        }
        let palette = palette_from_theme(cx.theme(), 1.0);
        match index {
            PALETTE_FG => palette.fg,
            PALETTE_BG => palette.bg,
            PALETTE_CURSOR => palette.cursor,
            i if i < 256 => palette.resolve(Color::Indexed(i as u8), &|_| None),
            _ => palette.fg,
        }
    }

    fn write(&self, bytes: impl Into<std::borrow::Cow<'static, [u8]>>) {
        if let Some(session) = &self.session {
            session.write(bytes);
        }
    }

    /// End the program that did not stop for Ctrl-C: hang up on it, as closing the tab does.
    pub fn hang_up(&mut self, cx: &mut Context<Self>) {
        if self.running() {
            self.session = None;
            self.exit = Some(ExitInfo {
                code: None,
                signal: Some(1),
            });
            cx.emit(TerminalEvent::Finished(self.exit));
            cx.notify();
        }
    }

    /// Open the find bar and put the cursor in it.
    pub fn show_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(&OpenFind, window, cx);
    }

    /// The whole screen and history as text.
    pub fn all_text(&self) -> String {
        self.backend.all_text()
    }

    /// Stop the program the way Ctrl-C does.
    pub fn interrupt(&self) {
        if let Some(session) = &self.session {
            session.interrupt();
        }
    }

    /// What the person typed or pasted, sent on; the view goes back to the live end.
    fn send_input(
        &mut self,
        bytes: impl Into<std::borrow::Cow<'static, [u8]>>,
        cx: &mut Context<Self>,
    ) {
        if self.backend.display_offset() > 0 {
            self.backend.scroll(Scrolling::Bottom);
        }
        if self.backend.has_selection() {
            self.backend.select_clear();
        }
        self.write(bytes);
        self.restart_blink(cx);
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.backend.clear();
        self.backend.select_clear();
        // A shell redraws its prompt on a form feed.
        self.write(&b"\x0c"[..]);
        cx.notify();
    }

    pub fn copy_all(&self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.backend.all_text()));
    }

    fn focus_changed(&mut self, gained: bool, cx: &mut Context<Self>) {
        self.focused = gained;
        self.backend.set_focused(gained);
        if self.backend.modes().focus_in_out {
            self.write(keys::focus(gained));
        }
        if gained {
            self.output_since_focus = false;
        } else if self.output_since_focus && self.last_idle.elapsed() >= IDLE_REFRESH {
            self.output_since_focus = false;
            self.last_idle = Instant::now();
            cx.emit(TerminalEvent::Idle);
        }
        self.restart_blink(cx);
        cx.notify();
    }

    // ----- the cursor -----

    fn blink_wanted(&self) -> bool {
        self.focused
            && match self.look.blink {
                Blink::Off => false,
                Blink::On => true,
                Blink::Terminal => self.term_blinks,
            }
    }

    /// Show the cursor and, if it should blink, start the clock again. Typing does this, so the
    /// cursor does not disappear under the hand.
    fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.blink_on = true;
        self._blink = None;
        if !self.blink_wanted() {
            return;
        }
        self._blink = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK).await;
                let go = this.update(cx, |this, cx| {
                    if !this.blink_wanted() {
                        this.blink_on = true;
                        cx.notify();
                        return false;
                    }
                    this.blink_on = !this.blink_on;
                    cx.notify();
                    true
                });
                if !matches!(go, Ok(true)) {
                    break;
                }
            }
        }));
    }

    // ----- the keyboard -----

    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Keys typed in the find bar are the bar's.
        if !self.focus.is_focused(window) {
            return;
        }
        let ks = &ev.keystroke;
        // Text from an input method arrives through the input handler, not as keys.
        if (ev.prefer_character_input && ks.key_char.is_some()) || self.marked.is_some() {
            return;
        }
        let modes = self.backend.modes();
        let m = ks.modifiers;
        if m.shift && !m.control && !m.alt && !m.platform && !modes.alt_screen {
            let how = match ks.key.as_str() {
                "pageup" => Some(Scrolling::PageUp),
                "pagedown" => Some(Scrolling::PageDown),
                "home" => Some(Scrolling::Top),
                "end" => Some(Scrolling::Bottom),
                _ => None,
            };
            if let Some(how) = how {
                self.backend.scroll(how);
                cx.notify();
                cx.stop_propagation();
                return;
            }
        }
        if let Some(bytes) = keys::encode(ks, modes, keys::alt_is_meta(self.look.option_as_alt)) {
            self.send_input(bytes.into_owned(), cx);
            cx.stop_propagation();
        }
    }

    fn modifiers_changed(&mut self, m: Modifiers, cx: &mut Context<Self>) {
        let held = link_modifier_held(m);
        if held != self.link_modifier {
            self.link_modifier = held;
            cx.notify();
        }
    }

    // ----- the clipboard -----

    fn copy(&mut self, _: &CopySelection, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.backend.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        let bytes = keys::paste(text, self.backend.modes().bracketed_paste);
        self.send_input(bytes, cx);
    }

    fn paste(&mut self, _: &PasteClipboard, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.paste_text(&text, cx);
        }
    }

    fn select_all(&mut self, _: &SelectAllText, _window: &mut Window, cx: &mut Context<Self>) {
        self.backend.select_all();
        cx.notify();
    }

    // ----- the mouse -----

    fn report(
        &mut self,
        button: MouseBtn,
        action: MouseAction,
        m: Modifiers,
        col: usize,
        row: i32,
    ) {
        let modes = self.backend.modes();
        let row = row.clamp(0, self.metrics.rows as i32 - 1) as usize;
        if let Some(bytes) = mouse::report(modes, button, action, mods_of(m), col, row) {
            self.write(bytes);
        }
    }

    pub(crate) fn mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window, cx);
        let (px, py) = (ev.position.x.as_f32(), ev.position.y.as_f32());
        let m = self.metrics;
        let modes = self.backend.modes();

        if ev.button == MouseButton::Left && m.in_scrollbar(px, py) && !modes.alt_screen {
            self.drag = Some(Drag::Scrollbar);
            self.scrollbar_to(py);
            cx.notify();
            return;
        }
        let (row, col, side, _) = m.cell_at(px, py);
        let row = row.clamp(0, m.rows as i32 - 1);
        self.pointer_col = col;

        if mouse::reporting(modes, ev.modifiers.shift) {
            if let Some(button) = mouse_button(ev.button) {
                self.pressed = Some(button);
                self.last_cell = Some((col, row));
                self.report(button, MouseAction::Press, ev.modifiers, col, row);
            }
            return;
        }

        match ev.button {
            MouseButton::Left => {
                if link_modifier_held(ev.modifiers)
                    && let Some(link) = self.link_at(row, col)
                {
                    cx.open_url(&link.url);
                    return;
                }
                let kind = match ev.click_count {
                    0 | 1 if ev.modifiers.alt => SelKind::Block,
                    0 | 1 => SelKind::Simple,
                    2 => SelKind::Word,
                    _ => SelKind::Line,
                };
                self.backend.select_start(kind, row, col, side);
                self.drag = Some(Drag::Select);
                cx.notify();
            }
            MouseButton::Middle => {
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                if let Some(text) = cx.read_from_primary().and_then(|item| item.text()) {
                    self.paste_text(&text, cx);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn mouse_move(
        &mut self,
        ev: &MouseMoveEvent,
        hovered: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (px, py) = (ev.position.x.as_f32(), ev.position.y.as_f32());
        let m = self.metrics;
        let (row, col, side, outside) = m.cell_at(px, py);
        self.pointer_col = col;

        match self.drag {
            Some(Drag::Select) => {
                self.autoscroll_dir = outside;
                if outside != 0 {
                    self.start_autoscroll(cx);
                }
                let row = if outside < 0 {
                    0
                } else if outside > 0 {
                    m.rows as i32 - 1
                } else {
                    row
                };
                self.backend.select_update(row, col, side);
                cx.notify();
                return;
            }
            Some(Drag::Scrollbar) => {
                self.scrollbar_to(py);
                cx.notify();
                return;
            }
            None => {}
        }
        if !hovered {
            return;
        }
        let row = row.clamp(0, m.rows as i32 - 1);
        let modes = self.backend.modes();
        if mouse::reporting(modes, ev.modifiers.shift) {
            if self.last_cell != Some((col, row)) {
                self.last_cell = Some((col, row));
                let button = self.pressed.unwrap_or(MouseBtn::None);
                self.report(button, MouseAction::Motion, ev.modifiers, col, row);
            }
            return;
        }
        self.link_modifier = link_modifier_held(ev.modifiers);
        let hover = self.link_at(row, col).map(|l| (row as usize, l));
        if hover != self.hover {
            self.hover = hover;
            cx.notify();
        }
    }

    pub(crate) fn mouse_up(&mut self, ev: &MouseUpEvent, cx: &mut Context<Self>) {
        if let Some(button) = self.pressed.take() {
            let m = self.metrics;
            let (row, col, _, _) = m.cell_at(ev.position.x.as_f32(), ev.position.y.as_f32());
            self.report(button, MouseAction::Release, ev.modifiers, col, row);
        }
        match self.drag.take() {
            Some(Drag::Select) => {
                self._autoscroll = None;
                self.autoscroll_dir = 0;
                match self.backend.selected_text() {
                    Some(text) => {
                        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                        cx.write_to_primary(ClipboardItem::new_string(text.clone()));
                        if self.look.copy_on_select {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                        }
                    }
                    None => self.backend.select_clear(),
                }
                cx.notify();
            }
            Some(Drag::Scrollbar) | None => {}
        }
    }

    pub(crate) fn scroll_wheel(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let modes = self.backend.modes();
        let lines = match ev.delta {
            ScrollDelta::Pixels(p) => p.y.as_f32() / self.metrics.line_h,
            ScrollDelta::Lines(l) => l.y * WHEEL_LINES,
        };
        // Positive is towards the history.
        let n = mouse::take_lines(&mut self.wheel_carry, lines);
        if n == 0 {
            return;
        }
        match mouse::wheel(modes, ev.modifiers.shift) {
            Wheel::Report => {
                let (row, col, _, _) = self
                    .metrics
                    .cell_at(ev.position.x.as_f32(), ev.position.y.as_f32());
                let button = if n > 0 {
                    MouseBtn::WheelUp
                } else {
                    MouseBtn::WheelDown
                };
                for _ in 0..n.unsigned_abs().min(10) {
                    self.report(button, MouseAction::Press, ev.modifiers, col, row);
                }
            }
            Wheel::Arrows => self.write(mouse::wheel_arrows(modes, n)),
            Wheel::Scrollback => {
                self.backend.scroll(Scrolling::Lines(n));
                cx.notify();
            }
        }
        cx.stop_propagation();
    }

    fn scrollbar_to(&mut self, py: f32) {
        let content_history = self.backend.history_size();
        let want = self.metrics.offset_at(py, content_history) as i32;
        let now = self.backend.display_offset() as i32;
        if want != now {
            self.backend.scroll(Scrolling::Lines(want - now));
        }
    }

    fn start_autoscroll(&mut self, cx: &mut Context<Self>) {
        if self._autoscroll.is_some() {
            return;
        }
        self._autoscroll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AUTOSCROLL).await;
                let go = this.update(cx, |this, cx| this.autoscroll_step(cx));
                if !matches!(go, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn autoscroll_step(&mut self, cx: &mut Context<Self>) -> bool {
        if self.drag != Some(Drag::Select) || self.autoscroll_dir == 0 {
            self._autoscroll = None;
            return false;
        }
        let up = self.autoscroll_dir < 0;
        self.backend
            .scroll(Scrolling::Lines(if up { 2 } else { -2 }));
        let row = if up { 0 } else { self.metrics.rows as i32 - 1 };
        self.backend.select_update(
            row,
            self.pointer_col,
            if up { Side::Left } else { Side::Right },
        );
        cx.notify();
        true
    }

    fn link_at(&self, row: i32, col: usize) -> Option<Link> {
        if row < 0 {
            return None;
        }
        link_at(&self.backend.row(row as usize), col)
    }

    // ----- find -----

    fn open_find(&mut self, _: &OpenFind, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(bar) = &self.find {
            bar.input.update(cx, |input, cx| input.focus(window, cx));
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find"));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let text = input.read(cx).value().to_string();
                    let error = this.backend.set_search(&text).err();
                    if let Some(bar) = this.find.as_mut() {
                        bar.error = error;
                    }
                    if !text.is_empty() {
                        this.backend.search_step(true);
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.find_step(true, window, cx),
                _ => {}
            },
        );
        input.update(cx, |input, cx| input.focus(window, cx));
        self.find = Some(FindBar {
            input,
            error: None,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn find_step(&mut self, forward: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.backend.search_step(forward);
        cx.notify();
    }

    fn close_find(&mut self, _: &CloseFind, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.take().is_some() {
            let _ = self.backend.set_search("");
            self.focus.focus(window, cx);
            cx.notify();
        }
    }
}

// ----- input methods: composed text, dead keys -----

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| 0..m.encode_utf16().count())
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.take().is_some() {
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = None;
        if !text.is_empty() {
            // Typed characters, not a paste: no wrapper.
            self.send_input(text.as_bytes().to_vec(), cx);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!new_text.is_empty()).then(|| new_text.to_owned());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let m = self.metrics;
        let (row, col) = self.backend.cursor_cell();
        let origin = point(
            px(m.origin_x + col as f32 * m.cell_w),
            px(m.origin_y + row as f32 * m.line_h),
        );
        let _ = element_bounds;
        Some(Bounds::new(
            origin,
            gpui_kit::size(px(m.cell_w), px(m.line_h)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl TerminalView {
    /// The text an input method is composing, drawn at the cursor.
    pub(crate) fn marked_text(&self) -> Option<&str> {
        self.marked.as_deref()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let view = cx.entity();

        let mut root = div()
            .id("terminal")
            .size_full()
            .relative()
            .overflow_hidden()
            .key_context("Terminal")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_modifiers_changed(
                cx.listener(|this, ev: &ModifiersChangedEvent, _window, cx| {
                    this.modifiers_changed(ev.modifiers, cx)
                }),
            )
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::open_find))
            .on_action(cx.listener(Self::close_find))
            .on_action(
                cx.listener(|this, _: &FindNext, window, cx| this.find_step(true, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &FindPrevious, window, cx| this.find_step(false, window, cx)),
            )
            .child(TerminalElement::new(view));

        if let Some(error) = self.error.clone() {
            root = root.child(
                v_flex()
                    .absolute()
                    .inset_0()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(div().text_color(theme.danger).child(error))
                    .child(
                        Button::new("terminal-retry")
                            .small()
                            .label("Retry")
                            .on_click(cx.listener(|this, _, _window, cx| this.rerun(cx))),
                    ),
            );
        }

        if let Some(bar) = &self.find {
            let invalid = bar.error.is_some();
            root =
                root.child(
                    h_flex()
                        .id("terminal-find")
                        .occlude()
                        .absolute()
                        .top_1()
                        .right_4()
                        .gap_1()
                        .items_center()
                        .p_1()
                        .rounded_md()
                        .border_1()
                        .border_color(if invalid { theme.danger } else { theme.border })
                        .bg(theme.popover)
                        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                            if ev.keystroke.key == "escape" {
                                this.close_find(&CloseFind, window, cx);
                                cx.stop_propagation();
                            }
                        }))
                        .child(div().w(px(200.)).child(Input::new(&bar.input).small()))
                        .child(
                            Button::new("terminal-find-prev")
                                .icon(IconName::ChevronUp)
                                .ghost()
                                .small()
                                .tooltip("Previous match")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.find_step(false, window, cx)
                                })),
                        )
                        .child(
                            Button::new("terminal-find-next")
                                .icon(IconName::ChevronDown)
                                .ghost()
                                .small()
                                .tooltip("Next match")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.find_step(true, window, cx)
                                })),
                        )
                        .child(
                            Button::new("terminal-find-close")
                                .icon(IconName::Close)
                                .ghost()
                                .small()
                                .tooltip("Close")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_find(&CloseFind, window, cx)
                                })),
                        ),
                );
        }
        root
    }
}
