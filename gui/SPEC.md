# reactor-gui — the native frontend

**Status: specification, agreed.** Every decision here was settled in a
design review (grilling, round by round); the two that change the repo's
structure are recorded as ADRs
([0031](../docs/adr/0031-reactor-gui-lives-in-the-repo-and-installs-with-it.md),
[0032](../docs/adr/0032-the-gui-extends-pis-rpc-through-existing-channels-only.md)),
the pi facts this rests on are in
[`docs/pi-api-notes.md`](../docs/pi-api-notes.md) under *"Facts for
reactor-gui, verified against pi 0.87.0"*.

reactor-gui is a Rust desktop application (gpui-kit) that runs a REactor
session in a native window: transcript, composer, catalogue, live services,
and the UI reactor's extensions want to show. It talks to pi the way pi
recommends for custom UIs — RPC mode over stdio — and it changes nothing
about pi or about the TUI: `pi` in a terminal remains a fully supported,
first-class way to run REactor, byte for byte as it is today.

```
┌─────────────────────────────────────────────────────────────────┐
│ reactor-gui (Rust, gpui-kit)                                     │
│  ┌──────────┐ ┌───────────────────────────┐ ┌────────────────┐  │
│  │ session  │ │ transcript                │ │ catalogue &    │  │
│  │ tree     │ │  markdown · thinking ·   │ │ toolsets       │  │
│  │ (from    │ │  tool cards · entries     │ │ ────────────── │  │
│  │ get_tree)│ │ ────────────────────────  │ │ services (live)│  │
│  │          │ │ composer                  │ │ + side views   │  │
│  └──────────┘ └───────────────────────────┘ └────────────────┘  │
│  status bar: extension statuses · model · thinking · context %   │
└─────────────────────────────────────────────────────────────────┘
        │ spawn, env handshake        ┌──────────────────────────┐
        └───────────────────────────▶│ pi --mode rpc (JSONL)     │
                                      │  ├ reactor extensions    │
                                      │  ├ reactor CLI (pi.exec) │
                                      └──────────────────────────┘
        reactor-gui also runs `reactor … --format json` directly
        for the side panels (single source of truth: the CLI).
```

## 1. What it is, and is not

- **A second frontend, not a replacement.** TUI mode is untouched; a session
  started in the GUI is a normal pi session file, resumable from the terminal.
