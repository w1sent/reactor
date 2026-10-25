# The window's shortcuts are the person's to choose, and a panel's shows it before it closes it

> Amended 2026-10-25 by [ADR-0068](0068-the-console-is-a-terminal-on-alacritty-terminal.md). The
> console is now a terminal, and a terminal needs the keys this ADR made window-wide: `ctrl-p` is
> the shell's "previous line", `ctrl-w` kills a word, `Escape` is `vim`'s. A window-wide shortcut
> therefore has a flag saying whether it stays active while a terminal has the focus, and the
> others are shadowed there. See "A shortcut is shadowed in a terminal" below; the rest stands.

The Panels menu was the only way to reach a panel besides the rail and the palette, and
three keys (the palette, the settings, closing a tab) were fixed in the code. A Panels item
toggled whether the panel was in the dock, not whether it could be seen. So Console, with
the bottom dock collapsed, took the console out instead of showing it, and the item looked
as if it had done nothing. Closing the console also left the bottom dock behind as an
empty frame.

## Decisions

**A panel's menu item or shortcut shows it, and closes it only when it is in view.** In
view means the tab in front of its group, in the centre or in a dock that is not
collapsed. A panel that is there but hidden is brought forward and its dock expanded; a
panel that is not there opens where the layout keeps it. A side or bottom dock whose last
panel is closed is taken away rather than left empty, and opening a panel there creates
it again.

**Every panel and four commands have a shortcut.** By default these are the rail's panels
on `secondary-1` to `secondary-4` in the rail's order; Transcript, Graph and Documents on
`secondary-shift-t`, `-g` and `-d`; the console on ``ctrl-` `` and a new console on
`ctrl-~` (shift and the backquote key, as terminals have it); and the palette, the
settings and closing a tab on the keys they had. `secondary` is Cmd on macOS and Ctrl
elsewhere. The Panels menu shows each item's keys and gained New Console.

**The person changes them on a Shortcuts page of the settings, and the change applies at
once.** Choices are kept in `~/.reactor/gui.json` (`ui.shortcuts`) by a stable id, only
where they differ from the default; an empty choice means no keys. Keys are applied on
Enter or when the field loses the focus, never while they are typed: `c` on the way to
`ctrl-k` would otherwise be bound for a moment. Keys that cannot be bound are refused with
the reason, and two shortcuts on the same keys are both marked, comparing the keys as
pressed, so `secondary-1` and `ctrl-1` are the same on Linux.

**A window-wide shortcut needs Ctrl, Alt, Cmd or Fn, or must be a function key.** A bare
key or `shift-k` would take that key from every text field: the bindings are window-wide,
and gpui matches a binding before the input sees the key.

**A shortcut is shadowed in a terminal, unless it is on the short list that is not.** The
terminal has the key context `Terminal`. gpui matches a binding before the focused element sees
the key, so a window-wide `ctrl-w` would never reach a shell. Each shortcut has a flag, *stays
active in a terminal*, and the bindings of those without it are installed with the context
`!Terminal`. Escape, which the composer binds, is the same. The ones that stay active are the
console's own two (toggle, new), the terminal's copy, paste and find, and every shortcut on `cmd`
on macOS, where `cmd` collides with nothing a terminal sends. Everything else, the rail's panels
on `secondary-1` to `-4` among them, is shadowed on Linux and Windows, where `secondary` is Ctrl.
The choices stay stored by stable id; the page marks a shortcut that is shadowed in a terminal,
so it is not a surprise.

**These bindings are installed apart from all others.** They are tagged, and a change takes
out only the tagged ones and puts the new ones in. The keys of the components (inputs, the
dock) and of single panels (the graph, the editors) depend on where the focus is, are not
window-wide, and stay fixed.

## Consequences

- The defaults leave the keys of gpui-kit's components alone, which a test cannot check
  for every platform. Tests check that the defaults parse and do not clash with each
  other, that every panel has one, and how hand-edited choices are cleaned up on load.
- A shortcut the person moves onto keys a component uses (`ctrl-a` in a text field) takes
  them over everywhere. The page says what a clash with another shortcut is, but not a
  clash with a component's keys.
- The palette lists panels without their keys.

## Considered and rejected

- **Toggling presence, as before, plus a separate "show" command.** Two items per panel
  for what one does correctly.
- **Applying keys as they are typed, which is what the settings component's text field
  does.** It bound every prefix of what was typed, a bare letter among them.
- **Rebuilding the whole keymap on a change.** The components bind their keys once at
  startup and cannot be asked again, so clearing everything would lose them.
- **Recording keys by pressing them.** It reads better, but a field that takes every key
  cannot be left with Tab or Escape, and a sequence like `ctrl-k ctrl-s` has no natural end.
  Typed keys cover both.
