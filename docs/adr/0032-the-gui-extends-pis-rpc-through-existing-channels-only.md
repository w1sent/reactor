# The GUI extends pi's RPC through existing channels only

When reactor's extensions run under reactor-gui, they get richer UI than
pi's RPC protocol offers — but the extension rides **only** channels that
already exist in pi's RPC mode, and every addition is invisible or harmless
to any other pi client and to the TUI. No new command types, no patched pi,
no upstream dependency. The contract's specification is
[`gui/SPEC.md`](../../gui/SPEC.md) §4; the pi facts it rests on are recorded
in [`pi-api-notes.md`](../pi-api-notes.md).

## Why

**There is no upstream path.** The protocol cannot grow a
`set_component`-style method or a `navigate_tree` RPC command without pi
itself changing — which is not available to us. So every capability must be
expressible with what pi already does.

**It happens to be fully expressible, because every channel is already
transparent:**

- *Detection*: reactor-gui sets `REACTOR_GUI=1` in the child's environment;
  extensions read `process.env`. No protocol surface at all.
- *Outbound*: a view is a `setWidget` with string lines — the only widget
  form RPC mode forwards. Line 0 carries a marker + JSON, the remaining
  lines a readable fallback; the GUI parses the marker, every other client
  renders the fallback. A client that never heard of reactor shows a correct
  table.
- *Inbound*: user actions are extension commands invoked through the
  documented `prompt` path. Verified in the shipped dist: extension
  commands execute immediately even mid-stream, and leave no transcript
  entry — pi returns "no prompt to send". The command is registered by the
  extension that owns the view (extensions share no state, [ADR-0014](0014-extensions-share-the-cache-not-each-other.md))
  and advertised inside the view payload, so the GUI never guesses.
- *Branch switching*: RPC has no `navigate_tree` command, but pi wires
  `ctx.navigateTree` into extension command contexts in RPC mode
  (verified). A small `gui-bridge` extension registers `/reactor-tree`
  calling it; in the TUI the builtin `/tree` already covers the need, and
  the command is harmless there.
- *Blocking input*: unchanged — `select`/`confirm`/`input`/`editor` already
  have a first-class RPC request/response sub-protocol.

**The mode matrix is the guarantee.** TUI: factories and `ctx.ui.custom()`,
unchanged. RPC without the GUI: the existing text fallbacks, unchanged —
other clients keep working. RPC with the GUI: envelope views and event
commands. An extension checks which world it is in and picks; the reactor
CLI calls behind the actions are the same in all three.

## What it costs

- **JSON inside widget lines is inelegant**, and the envelope is
  reactor-private vocabulary: it must be versioned (`v` field) and the
  fallback lines are mandatory, because they are the transparency.
- **Schema restraint is a rule, not a preference.** v1 is `table`/`list`/
  `detail` + row actions + footer, because that is what the selector and
  guide need. Extensions that need more propose schema v2 first.
- **Statelessness on the GUI side** ([ADR-0029](0029-extensions-share-stateless-presentation-code.md)
  discipline): the GUI keeps scroll position only; facts live in the
  extension and the CLI/cache. Re-render and refresh are one path: send a
  new envelope.

## Considered and rejected

- **New RPC command types** (e.g. `{"type":"ui_event"}`). Rejected outright:
  pi's RPC errors on unknown commands — the least transparent thing
  possible.
- **Proposing the protocol upstream (structured widgets, navigate over
  RPC).** Would be the cleanest long-term home, and is where this belongs
  if it ever grows beyond reactor — but it is not available, and the
  contract must work on stock pi.
- **gpui-shell JavaScript extensions drawing UI in-process.** A second
  extension system beside pi's, split-brained for authors, and it does
  nothing for the TUI path.
- **A reactor-side file/IPC channel as the real transport** (widgets to a
  socket, events from it). More capability, but a second transport to
  discover, secure and keep alive — for UI the stdio channel already
  carries.
- **Embedded terminal as the UI mechanism** (render pi's TUI in a terminal
  widget, keep `ctx.ui.custom()` everywhere). Preserves every TUI feature
  at zero protocol cost, but produces a terminal in a window rather than
  the native panels the GUI exists for. Kept as a documented fallback, not
  a path.