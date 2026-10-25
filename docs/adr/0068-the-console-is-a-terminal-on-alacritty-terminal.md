# The console is a terminal on `alacritty_terminal`

> Replaces the line-log console of [ADR-0052](0052-the-window-is-a-rail-a-drawer-and-a-workspace.md)'s
> bottom dock and the "v0.2 is deliberately not a screen" and "v0.3 if and only if" text of the
> GUI's SPEC. What 0052 decided about where the console sits (a workspace tab, collapsed at
> start) stands. Amends the key handling of
> [ADR-0054](0054-the-window-s-shortcuts-are-the-person-s-to-choose.md), which says so in a note.

The console ran one command at a time on a pty and showed what came back as a list of lines.
A small reader kept colour and the rewriting of a line, and threw away every other escape
sequence. That was enough for `reactor install <id>` and for `sudo` to ask for a password,
and it was the reason a pty was used. It is not enough for what a person puts in a terminal:
`vim`, `htop`, `less`, `tmux`, a REPL with line editing, a progress bar that moves the cursor,
a shell that is left running. The prompt was a text field below the output, so a program that
read single keys, or drew a screen, could not be used at all. The person leaves for a real
terminal, and the session's evidence, commands and files are then in two places.

## Decisions

**The console is a terminal emulator.** A console tab hosts either an interactive shell (the
default for a new one) or a task program (`reactor install <id>`, `/console <cmd>`). There is
no prompt row and no line log: the grid, the cursor, the alternate screen, the scrollback and
the mouse are the program's. The line-log model, and the SPEC's plan to build a terminal only
"if and only if" a program needed a screen, are replaced.

**Emulation and the pty loop come from the published `alacritty_terminal`, pinned exactly.**
`alacritty_terminal = "=0.26.0"` from crates.io: no fork, no git or `[patch]` dependency, no
vendored copy. The crate's API moves between minors, so the pin is exact and one module
(`terminal/backend.rs`) is the only file that names its types; the rest of the GUI sees
REactor's own small `Cell`, `Modes`, `Cursor` and `Event`. A defect in the crate that blocks the
work is reported and decided on, not worked around quietly.

**The view is a custom gpui element that paints a snapshot.** The crate's reader thread parses
bytes into a `Term` behind a fair mutex. The UI never reads the pty. It takes the lock briefly,
copies the visible rows, the cursor, the selection and the mode into a plain value, releases
the lock and paints from the copy. Colours are the theme's tokens, and the 256-colour cube and
grey ramp are computed. There is no per-panel poll: the reader thread's events wake the panel
through a channel awaited on gpui's executor, the first at once and the rest coalesced for a few
milliseconds, so an echo does not wait for a tick and a flood does not starve input
([ADR-0042](0042-the-gui-hosts-the-agent-in-process.md): awaiting a channel on gpui's executor
is not a second runtime).

**A shell by default; installs and `/console <cmd>` are task programs.** A shell session
starts when the panel is first laid out, with the real size of its grid, not when the console
is created: the first console is made while the bottom dock is collapsed, and a hidden shell
would have the wrong size and cost a process. A task program keeps its output when it exits,
the pane stays open to scroll, select and copy, a strip under the grid says `exited with code N`
(silent on success) and offers *Rerun*. Closing the tab ends the session. *Stop* sends Ctrl-C
to the foreground group the way a terminal does; closing hangs up.

**The catalogue and the services are re-read** when a task finishes and when a shell exits.
A long-lived shell has no "command finished" signal without shell integration, so they are
also re-read when the terminal loses the focus after output arrived, at most once in a while.

**A terminal gets the keys.** The terminal's root has the key context `Terminal`, and the
window's shortcuts that a terminal needs (`ctrl-p`, `ctrl-w`, `ctrl-1` and `Escape`) are
inactive while it has the focus. Which stay active is in the amendment to
[ADR-0054](0054-the-window-s-shortcuts-are-the-person-s-to-choose.md). Copy and paste in a
terminal are `cmd-c` and `cmd-v` on macOS and `ctrl-shift-c` and `ctrl-shift-v` elsewhere,
because `ctrl-c` is the interrupt. gpui matches a binding before the focused element sees a key,
so three keys that the toolkit binds for the whole window are released in the `Terminal` context
too: Tab and Shift-Tab (the root view moves the focus with them) and Ctrl-C (a global copy of
selected text). A test lists every key the terminal sends and fails if any other binding would
take one.

