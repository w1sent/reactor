# reactor-gui — the native frontend

**Status: implemented.** The structural decisions are
[ADR-0031](../../docs/adr/0031-reactor-gui-lives-in-the-repo-and-installs-with-it.md)
(it lives in this repo) and
[ADR-0042](../../docs/adr/0042-the-gui-hosts-the-agent-in-process.md) (it hosts the
agent in process; supersedes the RPC design of ADR-0032).

reactor-gui is a Rust desktop application (gpui-kit) that runs a REactor
session in a native window: transcript, composer, session tree, catalogue, live
services, and the context budget.

```
┌─────────────────────────────────────────────────────────────────┐
│ reactor-gui (Rust, gpui-kit) — one process                       │
│  ┌──────────┐ ┌───────────────────────────┐ ┌────────────────┐  │
│  │ session  │ │ transcript                │ │ tools &        │  │
│  │ tree     │ │  markdown · thinking ·    │ │ toolsets       │  │
│  │ (from    │ │  tool cards · reductions  │ │ context        │  │
│  │ store)   │ │ ────────────────────────  │ │ ────────────── │  │
│  │          │ │ composer                  │ │ services (live)│  │
│  └──────────┘ └───────────────────────────┘ └────────────────┘  │
│  status bar: statuses · model · context %                        │
└─────────────────────────────────────────────────────────────────┘
   Backend ── reactor-agent (own tokio runtime) ── reactor-core
   panels ─── reactor-client ─────────────────── reactor-core
```

## 1. What it is, and is not

- **The frontend of the Rust harness.** One process: gpui on the main thread, the agent
  on its own tokio runtime, no child process.
- **A GUI for the person at the keyboard.** It does not change what the agent is told
  (the registry's job), does not add tools, does not parse `tools.toml` (ADR-0005: the
  catalogue comes from `reactor-core`).
- **No pi.** Nothing here starts or talks to pi; the pi flavor was removed from the tree
  (git history, last at `014a9b8`).

## 2. Transport: in process

`reactor-gui` links `reactor-agent`, `reactor-context` and `reactor-core`. `Backend`
(`src/backend.rs`) owns a multi-thread tokio runtime and the `Agent`; gpui's executor never
runs agent work.

- **Events** flow agent → GUI over a `std::sync::mpsc` channel (`UiEvent`), drained by a
  50 ms pump on the UI thread. Streaming deltas, tool progress, reductions, turn end, and
  context measurements all arrive this way.
- **Commands** flow GUI → agent by method call on `Backend` (`prompt`, `cancel`,
  `set_model`, `preview`, `reduce_now`, `restore`, `switch_branch`, …); results come back
  as `UiEvent`s. Turn cancellation is a `CancellationToken`.
- **The transcript is a pure function of the store's current branch** (`Session::rebuild`)
  plus live streaming buffers (`Session::apply`); ending a turn rebuilds from the store,
  so what is on screen and what is on disk cannot drift.
- **Models** come from `settings.models` (`provider/name` specs) and `/model provider/name`.
  There is no thinking-level picker (dropped in phase 5).
- The catalogue panels use `Client::for_session` — `reactor-core` in process, with the
  session's paths so activation edits land in the session's scope (§3).

## 3. Session model: REactor's own store

Sessions are `reactor-agent` stores (ADR-0036): `~/.reactor/sessions/<id>/session.jsonl`.
The GUI is bound to one session per window; branching moves the head within it.

| launch | effect |
|---|---|
| `reactor-gui <dir>` | new session in that cwd |
| bare `reactor-gui` | **workdir chooser**, then the above |
| `-c` / `--continue` | the newest session for the cwd |
| `-r` / `--resume` | **session picker** over the store, then that session |
| `--session <dir>` | that session |
| `--model provider/name` | start on that model |

- **Session-scoped activation** (ADR-0042). Tool/toolset toggles write
  `<session dir>/activation.json`, and the panels show whether the session overrides the
  machine default. *make default* promotes the session's state to `state.json`;
  *inherit* removes the override.
- **Context settings** cascade the same way (ADR-0038): built-in → `~/.reactor/settings.json`
  → the session. The Context panel shows each value's origin, previews a reduction, runs
  one now, lists reductions in force with *undo*, and can make the session's values the
  default. Dimmed transcript rows are hidden from the model by a reduction.
- **Tree.** Read from the store; *Continue from here* switches the head to a final
  assistant reply.
- One attached frontend per session directory, by contract (no locking).

## 4. Extension views — none

The GUI has no plugin surface: every panel is native and reads REactor's own state.
(The former extension-view envelope, and the `/guide` walkthrough built on it, were
retired with pi — ADR-0042.)

## 5. Data panels: the catalogue, through a seam

Side panels (catalogue, toolsets, services) come from `reactor-core`, asked
through a Rust trait whose payloads are the `reactor` CLI's `--format json`
contract:

```rust
trait ReactorClient {
    async fn tools(&self) -> Result<ToolsPayload>;
    async fn toolsets(&self) -> Result<ToolsetRows>;
    async fn services(&self, refresh: bool) -> Result<ServicesPayload>;
    // …mirrors the CLI's JSON contract, command for command
}
```

- **`LibClient` (default)** — calls `reactor-core` in-process (ADR-0034): no
  process per panel refresh. Core's reports reach the payload structs through
  their serialized form, so the GUI has one description of each payload — the
  wire — whichever path filled it. No `--refresh` by default, so the shared
  TTLs and `cache.json` decide freshness (ADR-0014 — no second probe policy).
