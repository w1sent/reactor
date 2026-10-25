//! The console as a terminal emulator (docs/adr/0068).
//!
//! `backend` is the only file that names the emulation crate. The rest of the module is split by
//! what it knows: `keys`, `mouse`, `palette`, `glyph`, `metrics`, `links` and `rows` are pure and
//! tested as they are; `element` paints and `view` is the gpui entity; `session` starts the
//! program.

pub mod backend;
pub mod element;
pub mod glyph;
pub mod inbox;
pub mod keys;
pub mod links;
pub mod metrics;
pub mod mouse;
pub mod palette;
#[cfg(all(test, unix))]
mod programs_tests;
pub mod rows;
pub mod session;
pub mod types;
pub mod view;
#[cfg(test)]
mod view_tests;

pub use session::Kind;
pub use view::{Blink, Look, TerminalEvent, TerminalView};

use gpui_kit::{App, KeyBinding, NoAction, actions};

actions!(
    terminal,
    [
        /// Copy the selection (`cmd-c`, or `ctrl-shift-c` where `ctrl-c` is the interrupt).
        CopySelection,
        /// Paste the clipboard (`cmd-v`, `ctrl-shift-v`).
        PasteClipboard,
        /// Select everything, the history included.
        SelectAllText,
        /// Open the find bar.
        OpenFind,
        FindNext,
        FindPrevious,
        /// Close the find bar and give the focus back.
        CloseFind,
    ]
);

/// The key context of a focused terminal. Window-wide shortcuts that a shell needs are inactive
/// while it has the focus (docs/adr/0054).
pub const CONTEXT: &str = "Terminal";

/// The terminal's own chords: copy and paste are on Shift with Ctrl, because plain Ctrl-C is the
/// interrupt; on macOS they are on Cmd, which collides with nothing a terminal sends.
pub fn bind_keys(cx: &mut App) {
    let ctx = Some(CONTEXT);
    // gpui matches a binding before the focused element sees the key. Three of the window's take
    // keys a program needs: Tab and Shift-Tab move the focus (the root view's), and Ctrl-C copies
    // (a global binding for selected text), where in a terminal it is the interrupt. A `NoAction`
    // in the deeper context lets them through to the program; a text field inside the terminal
    // (the find bar) has its own bindings, which are deeper still.
    cx.bind_keys([
        KeyBinding::new("tab", NoAction, ctx),
        KeyBinding::new("shift-tab", NoAction, ctx),
        KeyBinding::new("ctrl-c", NoAction, ctx),
    ]);
    if cfg!(target_os = "macos") {
        cx.bind_keys([
            KeyBinding::new("cmd-c", CopySelection, ctx),
            KeyBinding::new("cmd-v", PasteClipboard, ctx),
            KeyBinding::new("cmd-a", SelectAllText, ctx),
            KeyBinding::new("cmd-f", OpenFind, ctx),
            KeyBinding::new("cmd-g", FindNext, ctx),
            KeyBinding::new("cmd-shift-g", FindPrevious, ctx),
        ]);
    } else {
        cx.bind_keys([
            KeyBinding::new("ctrl-shift-c", CopySelection, ctx),
            KeyBinding::new("ctrl-shift-v", PasteClipboard, ctx),
            KeyBinding::new("ctrl-shift-a", SelectAllText, ctx),
            KeyBinding::new("ctrl-shift-f", OpenFind, ctx),
            KeyBinding::new("f3", FindNext, ctx),
            KeyBinding::new("shift-f3", FindPrevious, ctx),
        ]);
    }
}