**The panel takes the crate's lease before it locks the grid.** The reader thread reads the pty,
and if the grid is locked by the panel at that moment it reads again before parsing; when the
program has just exited, that second read fails and the bytes already read are dropped. The last
output of a task that ends while the screen is being copied was lost this way, in a third to a half
of the runs of a test that copies the screen continuously. Every panel-side lock of the grid
(`Backend::lock`) takes the lease first, which the reader also takes before it reads, so the
reader waits before it reads and not after. This is a workaround in our own code for a defect of
0.26.0, chosen by the person over documenting it; the reproduction is a test.

**The clipboard is written to by a program, not read from.** OSC 52 may set the clipboard
(the crate's `OnlyCopy`); a program cannot ask for its contents, which would be a way to take
whatever the person last copied. Paste is the person's own action, bracketed when the program
asked for it.

**The child is told what it talks to**, per child and never process-wide: `TERM=xterm-256color`,
`COLORTERM=truecolor`, `TERM_PROGRAM=REactor`, a `LANG` when none is set. Variables that name the
terminal REactor itself was started from (`ALACRITTY_*`, `KITTY_*`, `TMUX*` and the like) are
removed, so a program does not believe it is in a terminal it is not.

**Windows is in scope.** The crate uses ConPTY there. The program and arguments of a task, and
the default shell, are built in one place for the platform (`pwsh`, then `powershell`, then
`%COMSPEC%`; `cmd /C` for `/console <cmd>`), and the call sites do not change.

## Consequences

- `console.rs`, `ConsoleBuffer`, `AnsiReader`, the prompt `Input` row and the console's
  `MessageScroller` go, and with them `portable-pty` from this crate. `ansi_color` goes with the
  line log; the terminal maps the 16 colours onto the theme's tokens, `_light` variants for the bright ones.
- A terminal can be drawn by programs that assume things a log never met: wide and combining
  characters, the emoji presentation selector, 24-bit colour, mouse reporting. Where the crate
  gets one of them wrong the pane shows it, and a regression test records which. Two are known in
  0.26.0, the newest release, and are kept as ignored tests that say why (a third, lost output
  at exit, is worked around, above): a character followed by
  U+FE0F (a heart, a warning sign) occupies one cell in the grid while a font draws it two wide,
  so it overlaps the next cell; and every combining mark of a cell is appended to a list with no
  limit, so a stream of them makes one cell large and every snapshot that copies it slower.
  Neither is worked around here; the choice not to was the person's, and the tests are what to
  turn on when a release fixes them.
- The window's shortcuts are shadowed in a focused terminal (the Shortcuts page says which).
  A person who relied on `ctrl-w` closing the console tab while typing in it now uses the tab's
  own close or the palette.
- The engine's types can only add or override variables of the child, not remove them, so those
  that name the terminal REactor was started from (`KITTY_*`, `TMUX`, and the like) are set empty
  rather than absent.
- A running shell is killed when its tab closes. A program in it that ignores a hang-up is ended
  with the shell; nothing is asked first (see below).
- The font is the console font of the settings; the scrollback length, the cursor and the
  minimum contrast are new settings under `ui`.
- Memory: a scrollback of 10 000 rows is held for each console that has run, and none for one
  that never started.

## Left for later

- Shell-integration marks (OSC 133), which would tell the app when a command finishes and let
  the catalogue refresh follow it instead of the focus.
- Kitty's keyboard protocol and graphics protocol. The keyboard mode stays off in the crate,
  and keys are encoded for xterm.
- A cache of shaped rows, keyed by the crate's damage. Painting is measured first.
- Asking before closing a terminal that is running something other than its shell.
- Opening a `path:line` in the app's own file viewer from the terminal's output, as a URL is
  opened in the browser.

## Considered and rejected

- **Keep extending `AnsiReader`.** Each program that moves the cursor needs another sequence,
  and a line list cannot hold a screen; it ends as a poor terminal emulator written by hand.
- **Write the parser and grid here**, on `vte`. A terminal is mostly details that are known and
  boring (scroll regions, wrapping, wide cells, modes, the alternate screen, reflow on resize),
  and the crate has them tested by users. It would have been a second project.
- **`portable-pty` with the crate's `Term`.** The pty of the old console, feeding bytes into the
  grid. Rejected because the crate's loop brings a non-blocking write queue, a fair lock between
  the parser and the reader of the grid, and the handling of the child's exit and of draining its
  output, all of which would be written again.
- **A smaller step first** (a screen without the mouse, or without the shell). It would be thrown
  away: the work is the same grid and the same keys.
- **A fork of the crate** for the defects another embedder has recorded. Rejected as a starting
  point; the tests that would show them run against the published release, and a failure is a
  decision for the person, with the options.
- **Letting a program read the clipboard** (OSC 52 paste). Convenient for a remote editor, and a
  way for any program to take what the person copied.