- **`CliClient` (debug fallback)** — shells out to `reactor` (std::process
  with a watchdog timeout of 20s). Selected with
  `REACTOR_GUI_CLIENT=cli`. If the two ever disagree that is a bug at the
  library boundary; `crates/reactor-cli/tests/agree.rs` runs both side by side
  over every method — reads in both probe orders, writes to identical config
  dirs (state.json compared byte for byte), errors, project-scoped state.
- The GUI holds a `Client` enum over the two (it must be `Clone` to move into
  background tasks). The GUI never parses `tools.toml`.

## 6. The window

**Chrome** (`chrome.rs`): the app requests client-side decorations and draws
gpui-kit's `TitleBar` itself, with the Layout menu inside it via `AppMenuBar`
on Linux/Windows (macOS keeps its native bar). gpui draws neither on its own:
GNOME's Wayland session offers no server-side decorations, and `set_menus` is a
native bar on macOS only — elsewhere it merely records the menus.

Single window, gpui-kit (`gpui-kit = "0.6.0"` pinned), `DockArea` with a
serialized layout. **Ayu Dark theme by default** (§7), Lucide icons
(gpui-kit's bundled default).

- **Center**: transcript + composer.
  - Transcript: `Session::rebuild` of the store's current branch, plus live
    streaming buffers. Assistant text renders as markdown; thinking blocks
    render collapsed; tool calls are cards (name, args, streamed output,
    result, error state). Reductions render as cards with *undo*; rows a
    reduction hides from the model are dimmed.
  - Composer: multi-line text area. `Enter` sends (or queues a follow-up while
    the agent works); `Shift+Enter` newline; `Esc` interrupts; an explicit
    Interrupt button cancels the turn. `/`-commands (`COMMANDS` in `app.rs`) run
    in the app. The model picker is in the status bar; context % is measured from
    the request the agent would send next.
- **Left** (collapsible): the session tree, read from the store. *Continue from
  here* switches the head to a final assistant reply, disabled while the agent
  works.
- **Right** (collapsible, tabs): **Tools**, **Toolsets** (each its own tab, with
  enable/disable actions and the session-scope header, §3) and **Context** (§3);
  a bottom slot for **Services** (live rows, polled on a TTL-respecting timer).
  Every side panel shows a loading indicator while its refresh is in flight,
  never a bare "no data" before the first answer arrives.
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
  the agent-event pump uses — no second executor (§2's rule), just one per panel
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
- **Bottom**: status bar, under the dock. Left: statuses (manifest, scenario, identity) from
  `status_items`. Right: model picker · context %.
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
  and a mapping from that vocabulary to gpui-kit's tokens. Exact gpui-kit
  `ThemeColor` enum names are verified against 0.6.0 at build time and the
  table lives in this spec's implementation notes.
- Light theme: skeleton behind a flag, marked experimental in v0.1; the token
  set is theme-agnostic from day one so future themes are data, not code.
- Icons: Lucide, gpui-kit's default set.

## 8. Repository, install, tests

- The GUI's crates, `crates/reactor-client` (`ReactorClient`, `LibClient`,
  `CliClient`) and `crates/reactor-gui` (the application), are members of the root
  cargo workspace (ADR-0031). The root `default-members` leave `reactor-gui` out —
  its native windowing stack (fontconfig, xkbcommon, …) would make a bare
  `cargo test` fail on a machine without it — so build it with `-p reactor-gui`.
- Install: `cargo install --git https://github.com/w1sent/reactor reactor-gui`,
  or `cargo install --path crates/reactor-gui` from a checkout.
- GUI-local config (recent workdirs) lives in `~/.reactor/gui.json` (a legacy
  `~/.pi/reactor-gui.json` is read once if the new file is absent).
- Tests: hermetic — `backend` (tree rows, context view, session listing) and `session` (store → transcript) test without a window; `reactor-client` against the real CLI's JSON contracts.
  `reactor-client`'s `LibClient` is tested against the real binary by
  `crates/reactor-cli/tests/agree.rs` (§5).

## 9. Scope

**v0.1** — transcript, composer (follow-up/interrupt), right dock
(tools/toolsets/context + services), the bottom console with catalogue installs,
status bar, command palette, the session tree panel with branch switching, workdir
chooser + session picker, Ayu Dark theme. Phase 5 moved it onto the in-process agent.

**v0.2** — **shipped: a pty behind the console, with minimal escape
handling** (`portable-pty`, plus `console::AnsiReader` for colour and the
line-rewriting a progress bar does; everything else consumed, not printed),
**the layout system** (§6.1), a merged input/output console with selectable
text, and multiple independent console panels open at once. Still open: HTML export, session labels +
clone, steer gesture, "open session in new window" launcher, light theme,
and persisting a hand-arranged layout
across launches (`DockAreaState` serializes; nothing writes it yet).

**v0.3** — **full terminal emulation**, if and only if the console is ever
asked to host a program that draws a screen (`vim`, `htop`, a curses
installer). That means a real vt100 grid — cursor addressing, scroll
regions, alternate screen — behind a parser like `vte`, and a buffer that is
a grid rather than a list of lines. Deliberately last: it is a different
component wearing the same name, and nothing in the catalogue's install
recipes needs it. Do not start it to fix `sudo`; v0.2 covers that.

## 10. Risks, and what is verified at build time

- **gpui-kit 0.6.0** — pin exact; Linux platform deps documented at build;
  `ThemeColor` names verified then.