- **A GUI for the person at the keyboard.** It does not change what the agent
  is told (that is the registry's job, unchanged), does not add tools, does
  not parse `tools.toml` (ADR-0005 holds inside the GUI too — the CLI is the
  only source of catalogue facts).
- **Pi-coupled at exactly one seam**: the documented RPC protocol and the
  extension UI sub-protocol. Nothing private, nothing patched, no fork of pi.

## 2. Transport: RPC mode

`pi --mode rpc`, one child process per GUI launch, JSONL over stdin/stdout,
driven from Rust with serde — a dedicated reader thread and a
mutex-guarded writer, deliberately no async runtime (one reader thread and a
linear protocol need nothing more, and the GUI crate stays free of an
executor war with gpui's own). Why RPC and not the alternatives:

- **SDK** embeds pi in a Node.js process; the GUI is Rust. Bridging Node is
  just RPC with a heavier middle.
- **JSON mode** (`--mode json`) is one-shot print: no input channel, no
  abort, no model control. Not an interactive frontend.
- **Embedded terminal** (rendering pi's TUI inside a terminal widget) would
  keep every TUI feature but yields a terminal in a window, not native
  panels. Recorded as a fallback shape, deliberately not built.

**Client rules:**

- Framing: split records on `\n` only, strip one trailing `\r`. Never use a
  Unicode-aware line splitter (pi's docs warn about `U+2028`/`U+2029` in
  strings — the JSON payload may contain them).
- Correlate commands by `id`; responses are
  `{type:"response", command, success, …data|error}`.
- Events stream asynchronously; `message_end.message` is authoritative,
  `message_update` deltas are assembled by `contentIndex` (never render a
  partial as final).
- Dialog requests (`extension_ui_request` with method
  `select|confirm|input|editor`) must be answered with a matching
  `extension_ui_response` on stdin; timeouts are handled agent-side (pi
  auto-resolves), so the GUI may simply respond whenever the user does.
- Keep reading promptly: pi watches stdout backpressure.

**Version floor: pi 0.87.0.** At startup the GUI checks `pi --version`,
refuses below the floor with a clear message, and runs a `get_state` smoke
test. Unknown event types never crash: they render as raw JSON in a debug
view. Protocol drift is a documented risk (same discipline as
`docs/pi-api-notes.md` — re-verify the RPC surface on every pi bump).

## 3. Session model: pi's own, one session per launch

The GUI does **not** switch, fork, or create sessions at runtime. A GUI
process is bound to exactly one session, chosen at launch exactly the way
`pi` chooses one; the launch flags pass through:

| launch | pi child |
|---|---|
| `reactor-gui <dir>` | `pi --mode rpc` in that cwd (new session) |
| bare `reactor-gui` | **workdir chooser** first (below), then the above |
| `-c` / `--continue` | forwarded — pi continues the recent session |
| `-r` / `--resume` | **native session picker** (below), then `--session <path>` |
| `--session <path\|id>` | forwarded |
| `--fork <path\|id>` | forwarded — fork-then-open, pi's own `--fork` semantics |
| `--no-session` | forwarded — ephemeral scratch session |
| `--name` | forwarded |

- **Workdir chooser** (nothing passed): a startup panel — left half lists
  frequently used folders (the GUI's own recents, see §8), right half is a
  directory picker. Choosing one spawns the child with that cwd.
- **Session picker** (`-r`): pi's `--resume` picker is interactive-only, so
  the GUI implements its own: it scans the shared store
  (`~/.pi/agent/sessions/<encoded-cwd>/`) and reads header lines — the same
  store, the same files, no pi dependency for listing — then spawns
  `pi --mode rpc --session <path>`. (`--continue`/`--session` are resolved by
  pi before mode dispatch, so they work in RPC mode; verified, see notes.)
- **Shared store, soft exclusivity.** pi has no session-file locking (verified)
  and REactor will not bolt one on. The GUI keeps its session exclusively
  *by contract*: one attached frontend per session file; concurrent writes by
  another pi process show up as leaf drift and are warned about, not fought.
- The child is killed when the window closes (pi handles SIGTERM cleanly).

## 4. The extension UI contract (GUI mode)

The problem: reactor's extensions already draw UI in the TUI (the selector's
two-pane curator, the guide's pages, the status panel) through
`ctx.ui.custom()` and widget factories — both are TUI-only and degrade over
RPC. The GUI must render extension UI natively, and future extensions must
have a way to get UI into the GUI, without touching pi and without breaking
any other pi client.

**The contract: reactor-specific, additive, and transparent — it rides only
channels that already exist in pi's RPC.** Nothing we send is an unknown
command; nothing another client receives is unrenderable.

### 4.1 Handshake

The GUI spawns pi with `REACTOR_GUI=1` in the environment. Extensions read
`process.env.REACTOR_GUI` once at registration. Detection matrix:

| mode | what the extension sees | behaviour |
|---|---|---|
| TUI | `ctx.mode === "tui"` | factories, `ctx.ui.custom()` overlays — **unchanged** |
| RPC, no GUI | `ctx.mode === "rpc"`, no env | text fallbacks — **unchanged** (other clients keep working) |
| RPC, GUI | env set | envelope views + event commands (below) |

### 4.2 Outbound: the view envelope

A view is a `setWidget` whose payload carries a marker line, JSON, and a
human-readable fallback:

```
ctx.ui.setWidget("reactor:selector", [
  "REACTOR-GUI-VIEW v1 " + JSON.stringify(payload),
  "…one or more readable lines…",        // fallback for every other client
])
```

- Key form: `reactor:<viewId>`. The GUI recognizes the envelope by the
  marker prefix on line 0; every other client renders the fallback lines —
  transparency by construction, because RPC `setWidget` is string-arrays-only
  (verified — factories are silently ignored, so text is the only carrier).
- The fallback lines are mandatory and must be useful on their own (a
  client that cannot parse the envelope still shows a correct table).
- Clearing: `setWidget(key, undefined)`.

**Payload, schema v1** — deliberately minimal; nothing ships that the two
reference implementations don't need:

```json
{
  "v": 1,
  "view": "selector",                       // viewId
  "title": "Tools",
  "command": "/reactor-tools-event",        // the event command for actions
  "placement": "overlay",                   // "overlay" | "side"
  "table": {
    "columns": [
      { "id": "tool",  "title": "Tool", "width": 16 },
      { "id": "state", "title": "",    "width": 4  }
    ],
    "rows": [
      { "id": "bn",
        "cells": { "tool":  { "text": "bn" },
                   "state": { "text": "●", "color": "success" } },
        "actions": [ { "id": "toggle", "label": "Toggle", "disabled": false } ] }
    ]
  },
  "footer": "9 active · 12 catalogued"
}
```

- Primitives v1: `table` | `list` | `detail` (title + markdown body). Per-row
  `actions`, optional `footer`. No nested views, no free-form components —
  an extension that needs more proposes schema v2.
- `color` values use **pi's theme vocabulary** (`text`, `accent`, `dim`,
  `muted`, `success`, `error`, `warning`, …) — mapped GUI-side through the
  same token table as the theme (§7), so payload colors and window colors
  can never disagree.
- `placement`: `overlay` = a sheet over the transcript (selector, guide —
  matching today's TUI overlay semantics); `side` = a panel in the right
  dock.

### 4.3 Inbound: per-extension event commands

User actions go back as **extension commands invoked through RPC `prompt`**:
the GUI sends `{"type":"prompt","message":"/reactor-tools-event <json>"}`.
Verified properties that make this the right channel:

- extension commands execute immediately, even mid-stream (handled before
  the streaming check);
- the command leaves **no transcript entry** — pi returns "no prompt to
  send", so UI actions never pollute the model's context;
- commands cannot be queued via `steer`/`follow_up`, so events always run
  promptly.

Each extension registers its own event command (no shared dispatcher —
extensions share no state, ADR-0014) and advertises it in the envelope's
`command` field, so the GUI never guesses. The handler performs the action
(usually via the `reactor` CLI through `pi.exec`, ADR-0005) and repaints a
fresh envelope. Open→act→repaint is one code path; there is no second
protocol.

### 4.4 Statelessness — the GUI holds no facts

The GUI renders what the envelope says and keeps only scroll/viewport. Tool
activation state, service state, guide page — all live in the extension and
the CLI/cache, exactly as ADR-0029 rules for the TUI. Re-render and refresh
are the same operation: send a new envelope.

### 4.5 Blocking input stays on pi's native dialogs

`ctx.ui.select/confirm/input/editor` already translate to
`extension_ui_request`/`extension_ui_response`; the GUI renders these as
native modals and answers on stdin. No envelope for dialogs — the existing
protocol is already good.

### 4.6 The tree bridge

pi's `/tree` is interactive-only and RPC has no `navigate_tree` command
(verified — the command switch ends at `get_commands`), but pi wires
`ctx.navigateTree(targetId)` into extension command contexts in RPC mode.
`extensions/gui-bridge/` registers `/reactor-tree <entryId>` calling it —
the session tree panel's "switch branch" action, transparent (in the TUI the
builtin `/tree` already exists; the command is harmless there and works too).
While the agent streams or compacts, navigation rejects (pi's own rule); the
GUI disables the action during streaming.

### 4.7 Who changes on the reactor side

The coupling is why the GUI lives in this repo (ADR-0031):

- `extensions/lib/guiview.ts` — stateless helpers (ADR-0029 pattern): build
  envelope JSON + fallback lines, detect GUI mode.
- `selector/`, `guide/` — a GUI-mode branch: envelope view instead of
  `ctx.ui.custom()`; their event commands; the same reactor-CLI calls behind
  the actions as behind the TUI keys.
- `extensions/gui-bridge/` — the tree command (and future GUI-only bridges).
- `status/` stays as-is: the GUI builds its services panel from the CLI (§5),
  so the extension needs no envelope.

## 5. Data panels: the CLI, through a seam

Side panels (catalogue, toolsets, services) come from the `reactor` CLI's
`--format json` outputs, invoked by the GUI through a Rust trait:

```rust
trait ReactorClient {
    async fn tools(&self) -> Result<ToolsPayload>;
    async fn toolsets(&self) -> Result<ToolsetRows>;
    async fn services(&self, refresh: bool) -> Result<ServicesPayload>;
    // …mirrors the CLI's JSON contract, command for command
}
```

- **Today**: `CliClient` — shells out (std::process with a watchdog timeout
  of 20s, the extensions' own backstop), no `--refresh` by default so the
  CLI's own TTLs and `cache.json` decide freshness (ADR-0014 — the GUI adds
  no second probe policy).
- **Later**: a Rust port of the CLI lands as its own sibling repo (ADR-0002)
  and is consumed as a crate behind the same trait (`LibClient`). The GUI
  never parses `tools.toml` before that day and never after it.

## 6. The window

Single window, gpui-kit (`gpui-kit = "0.6.0"` pinned), `DockArea` with a
serialized layout. **Ayu Dark theme by default** (§7), Lucide icons
(gpui-kit's bundled default).

- **Center**: transcript + composer.
  - Transcript: assembled from `message_start`/`message_update`/`message_end`
    and tool-execution events; reconciled against `get_entries` on load.
    Assistant text renders as markdown; thinking blocks render collapsed;
    tool calls are cards (name, args, streamed output, result, error state).
    Custom entries render natively by `customType`
    (`reactor-detail`, `reactor-reporting`, `reactor-scenario`,
    `pi-goal-setting`, `pi-identity`, `pi-history-tools`,
    `pi-rolling-context`, `pi-auto-continue`, `pi-context-editor`), with a
    generic (title/body/JSON) fallback so an unknown future entry type still
    shows.
  - Composer: multi-line text area. `Enter` sends (or queues **follow_up**
    while streaming, with queue state shown); `Shift+Enter` newline; `Esc` =
    `clear_queue` then `abort`, restoring the returned queue text into the
    composer (exactly what `clear_queue`'s return exists for); an explicit
    Interrupt button aborts. Model picker (`get_available_models`,
    `set_model`) and thinking picker (`get_available_thinking_levels`,
    `set_thinking_level`) in a header bar; context % from
    `get_session_stats`. No steer gesture in v0.1 — follow-up + abort cover
    it; steer can join later through the same channel.
- **Left** (collapsible): the session tree from `get_tree` — pi's `/tree`
  as a panel. Click previews the entry in the transcript; explicit
  switch-branch action goes through the tree bridge (§4.6), disabled while
  streaming.
- **Right** (collapsible, tabs): a top tab group holding **Tools**, **Toolsets**
  (each its own tab, not one merged list — over `reactor tools/toolsets
  --format json`, with enable/disable actions that run the CLI — the
  selector's mutations without its overlay) and **Views** (envelope views
  with `placement: "side"`, gui/SPEC.md §4.2 — see below); a bottom slot for
  **Services** (live rows from `reactor services --format json`, poll on a
  TTL-respecting timer). Every side panel shows a loading indicator while its
  CLI/RPC round trip is in flight, never a bare "no data" before the first
  answer arrives.
  - **View rendering is one seam** (`reactor-gui/src/views.rs`): both the
    Views tab (`placement: "side"`) and the transcript's overlay sheet
    (`placement: "overlay"`, today's selector/guide) call the same
    `views::render_content`, which matches on `ViewContent`. Schema v1 ships
    `Table`/`List`/`Detail`; a future primitive (a timeline, a graph, a
    syntax-highlighted code view) is a new `ViewContent` variant plus one new
    `render_*` function here — every call site already renders whatever
    `render_content` returns, so nothing else changes when the catalogue of
    visualizations grows.
- **Bottom dock — the console** (`reactor-gui/src/console.rs` for the pty
  and ANSI handling, `ConsolePanel` in `reactor-gui/src/panels.rs` for the
  UI): a general command runner, not an install log. What the user types
  runs through their shell (pipes and `&&` included); what the GUI sends
  runs directly. Output streams into the same scrollback the command was
  typed into — one continuous area, not a separate input box floating above
  a separate output pane — because that is what makes a running program's
  own prompt (an installer's `Proceed? [y/N]`, a REPL) read naturally: the
  answer appears right where the question was asked, the way a real
  terminal works. A finished command is left to speak for itself in its own
  output; nothing appends a synthetic "done" line once it exits
  successfully (only a nonzero exit or a kill leaves a marker, since those
  are facts the output alone would not otherwise show).

  **The prompt is the scrollback's own last row**, not a control sitting
  below it: `ConsolePanel`'s `MessageScroller` renders `lines.len() + 1`
  rows, and the extra one past the real output is the live `InputState`
  (full cursor/editing/IME support, just relocated) — so it scrolls with
  the log and reads as its own next line, the way a terminal's cursor does,
  rather than a search box bolted under a results pane. Being the list's
  last row is not on its own enough to read that way: `Input`'s defaults are
  a bordered, backgrounded box with its own gold focus ring, which kept
  painting a visible search-box outline around the prompt regardless of
  where it sat. `.appearance(false).bordered(false).focus_bordered(false)`,
  plus matching the surrounding rows' font, size and zero padding, is what
  actually makes it read as bare terminal text with a cursor rather than a
  control embedded in the log. The whole log is
  selectable: every span is its own `SelectableText::new(...)` participant,
  ordered by line then position in it (`gpui_base::selectable_text`) — never
  several spans sharing one `TextSelectionHandle` via `with_handle`, which
  looked plausible (the type exists for exactly "several elements form one
  document") but isn't: a handle is one hitbox and one run slot in the
  window's selection state, keyed by the handle's entity id, so every span
  after the first silently overwrote the one before it each frame and only
  whichever span painted last was ever actually selectable — the bug behind
  "selection works most of the time." Each span owning its participant
  (`document_order` alone stitches them into one draggable selection) fixed
  it.

  Each `ConsolePanel` owns its session, buffer and polling loop itself
  rather than sharing state through `ReactorApp` — the toolbar's *New*
  button (or installing another tool) opens another, independent console
  panel via `ReactorApp::open_console`, so several commands can run at once
  without one's output interleaving into another's. Reader threads and a
  channel per console, drained on each panel's own timer at the same cadence
  the RPC pump uses — no second executor (§2's rule), just one per panel
  instead of one shared by all.

  **A pty, and what it is not.** Commands run on a pty, not pipes, because
  a program asks `isatty()` before deciding how to behave: on a pipe `sudo`
  refuses to prompt at all, which took out every Linux distro manager —
  `tools.toml` marks `pacman`, `apt`, `dnf`, `zypper`, `apk` and `port` as
  needing root, against `brew`, `uv`, `pipx`, `pip`, `cargo`, `go` and `npm`
  which do not. The cost lands on the way back: a program talking to a
  terminal emits escape sequences, so `console::AnsiReader` keeps the two
  that carry meaning for a log — colour (mapped onto the theme's own
  `red`/`green`/`yellow`/… hues, not xterm's) and the line-rewriting a
  progress bar does —
  and consumes the rest rather than printing it. It is deliberately not a
  screen: no cursor addressing, no scroll regions, no alternate screen, so a
  program that paints a UI (`vim`, `htop`) will not render. That is §9's
  v0.3, and it is a different component wearing the same name.

  `ConsoleBuffer` seals a line the GUI wrote itself (`push_line`, the `$
  command` echo, an exit notice) so the next byte of the command's own
  output starts a fresh line rather than extending it — `write` otherwise
  always extends `lines.last_mut()` regardless of who finished it last, so
  `whoami` followed by `user\n` rendered as one line, `whoamiuser`.

  - **Installing from the catalogue.** The Tools panel hides tools that are
    not installed — on a fresh machine that is most of the catalogue, and it
    buried the few the user has behind rows they cannot act on — behind a
    toggle that reveals them with an *install* action each. Install runs
    `reactor install <id>` in the console rather than anywhere private, so
    its plan, its questions and its failures are all in one visible place.
    A finished command re-reads the catalogue and services, so a tool that
    just appeared stops claiming it is missing.
- **Bottom**: status bar, under the dock. Left: extension statuses from
  `setStatus`, sorted by key so REactor's anchor (`0-reactor`) leads as
  designed. Right: model · thinking pickers (`set_model` /
  `set_thinking_level`) · context %. Plain-text widgets (non-envelope
  `setWidget`) render above the composer — `aboveEditor` parity.
- **Command palette**: `get_commands` + the prompt templates/skills it
  lists; invoking sends the `/command` through `prompt`.
- **Dialogs**: native modals for `select/confirm/input/editor`.

### 6.1 Layout: nothing is fixed furniture

Every panel is draggable between docks and droppable onto another panel's
tab bar — gpui-kit's dock does that once an area holds more than one tab
group, so the console can sit wherever the transcript can, and any two
panels can be stacked into one tabbed group. That "more than one tab group"
clause is load-bearing, not incidental: gpui-base's
`TabGroupContext::draggable` is `!is_locked() && !is_alone()`, and
`is_alone()` means *no sibling group in the same split* — a dock or centre
holding one lone tab group is undraggable no matter what the panel itself
allows. Every preset below therefore pairs each used area with a sibling
group (`layout::Panels::split`) rather than ever leaving one as a lone
group, which is what makes every panel actually rearrangeable rather than
only the ones that happened to land next to something else. `Focus` is the
deliberate exception — showing the transcript alone with nothing else on
screen is the point of it.

The GUI's own contribution on top of that freedom is the way back:

- **Named presets** (`reactor-gui/src/layout.rs`), each putting a *different*
  panel in the centre, which is the plainest statement that the centre is
  not owned by the transcript:
  - **Default** — transcript paired with the session tree in the centre,
    tools/toolsets paired with views on the right, console paired with
    services along the bottom.
  - **Focus** — the transcript alone, every dock gone.
  - **Analysis** — console paired with the transcript in the centre,
    catalogue and tree on the sides.
  - **Catalogue** — tools/toolsets/views paired with the tree in the centre,
    big enough to read descriptions rather than guess from ids.
- **A `Layout` menu** carrying the presets and a switch per dock. Defined
  once through `cx.set_menus`, which is the real menu bar on macOS and the
  same menus drawn in-window by `AppMenuBar` on Windows and Linux. Because
  that native menu bar has no window of its own, macOS validates and
  dispatches its items through `App`-global action listeners, not the
  focused window's dispatch path — so the handlers live behind
  `cx.on_action` on `App` (not an element-scoped `.on_action`), reaching the
  one window through a `MainWindow` global (`app.rs`) set when it opens: a
  window handle plus a weak `ReactorApp` entity, recovered with
  `cx.update_window`. An element-scoped handler alone left every item
  permanently greyed out, since there was never a focused window for macOS
  to ask.

  That `cx.update_window` call is itself wrapped in `cx.defer`, not called
  immediately: a menu click reaches the `App`-global handler from *inside*
  that same window's own action dispatch (gpui's `Window::dispatch_action`
  already takes its window's slot out of `App` for the duration), so calling
  back into it right there found "window not found" — the slot was already
  taken — and the discarded `Result` made every click look like it silently
  did nothing. `cx.defer` queues the call for after that borrow ends, the
  same way gpui's own `dispatch_action` defers itself past the click handler
  that triggered it.
- **Presets rearrange, they do not rebuild.** `layout::Panels` holds a handle
  to each panel from startup, so switching keeps the console's scrollback,
  the transcript's expanded thinking blocks and every scroll position —
  state that lives inside those entities and would be silently thrown away
  by constructing them afresh.

## 7. Theme

One shipped theme, **[Ayu Dark](https://github.com/ayu-theme/ayu-colors)**,
ported from upstream's `themes/dark.yaml` palette — a cool near-black with a
warm gold accent, and every semantic role its own hue rather than one accent
color overloaded for everything (the original "cyber dark" theme's mistake:
success and accent were the same green, so an active toggle and a "this
worked" state looked identical):

| role | value |
|---|---|
| background | `#0D1017` (cool near-black) |
| surface / panel | `#0F131A` – `#141821` (a visible step above background) |
| text | `#BFBDB6` (warm off-white, not stark) |
| muted | `#7C8798` |
| accent | `#E6B450` — Ayu's signature gold, reserved for accent + active states |
| success | `#70BF56` (green — distinct from accent) |
| warning | `#FF8F40` (orange — distinct from accent) |
| error | `#F07178` (red) |
| info / link | `#59C2FF` (blue) |
| border | `#242B36`, inputs `#2B3341` — visibly above the panel fill, not just a hair off it |

- Icons are gpui-kit's bundled Lucide catalog, resolved through the platform
  `AssetSource` the application registers at startup
  (`gpui_kit::application().with_assets(AllAssets)` — its absence is a silent
  failure, every icon paints nothing, no panic and no log; this bit v0.1 once
  and is the reason it is called out here).

- The GUI exposes a **semantic token set** (bg, surface, border, text,
  muted, dim, accent, success, warning, error, selection, hover, syntax*)
  and a documented **mapping table from pi's theme vocabulary** to those
  tokens — payload colors (§4.2) flow through the same table. Exact gpui-kit
  `ThemeColor` enum names are verified against 0.6.0 at build time and the
  table lives in this spec's implementation notes.
- Light theme: skeleton behind a flag, marked experimental in v0.1; the token
  set is theme-agnostic from day one so future themes are data, not code.
- No pi theme import in v1 (decided). Icons: Lucide, gpui-kit's default set.

## 8. Repository, install, tests

- `gui/` is a Cargo workspace inside this repo (ADR-0031):
  - `gui/crates/reactor-rpc` — JSONL client, serde types for commands,
    responses, events, extension UI requests.
  - `gui/crates/reactor-cli` — `ReactorClient` + `CliClient`.
  - `gui/crates/reactor-gui` — the application.
- `scripts/install.py` builds the GUI by default (`cargo build --release`,
  binary onto PATH like the CLI); when Rust is missing it prints a warning
  and finishes the rest of the install unchanged. `--skip-gui` opt-out.
- GUI-local config (recent workdirs) lives in `~/.pi/reactor-gui/` — not in
  the CLI's `~/.pi/reactor/`, not in pi's `~/.pi/agent/`.
- Tests: hermetic — `reactor-rpc`'s routing rules run against fixture lines
  with no child process (the routing rules are extracted from the reader
  thread for exactly that), `reactor-cli` against the real CLI's JSON
  contracts plus a `#[ignore]`d live round trip, and the contract's wire
  format mirrored by `tests/extensions/guiview.test.mjs` from the extension
  side. Live-pi tests are `#[ignore]`d (run manually).
  `npm test` and the python suite are untouched.

## 9. Scope

**v0.1** — the window as specified above: RPC bridge, transcript, composer
(follow-up/abort/Esc semantics, model + thinking pickers), right dock
(tools/toolsets + views + services), the bottom console with catalogue
installs, status bar, command palette, extension dialogs, the view-envelope
contract **with the selector and guide rebuilt on it as the two reference
implementations**, the session tree panel with branch switching, workdir
chooser + session picker, `/reload` stub, Ayu Dark theme.

**v0.2** — **shipped: a pty behind the console, with minimal escape
handling** (`portable-pty`, plus `console::AnsiReader` for colour and the
line-rewriting a progress bar does; everything else consumed, not printed),
**the layout system** (§6.1), a merged input/output console with selectable
text, and multiple independent console panels open at once. Still open: pi's
own `bash`/`abort_bash`
RPC surfaced in the same pane, HTML export (`export_html`), session labels +
clone, steer gesture, "open session in new window" launcher, light theme,
`LibClient` when the CLI port exists, and persisting a hand-arranged layout
across launches (`DockAreaState` serializes; nothing writes it yet).

**v0.3** — **full terminal emulation**, if and only if the console is ever
asked to host a program that draws a screen (`vim`, `htop`, a curses
installer). That means a real vt100 grid — cursor addressing, scroll
regions, alternate screen — behind a parser like `vte`, and a buffer that is
a grid rather than a list of lines. Deliberately last: it is a different
component wearing the same name, and nothing in the catalogue's install
recipes needs it. Do not start it to fix `sudo`; v0.2 covers that.

## 10. Risks, and what is verified at build time

- **pi RPC drift** — mitigated by the version floor, smoke test, raw-JSON
  fallback, and the pi-api-notes discipline.
- **gpui-kit 0.6.0** — pin exact; Linux platform deps documented at build;
  `ThemeColor` names verified then.
- **Envelope size** — views travel as widget lines; tables stay comfortably
  small (catalogue scale). The fallback-line requirement keeps large
  payloads from hiding data from text clients.
- **Session name in the picker** — stored by pi; the GUI shows it
  best-effort from the file header and falls back to id + date.