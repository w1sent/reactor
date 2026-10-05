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

## 3. Session model: sessions of a case

A session is a `reactor-agent` store in the **case** of the working directory
(ADR-0045, ADR-0046): rows in `<workdir>/.reactor/analysis.db`. A folder that is not a
case yet gets an **unsaved state** under `~/.reactor/unsaved/`, with the same layout;
nothing is written into the folder until it is saved as a case. The GUI is bound to one
session per window; branching moves the head within it, and a case holds many sessions.

| launch | effect |
|---|---|
| `reactor-gui <dir>` | new session in that folder's case, or in its unsaved state |
| bare `reactor-gui` | **workdir chooser**, then the above |
| `-c` / `--continue` | the newest session of the folder's case |
| `-r` / `--resume` | **session picker** over the case's sessions, then that session |
| `--session <id>` | that session of the folder's case |
| `--model provider/name` | start on that model |
| `case …`, `db …` | headless commands: no window is opened (`src/headless.rs`) |

- **Saving a case.** *Case › Save as Case…* in the menu (palette: `save-case`). A case can
  only be moved while nothing has it open, so the window closes and a new one opens on
  the same session, saving on its way in (`--save-case`). Refused while a turn runs.
- **Closing the window while a session works** asks first, before the question about an
  unsaved state: *Keep working*, or *Interrupt and close*, which stops every session that is
  working, waits for them to settle (a few seconds at most) and then closes as it would have.
  What they did stays in their logs. Both the platform's close request and the title bar's
  own close button go through this. A session that is working stands out in the Session Tree's
  list: both of its lines shimmer, its whole row breathes (a wash of the accent colour that
  swells and fades), and it is tagged *working*.
- **Closing an unsaved state** asks: *Save as Case*, *Delete*, or *Cancel*. The answer is
  carried out by a helper (`reactor-gui case save|discard <dir> --wait`) once the window
  has let go. An unsaved state in which nothing was said is deleted without asking. Both
  the platform's close request and the title bar's own close button go through this.
- **Memory switches** (ADR-0047) sit above the prompt in the transcript panel: *Other
  sessions* and *Case graph*. A click sets this session's layer; the default is
  `memory` in `settings.json`.
- **Session-scoped activation** (ADR-0042). Tool/toolset toggles write
  `<session dir>/activation.json`, and the panels show whether the session overrides the
  machine default. *make default* promotes the session's state to `state.json`;
  *inherit* removes the override.
- **Context settings** cascade the same way (ADR-0038): built-in → `~/.reactor/settings.json`
  → the session. The Context panel shows each value's origin, previews a reduction, runs
  one now, lists reductions in force with *undo*, and can make the session's values the
  default. Dimmed transcript rows are hidden from the model by a reduction.
- **The Context panel** has the usage bar pinned above two tabs. The bar is the next request
  split by *kind* (system prompt, manifest frame, scenario and reporting, REactor and MCP tool
  definitions, you, agent replies, thinking, tool calls, tool results, reductions), drawn in the
  theme's chart colours; *Details* opens a legend whose rows open that kind in the context
  window. **Plan**: the goal, guidelines and steps (*Edit…* opens the editor, *Frame…* the frame,
  the Manifest switch pauses it), the scenario stepper, and the Reporting switch and level.
  **Budget**: the context settings and reduction. The frame has two tabs, *As sent* (the manifest
  block, verbatim, with its cost) and *State* (what `/frame` prints). `/frame` opens it, and
  `/goal` or `/guidelines` without a text open the editor. The manifest's and reporting's
  remaining settings are Settings › Manifest and › Reporting.
- **Session ids.** A session is numbered within its case: `1`, `2`, `3` … (the number after the
  highest in the case; a gap is not filled; if another process takes it first the next is
  asked). Inside the case `#3` is enough — that is how titles, the status bar and the list
  show it — and across cases it is `<case id>#3` (in the tooltips; the case id is in
  `case.json`). `--session` takes `3`, `#3` or `<case id>#3`. A session made before numbering
  keeps its id (a time and a process id, shown by its last part) and is not counted.
- **Sessions are not tied to the window.** Any number of the case's sessions are open at once,
  each in a **transcript tab** of its own (titled `Transcript · <id>`, with a dot on the
  active one). Opening a session never closes anything: it adds a tab, or brings the tab
  forward if the session is already open (putting it back if it was closed). A session
  belongs to one window (its lock file), so one open in another window is refused.
- **The active session** is the one the rest of the window is about: the Session Tree, the
  Context panel, the status bar, the memory switches, `/` commands, the graph panel's edits.
  It is the session whose transcript was last in focus — focus anywhere in a tab, its prompt
  or a click, makes it active — and **it stays so when focus is somewhere else** (the Files
  panel, say): the last one in focus or opened is the active one. Which one it is, is shown:
  a `session <id>` item in the status bar (the full id in its tooltip), `Context · <id>`,
  `Tree of session <id>`, and the dot on its tab.
- **Sessions of the case** — the top of the Session Tree panel: newest first, what was first
  said in it, when it was started (local time), its entries, its id, and whether it is
  *active*, *open* here or *open elsewhere* (greyed). Sessions in which nothing was said are
  left out (opening a case starts one), except those open here. A click picks one; a **double
  click**, or *Open*, shows its transcript; *Close* lets go of the picked one (its tab goes, its
  lock is released; it stays in the case); any session can be closed, the window's first too
  (another then takes its place as the one the layouts show), except the last one and one that
  is working; **+** starts a new session in a tab of its own. Closing a tab only hides it: the
  session stays open, and keeps working. The transcript's toolbar has a second button, *Close
  session*, next to the tab's close: it does what *Close* does for that session. The refresh icon reads the list again;
  it is also read at start and after every turn. The row's details are tooltips.
- Each open session has its own events: its pump, its transcript model, its prompt box
  and tree/context state; they share the case's database handle and the embedding model.
  Save as Case is refused while any open session is working.
- **Tree.** Read from the store; *Continue from here* switches the head to a final
  assistant reply.
- One attached frontend per session, held by a lock file in the case's `run/`. A second
  window on the same session is refused; a second window on the same *case* is fine.

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
    streaming buffers. Assistant text renders as markdown, with a copy icon at
    its top right that puts the text *as written* (the Markdown source, not what is
    drawn) on the clipboard and shows a tick for a moment; thinking blocks
    render collapsed; tool calls are cards (name, args, streamed output,
    result, error state). Reductions render as cards with *undo*; rows a
    reduction hides from the model are dimmed.
  - Composer: multi-line text area. `Enter` sends (or, while the agent works,
    *schedules* the message to run right after the current prompt: each scheduled message shows as a
    marker at the foot of the transcript, and `↑` in the empty prompt brings the latest one back into
    the input to change and send again); `Shift+Enter` newline; `Esc` interrupts; an explicit
    Interrupt button cancels the turn. `↑` with the cursor on the prompt's top line puts the previous message you sent into the input (the text that was there is kept), and `↓` on the bottom line moves forward again, ending on that text; anywhere else the arrows just move the cursor. So an interrupted message is one `↑` away, ready to edit and send again. `/`-commands (`COMMANDS` in `app.rs`) run
    in the app. The model picker is in the status bar; context % is measured from
    the request the agent would send next.
- **Left** (collapsible): the session tree, read from the store. *Continue from
  here* switches the head to a final assistant reply, disabled while the agent
  works.
- **Right** (collapsible, tabs): **Tools**, **Toolsets** (each its own tab, with
  enable/disable actions and the session-scope header, §3; each with a search box above its
  list — every word typed must be in an entry's name, description or tags (a toolset's tools),
  and a search looks through the not-installed tools too, so one to install can be found) and
  **Context** (§3);
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

### 6.0 Explaining without clutter

The window carries data and controls, not prose. Anything that *explains* — what a bar means, how
a setting is changed, where something is saved — is a **tooltip** on the element it is about
(`hints::tip`), or, where there is nothing to hover (a heading, a whole popup), an **(i) button**
that opens the text when clicked (`hints::info_button`). Empty states, errors and confirmations are
feedback, not commentary, and stay as short text.

### 6.0d Timing a prompt

While a prompt runs, the marker at the foot of the transcript also shows how long it has been
running and how many tokens it has streamed so far: the provider's own count for each finished model
round, and four characters to a token for the round still streaming. When the prompt finishes, a line
under its result says when it finished, how long it took, how many tokens it streamed and at what rate
(`finished 14:03:12 · took 1m 12s · 850 tokens · 34.0 tokens/s`; the date is added for another day).
The rate is measured over the time spent streaming, so a long tool run does not make the model look
slow. The line is kept in the session log (`turn_stats`), so a resumed session shows it too. A prompt
that was interrupted or failed is not annotated.

### 6.0c Notifications

Anything the app tells the person — a command's result, a failed save, a model error — is a
notification (`ReactorApp::push_note`), shown twice:

- a **popup** in the top-right corner, under the title bar — gpui-kit's notification component, so its
  look, stacking, animation and close button are the library's. It goes after a timeout (*Settings →
  Behaviour*, 6 s by default) or when closed with its ×. The component's own auto-hide is a fixed 5 s,
  so popups are pushed without it and taken down by `ReactorApp::flush_toasts` at the configured time.
  The settings window shows the same popups;
- a line in the **history** behind the **bell** at the right-hand end of the title bar. The bell
  shows how many notifications have not been looked at; clicking it opens the list, newest first, with
  the time of each, and *Clear* empties it. `Esc` or a click outside closes it. `/notifications`
  opens it too. The component has no history, so this part is ours.

The history rules (unread count, cap of 200, dates for other days) are `src/notifications.rs`, tested
without a window; `src/notices_ui.rs` draws the bell and the list.

### 6.0b The context window

*Show the context window* — the Context panel's button, `/inspect`, or the palette — opens a popup
with the request the model would be sent next, taken apart. It is `reactor-agent`'s own
`context_preview`, the pieces the loop sends, so it shows what the model gets. Each piece carries
its **source**: the base prompt, identity, tool registry, skills, manifest, reporting, the current
scenario phase, the tool definitions, then the messages — yours, the model's, tool results,
reduction stand-ins and any reporting reminder. The chips above the list total the estimated tokens
per source and filter by it; the (i) beside a piece says where it comes from and how to change it.
*Copy all* puts the whole request on the clipboard. The popup is gpui-kit's dialog (the library draws the backdrop, handles focus and closes on Esc or a click outside).

### 6.0 Commands: the palette and the `/` popup

Every user-facing feature is a **slash command** — `ReactorApp::run_command` is the one
place that executes them — and two views list them (`src/palette.rs`):

- **The command palette** — `Ctrl+P` / `Cmd+P`, or *Commands → Command Palette…* in the
  app menu. It is gpui-kit's command component hosted in gpui-kit's dialog (the library draws the backdrop, handles focus and closes on Esc or a click outside). It lists the commands *and* what the session's state makes possible: a line per
  model (`Model: switch to …`), per tool (`Tool: enable adb`, `Tool: install frida`), toolset,
  scenario, identity and reduction in force, plus layouts, docks, context settings and so on.
  Choosing an entry runs it; one that needs arguments (`/goal`, `/tool`, …) puts `/name ` in
  the composer instead.
- **The `/` completion popup** — typing `/` in the prompt lists the bare commands above the
  input. `↑`/`↓` move, `Enter` or `Tab` take the highlighted one (a command that needs nothing
  more runs at once, the others are completed and wait for their arguments), `Esc` closes it.
  **Once a command's name and a space are typed, the popup offers that command's parameters**
  (`/identity ` the identities and the words it takes; `/model ` the models; `/layout `, `/dock `,
  `/panel `, `/preview `, `/reduce `, `/undo `, `/report `, `/manifest `, `/reactor-scenario `,
  `/context `, `/activation `, `/embedding `, `/tool `, `/toolset `, `/install `): typing narrows them
  (typos forgiven, the ones used most first among equals); a parameter that ends the command
  runs it, one that is followed by another argument (`/tool <id>` takes `on` or `off`) completes
  the text and offers the next. The palette does the same when its query is a command's name
  and a space. Taking a completion puts **the cursor at the beginning of what was completed**:
  the start of the command, or of the parameter.

**Ranking** is the same for both. An entry's score is the number of characters of the query it
matches — the longest common subsequence of what was typed and the entry's text, inside a
window no wider than twice the query — so a missing, extra, wrong or swapped letter costs one
match rather than the whole entry (`raduce` finds `reduce`). The query may lose one character in
four and still match. Entries are ordered by score; equal scores by how often the person has
run them (`command_usage` in `~/.reactor/gui.json`); equal frequency by the shorter text, then
alphabetically. With nothing typed, frequency alone orders the list.

### 6.0a Settings

*Commands → Settings…*, `Ctrl+,` / `Cmd+,` or `/settings` opens the settings in **a window of their
own** (`src/settings_window.rs`), built on gpui-kit's `Settings` component: a page list on the left
with a search box, the page's fields on the right, and a *reset* on each page for what has been
changed. Changes apply at once and are saved; there is no OK button. The window reads and changes
the main window's state, follows it when it changes, and shows the same notifications.

- **Fonts** — a family (an installed font's name) and a size in pixels for *Interface* and
  *Monospace* (the theme's base fonts, which every component reads), *Transcript*, *Tool calls and
  output*, *Prompt input* and *Console*. Unset means the theme's own. Saved in `~/.reactor/gui.json`.
- **Behaviour** — open thinking blocks by default; dim what a reduction took out of the model's view;
  how much tool output the transcript shows (the session keeps it all); how long notification popups
  stay; the layout a window opens with.
- **Identities** — the working personas: the built-in ones and the user's saved ones, each a row
  with its name, whether it is built in or saved, the start of its text (all of it in the
  tooltip), and whether it is the default for new sessions or in use now. *Use* selects it for
  this session (`/identity <name>`); *Edit* opens the **editor window** (a name and the text; a
  rename keeps its place; a name cannot be empty, have a space, be built in or be a word
  `/identity` uses); *View* on a built-in opens the editor on a copy to save under a name of
  your own; *Remove* asks twice. *New identity…* opens the editor blank, and a dropdown picks
  the default. The page opens from `/identity manage` and *Identities: manage…* in the palette;
  `/identity new [name]` and `/identity edit <name>` (and *Identity: new…* / *Identity: edit
  <name>* in the palette) open the editor directly. `/identity show [name]` (the one in use, with no
  name) opens its text in a **popup** that scrolls and whose text can be selected, with *Open in
  the editor*; a notification is for a line, not a page. The editor's text box has a height of its
  own (what the window leaves) and scrolls inside it, so a long identity never pushes the buttons
  out of reach. Saved identities are in `settings.json`.
- **Model** — the model new sessions start on (`defaultModel` in `settings.json`), chosen from the
  picker's list; a provider (anthropic, openai, gemini, openrouter, ollama; the page says whether its
  key variable is set), a model name, and *Set as default* / *Use in this session* / *Add to picker
  list*; the picker's list itself; *Make its model the default* for the session's model. API keys are
  read from the environment, never stored.
- Where a page chooses a provider (Model, Documents) the providers are a row of **tabs**, the
  current one selected, not a dropdown: every choice is in sight and a click makes it.
- **Documents** (ADR-0046) — the embedding model that lets the agent search imported documents by
  meaning (`embedding.model` in `settings.json`; none by default, and until one is chosen search is by
  keyword). The page shows the current model, how many documents and passages the case has, and each
  model's progress. While vectors are being made, a bar and "N of M passages · P%" show how far
  the run has got, live (the page is drawn again as each batch is stored). *Embed now* resumes an
  interrupted run; **Re-embed everything** forgets the chosen model's vectors and makes them all
  again (after documents were stored differently, or a model changed under the same name) — it asks
  for a second press first, because every passage goes to the provider again and the documents
  cannot be searched by meaning until it has finished; it stops a run that is going and starts
  afresh. *Turn off* returns to keyword search.
  Choosing: a provider (openai, gemini, ollama; the page says whether its key variable is set, and that
  with Ollama the text stays on this machine and with the others it is sent to the provider), a model
  name, *Use this model*. Changing the model re-embeds in the background and search keeps the earlier
  vectors until the new set is complete. Importing is `/import` or *Case › Import Documents…*.
- **Context** — the agent's context budget: reduction mode, the window in tokens (REactor does not ask
  the provider for it: set it for your model), the reserve for the reply, when a reduction starts and
  how far it goes (percentages of the usable window), and the summarizer model. A dropdown chooses
  the layer edited: the **default** (`settings.json`, for new sessions) or **this session only**. A
  field that is not set there shows what the session uses now; *reset* clears it, so a session falls
  back to the default and the default to the built-in value.
- **Tools** — *Make session's the default* and *Use the default* for the session's tool activation.

Fonts and behaviour are not per session: they are saved for every session as they change. The pages
that can differ per session (Model, Context, Tools) have their own *make the session's … the default*.

### 6.0b The context window

*Show the context window* — the Context panel's button, `/inspect`, or the palette — opens a popup
with the request the model would be sent next, taken apart. It is `reactor-agent`'s own
`context_preview`, the pieces the loop sends, so it shows what the model gets. Each piece carries
its **source**: the base prompt, identity, tool registry, skills, manifest, reporting, the current
scenario phase, the tool definitions, then the messages — yours, the model's, tool results,
reduction stand-ins and any reporting reminder. The chips above the list total the estimated tokens
per source and filter by it; the (i) beside a piece says where it comes from and how to change it.
*Copy all* puts the whole request on the clipboard.

### 6.0 Commands: the palette and the `/` popup

Every user-facing feature is a **slash command** — `ReactorApp::run_command` is the one
place that executes them — and two views list them (`src/palette.rs`):

- **The command palette** — `Ctrl+P` / `Cmd+P`, or *Commands → Command Palette…* in the
  app menu. It lists the commands *and* what the session's state makes possible: a line per
  model (`Model: switch to …`), per tool (`Tool: enable adb`, `Tool: install frida`), toolset,
  scenario, identity and reduction in force, plus layouts, docks, context settings and so on.
  Choosing an entry runs it; one that needs arguments (`/goal`, `/tool`, …) puts `/name ` in
  the composer instead.
- **The `/` completion popup** — typing `/` in the prompt lists the bare commands above the
  input. `↑`/`↓` move, `Enter` or `Tab` take the highlighted one (a command that needs nothing
  more runs at once, the others are completed and wait for their arguments), `Esc` closes it.
  **Once a command's name and a space are typed, the popup offers that command's parameters**
  (`/identity ` the identities and the words it takes; `/model ` the models; `/layout `, `/dock `,
  `/panel `, `/preview `, `/reduce `, `/undo `, `/report `, `/manifest `, `/reactor-scenario `,
  `/context `, `/activation `, `/embedding `, `/tool `, `/toolset `, `/install `): typing narrows them
  (typos forgiven, the ones used most first among equals); a parameter that ends the command
  runs it, one that is followed by another argument (`/tool <id>` takes `on` or `off`) completes
  the text and offers the next. The palette does the same when its query is a command's name
  and a space. Taking a completion puts **the cursor at the beginning of what was completed**:
  the start of the command, or of the parameter.

**Ranking** is the same for both. An entry's score is the number of characters of the query it
matches — the longest common subsequence of what was typed and the entry's text, inside a
window no wider than twice the query — so a missing, extra, wrong or swapped letter costs one
match rather than the whole entry (`raduce` finds `reduce`). The query may lose one character in
four and still match. Entries are ordered by score; equal scores by how often the person has
run them (`command_usage` in `~/.reactor/gui.json`); equal frequency by the shorter text, then
alphabetically. With nothing typed, frequency alone orders the list.

### 6.0a Settings

*Commands → Settings…*, `Ctrl+,` / `Cmd+,` or `/settings` opens a popup (`src/settings_ui.rs`).
Changes apply at once and are saved to `ui` in `~/.reactor/gui.json`; unset means the theme's own,
so a fresh install looks as it always did and *reset* clears a field.

- **Fonts** — a family (an installed font's name, checked when you press Enter or leave the
  field) and a size in pixels, for: *Interface* and *Monospace* (the theme's base fonts, which
  every component reads), *Transcript*, *Tool calls and output*, *Prompt input* and *Console*.
  Tool and toolset names and the `/` popup use the monospace font at its own size.
- **Behaviour** — open thinking blocks by default; dim what a reduction took out of the model's
  view; how much tool output the transcript shows (the session keeps it all); the layout a window
  opens with.

- **Model** — the model new sessions start on (`defaultModel` in `settings.json`): pick a provider
  (anthropic, openai, gemini, openrouter, ollama; the tab says whether its key variable is set),
  type a model name, then *Set as default*, *Use in this session* or *Add to picker list*. The
  picker's list (`models`) is editable here too. API keys are read from the environment, never stored.

- **Context** — the agent's context budget: reduction mode, the window in tokens (REactor does not ask
  the provider for it: set it for your model), the reserve for the reply, when a reduction starts and
  how far it goes (percentages of the usable window), and the summarizer model. A switch chooses the
  layer edited: the **default** (`settings.json`, for new sessions) or **this session only**. An empty
  field inherits, and *Use the defaults* / *Reset to built-in defaults* clear the layer — a session
  back to the default, the default back to the built-in values.

- **Tools** — whether this session has its own tool activation, with *Make session activation the
  default* and *Use the default activation*.

The popup has one tab per kind: Fonts, Behaviour, Model, Context, Tools. Each tab has its own reset
and, where a session can differ from the default (Model, Context, Tools), its own *make this session's
… the default* — a button acts on its tab only. Fonts and behaviour are not per session: they are
saved for every session as they change. The context budget is separate: the
Context panel and `/context`.

### 6.1 Layout: nothing is fixed furniture

Every panel is draggable between docks and droppable onto another panel's
tab bar — gpui-kit's dock does that once an area holds more than one tab
group, so the console can sit wherever the transcript can, and any two
panels can be stacked into one tabbed group. That "more than one tab group"
clause is load-bearing, not incidental: gpui-base's
`TabGroupContext::draggable` is `!is_locked() && !is_alone()`, and
`is_alone()` means *no sibling group in the same split* — a dock or centre
holding one lone tab group is undraggable no matter what the panel itself
allows. Every area of a preset is one tab group, and gpui-base denies a lone group
both dragging and closing through its tab bar — so closing does not go through
the tab group: it removes the panel from the `DockArea` directly
(`layout::Panels::remove`).

**Every panel can be closed and reopened**: from the Layout menu's *Panels*
submenu, from the palette (`Panels: <name>`, i.e. `/panel <name>`), and by the
close button at the end of each panel's own header toolbar. A closed panel comes
back where the active preset keeps it (or its default dock, if the preset does
not place it), opening that dock if it was collapsed.

The GUI's own contribution on top of that freedom is the way back:

- **Named presets** (`reactor-gui/src/layout.rs`) — Catalogue puts a different
  panel in the centre, the plainest statement that the centre is not owned by
  the transcript:
  - **Default** — transcript in the centre; tools, toolsets, context,
    services, files and the session tree as tabs on the right; the console below.
  - **Focus** — the transcript with only the console below.
  - **Analysis** — transcript in the centre, files and the tree on the left, tools,
    toolsets, context and services on the right, the console below.
  - **Catalogue** — tools in the centre, services on the left, toolsets on
    the right, the console below.
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

**SQLite files in the table viewer.** A file that starts with SQLite's magic bytes opens in
the table viewer whatever it is called (`files::suggest`), with its tables and views
picked from the toolbar (`Table: …`, with row counts) and read through
`reactor_db::browse`: opened read-only, journal mode untouched, nothing written, at most
100,000 rows per table (the footer says when it stopped). That is how the agent's
`reactor_data.sqlite` — and the case's own `.reactor/analysis.db` — are looked at. It needs
the file on local disk.

**Context documents** (ADR-0046). *Case › Import Documents…* or `/import [files, folders or
URLs…]` (none named: the system's file dialog, files and folders both) converts, cuts and
stores them in the background and says what happened as a notice: how many, how many
were already there, and which failed and why. Repeating an import adds only what is new.
`/embedding` shows the corpus and its vectors; `/embedding provider/name` chooses the
model (remembered as the default in `settings.json`, and said to send the documents'
text to that provider), `/embedding off` returns to keyword search, `/embedding now`
resumes an interrupted run. While vectors are being made the status bar says
`embedding n/m`. The same work is available headless: `reactor-gui ingest`, `embed` and
`case rebuild-indexes`.

**Handing over** has no window controls; it is headless (ADR-0045): `reactor-gui case export
<dir> <file.zip> [--password P]` writes the whole case as one AES-256 ZIP (password `infected`
unless given), `case import <file.zip> <dir>` unpacks it into an empty folder, and `graph
export|import|check` take the graph to and from STIX 2.1, CSV and graph libraries.

### 6.3 The graph panel

ADR-0047. `src/graph/model.rs` turns the stored graph into what is drawn and lays it out
(pure, tested without a window); `src/graph/panel.rs` draws it. It shares the centre with
the transcript in the Default and Analysis presets (`PanelKind::Graph`, `/panel graph`).

- **What is drawn.** Entities are boxes. A claim that relates exactly two entities, and
  that nothing else refers to, is one labeled line between them, with an arrow where the
  roles give a direction (`src → dst`, `actor → target`, `query → answer`, …). Several
  claims of one type between the same pair are one line (`net.resolution ×2`). A claim
  with more participants, or one an annotation is about, stays a node with a line per
  role — a node in the middle means "this relation is being discussed". Annotations are
  boxes with dashed lines to what they are about. Sources are not drawn; they are in the
  detail. Disputed is the warning colour; soft-deleted is hidden unless the eye is on.
- **Layout.** Each connected group is laid out on its own (force-directed) and the groups are then
  packed close together in rows, wider than tall, so that a node connected to nothing, or a
  small cluster, sits beside the big one and not far off. A new node with no line goes in rows
  below what is there. Force-directed, reproducible. What already has a place keeps it: only new
  nodes are placed, near what they connect to. A dragged node stays where it is put.
  *Auto-layout* in the toolbar arranges everything afresh and fits it into view, giving
  up the positions that were dragged.
  Positions live for the window's lifetime; they are not stored yet.
- **Moving about.** Drag the background to pan; the wheel zooms about the pointer
  (4 % to 300 %); *fit* in the toolbar brings everything into view. **Zoom scales
  everything**: node boxes, text, line widths and arrowheads shrink with it, so zooming
  out is a bigger picture and not the same crowd further apart. Below a readable text size
  a node is an empty box and its tooltip says what it is; below 60 % a relation's label
  is a dot on the line (click it to select the relation, hover for its words). Every
  node has a tooltip with its type and full text.
- **Added things are seen.** Whatever is added by hand goes where it can be seen: beside
  what is selected, else near the middle of the view, and the view pans (and zooms to at
  least 50 %) if it would be out of sight. *Right-click* on the plane opens the add form
  at that spot and the node lands *exactly* there; the form switches between *Entity*
  (type, with known types offered, and value) and *Note* (free text, about nothing in
  particular).
- **Selecting.** Click a node or a line's label. Ctrl/Cmd+click or Shift+click picks a
  node up or puts it down; Shift+drag on the plane sweeps an area; dragging any selected
  node moves them all. With several selected, the detail box lists what they are and the
  buttons below act on all of them: *Tag*/*Untag*, *Add note* (one note about all of
  them), *Confidence*, *Dispute*, *Restore*, *Soft-delete*, and **Connect…**: two nodes
  get a relation with a role each; more are the members of one claim (a node in the
  middle, a line to each). With one selected, the **inspector** is docked at the panel's left,
  beside the graph rather than over it (the divider is dragged, 300 to 900 px, kept while the
  window is open; it stays open while the plane is clicked or dragged, and only its X, or
  selecting nothing by a sweep, closes it), and shows the element in one structure whatever its kind
  (`graph/inspector.rs` is the data, `graph/panel/inspector_ui.rs` draws it). The header says
  what it is (a coloured tag), its title, who wrote which version and when, how sure it is (a
  row of bars over the confidence scale, filled up to its step; a click on a bar sets that step
  and a click on the step it is at clears it) and, for a hypothesis, where it stands as a
  segmented *Open / Supported / Rejected*. Tabs: **Text** (only for a long text, rendered as
  Markdown, with *Copy*, *Read* and *Edit*), **Evidence** (*Relations* for other kinds): what it
  is related to, grouped by what it says — supports and refutes first, then about, mentioned by,
  other roles — a click selects that element; **History**: every version and what it changed;
  **Details**: its type, when it happened or held, properties, id. The actions stay pinned below.
  A text is the title when it is one short line; a long or multi-line one has a title of its own
  (its first line) and is the Text tab. *Read* and *Edit* open a popup with the text large, an
  outline of its headings and the evidence at its side; saving writes a new version and the old
  text stays in the history. An entity's value is its identity and is not edited. The agent is
  told to write Markdown only for a long text; a person may write either.
- **Editing.** All of it goes through the write path the agent uses and is recorded as
  the analyst's (`reactor_db::graph::edit`, tested without a window). Toolbar: **+** adds
  an entity, a note or another annotation (the same value is always the same node). The add
  form's buttons choose: *Entity*; *Note* (free text); *Hypothesis* (open until supported or
  rejected); *Assessment* (about what is selected, pinned to the version it judged; with a
  hypothesis selected too, it *supports* or *refutes* it); *Cluster* (the selected nodes
  belong together); *Same as* (the two selected entities are one thing); *Searched* (a search that
  found nothing: what for and where). These are the agent's annotation kinds, written
  through the same path (`edit::annotate`). **Type fields offer the
  known types as they are typed** — those it begins, then a part of a dotted name
  (`ipv4` finds `net.ipv4`), then containing it, then by alias; most-used first — as
  chips under the field; a click takes one, its tooltip says how much it is used. In the detail box, for what is
  selected: *Confidence* (a menu of the calibrated scale), *Dispute* and *Restore*
  (status), *Hypothesis* (open, supported, rejected — only on a hypothesis), *Tag* and
  *Untag*, *Add note*, *Soft-delete*, and — for a single element — *Undo last change*,
  which previews what would go, whose it is, and is refused if something builds on it.
  *Relate to…* on a node: click the other end, then name the relation's type and the two
  roles; it is recorded as an observation sourced to the current session entry, so a
  relation a person states is as traceable as one pulled from a table. Explanations are
  tooltips and (i) buttons (the one at the panel's top left lists the gestures), per the
  design rule in `hints.rs`.
- **Ways to arrange it.** A menu in the panel's header (the current way is its label, marked in
  the list) picks how the whole graph is arranged, and applies it at once; *auto-layout* in the
  toolbar then repeats the chosen way. **Force-directed** is the default (above).
  **Hierarchical** (Sugiyama-style): lines made to run one way (a cycle is broken once), nodes in
  layers from the sources down, each layer ordered by sweeps to the average place of its
  neighbours, and placed along the layer pulled towards them. **Circular / radial**: the
  best-connected node in the middle, a ring for each step away from it, each node in a slice of
  the circle as wide as the nodes under it need, a ring as far out as its crowding makes it.
  **Orthogonal**: the hierarchical arrangement snapped onto a grid, and lines drawn as
  right-angled connectors (out of the facing side, two corners, into the facing side; over the
  top between two in a row). All of them lay each connected group out alone and pack the groups
  close together, and are reproducible. They arrange everything afresh; what is added later is
  placed beside what it connects to, as in the default. These three leave room for the widest
  node (a note, 220 px) with 90 px between neighbours and between layers, 100 px between groups;
  the default's tighter spacing is unchanged. The chosen way is kept while the window
  is open, not stored. Hierarchical has no dummy nodes on lines that skip a layer, so such a
  line is straight across the layers between.
- **Positions are kept.** Where the nodes are is stored with the case (`graph_layout`: no versions,
  no change log, carried by a case export): a node dragged, added or placed keeps its place
  across restarts and across sessions; *auto-layout* gives them all up and stores the new ones.
- **How much is drawn (progressive disclosure).** A *Detail* menu in the panel's header picks one
  of the concept's three levels, each with all of the one before it. **Basic**: entities, labeled
  lines and notes, with no status or confidence (the plain picture). **Analysis**: and status
  (disputed in the warning colour), confidence and who wrote it, hypotheses and what supports or
  refutes them. **Full**: and claims as nodes with a line for each role. **Auto** (the default)
  follows the zoom: below 45 % Basic, below 90 % Analysis, else Full, so a large graph fitted
  into the view is its overview and zooming in brings the detail. What a level leaves out is
  only not drawn — the filter, the detail box and the agent still have it — and the count reads
  "N of M nodes". A claim that is a node only because something is said about it still shows
  as the plain line between the two things it relates, standing for the claim (clicking it
  selects the claim), so that less detail never hides *that two things are related*; a note
  or hypothesis about it attaches to the end the line starts from; a claim with more than two
  participants is detail and goes with its node. Sources are not drawn at any level.
- **Filtering.** A box in the panel's top left filters the nodes as typed: words that must all be
  in the label or type; `type:net.` (a type that starts so, or has a dotted part that does),
  `tag:phase.`, `by:agent` / `by:person`, `kind:entity|claim|note`, `status:conflicting`;
  a leading `-` negates. Lines are drawn between nodes that are shown, and the count reads
  "N of M nodes". The syntax is behind an (i).
- **Tag hulls.** The *tags* toolbar button draws an outline, in the tag's own colour, round the shown
  nodes that share a tag (two or more; the twelve most shared), under the nodes, with the tag's
  name and count above its highest point. The outline is the convex hull of the nodes' boxes
  standing 46 px off them, with rounded corners (`model::padded_hull`): it follows the shape of
  the group, a capsule for two, and does not take in the empty corners a box would. Outlines of
  different tags overlap where a node has several tags. Zoomed out the name is the tooltip. The
  hull of a group is convex, so it can still cover a node that is not in the group; bubble sets,
  which bend round such nodes, are not built.
- **Finding the source.** *Source* in the detail box opens where the selected claim came from:
  a page in the browser, a file of the case in its viewer (at the line, if the locator is
  `line=42`, `L42` or `:42`), or the session it was said in (`session:<id>#entry=<n>`, which opens
  that session's transcript). An entity has none of its own, and says so.
- **Refreshing.** After every tool call and every turn, and with the toolbar button.
- *Not built yet:* bubble sets (hulls that bend round other nodes), sources as icons,
  adding an annotation other than a note by hand.

### 6.4 The documents panel

`src/docs_panel.rs` (`PanelKind::Docs`, `/panel docs`; in the Analysis preset's centre, closed
in the others). The case's reference documents, searched by hand. It runs the same search
as the agent's `docs_search` tool (`reactor_agent::embed::search_by`), so what is seen here
is what the agent would have been given.

- **Searching.** A box (Enter or *Search*). Where a search by meaning can be made — a model
  is chosen and has a vector for every passage — a choice of *Both* (fused, the default),
  *Keyword* (the words as written) and *Meaning*. Where it cannot there is no choice, and an
  (i) says why. The same rule decides what the agent is told: its tool has a `mode`
  parameter only then, and no word about meaning otherwise.
- **Empty passages.** A passage with only headings or code-fence lines is never listed (the store joins such passages to the next one; this also covers cases stored before that).
- **Results.** The list and the passage are side by side with a draggable divider between them (the list starts at 380 px, 160 px at least; the width is kept while the window is open, not stored). Up to 30 passages, best first: the document's title (or its source), the
  section, a snippet around the first match with the query's words marked, and the ranks
  (K for keyword, M for meaning; the fused score is in the tooltip). The status line says how
  the search ran.
- **Inspecting.** The selected passage in full, with the query's words marked, and the
  document, source, section, kind and metadata. *Open source* opens the file in its viewer
  (or a page in the browser), *Copy* copies the passage, *Around it* shows the two passages
  before and after it in the document. With no documents the panel offers *Import…*.
- **Closeness.** A result found by meaning shows, after its ranks, its similarity (cosine, 1 is
  identical, larger is closer) — in the list and as *Meaning* in the detail; the order is by a score
  made of the ranks, not by this. Settings › Documents › **Meaning threshold** (0 to 1, 0 is none;
  `embedding.minSimilarity` in `settings.json`) drops the passages whose similarity is under it from
  a search by meaning, the agent's and this panel's, and is read at each search, so a change applies
  at once. The status line and the agent's result say when a threshold was in force.
- **Order.** Results are listed *by relevance*: the order the search ranked them, which fuses the
  keyword and meaning ranks, so a passage ranked high by its words can come before one that is
  closer in meaning. Where results have a similarity, *Similarity* (beside the status line) lists
  them closest first, those found by words only last; the selection stays on the same passage.
- **Switch.** The agent's use of the documents is the *Documents* switch above the prompt in the
  transcript panel, with *Other sessions* and *Case graph* (`memory.documents`, same cascade: the
  setting, then this session's layer; on by default). Off withdraws `docs_search` from the session
  and refuses a call that still comes; this panel keeps working, since it is the person's own search.
- **Marking.** The query's words are marked in the snippets and the passage (substring match,
  ignoring ASCII case; it is not what the index matched, only what was typed). A match by
  meaning is between whole passages, so words say little about it: **Why** (offered only
  where meaning can be searched) splits the selected passage into sentences, embeds them
  with the query in one request to the chosen model and marks the closest (up to three, within
  0.08 cosine of the best) in a second colour, with their similarities in a line above. It
  sends the passage's text to the provider, so it is asked for per switch-on and not done
  on its own; the sentences are those of `reactor_agent::embed::explain`.
- **From the transcript.** A finished `docs_search` call is not shown as the tagged text the
  model received. The transcript parses it (`reactor_agent::tools::docs::parse_result`, kept
  next to the code that writes it and tested to round-trip) and shows the summary line
  (how many, for what, by which means) and one line for each of the best five matches:
  number, document, section, the start of the passage with the query's words as the anchor.
  The tooltip of a line has its source and more of the text. A line opens the whole result
  in this panel with that passage selected; so does *Open in Documents* (*All N in
  Documents* when there are more than five). The panel looks the passages up in the case
  again (`corpus::hits_at`), so it shows the stored text with its neighbours and source;
  nothing is searched or embedded again, and a passage whose document was removed since is
  left out. An error or "no passage matches" is shown as plain text, as before.
- The panel reads the counts and whether meaning is available again after an import and when
  the vectors are complete. The search runs on the agent's runtime: the query may be embedded
  by a provider over the network.

### 6.2 The filesystem panels

ADR-0044. `crates/reactor-fs` is the seam (no gpui); `reactor-gui/src/files/` the panels.

- **Files** (`explorer.rs`, `PanelKind::Files`, in the Default and Analysis presets): a lazy
  tree of the working directory (the root row is the directory's name; folders are read
  when first expanded; *Refresh* reads the open ones again). Search is a box with two
  modes: *Names* (substring of the relative path, or a glob when it has `*`/`?`) and
  *Contents* (a regular expression, ripgrep-style: binary files skipped). Both walk
  through the `Vfs` on the background executor; a result opens its file, and a content hit
  opens it at the line. The context menu: Open, Open in each viewer, Preview in popup,
  Open in default program, Open with… (the OS program picker), Show in file manager, Copy path / Copy full path, Properties, Hash
  (MD5, SHA-1, SHA-256, CRC-32, or all), New file/folder, Rename, Delete (confirmed),
  Refresh. Operations the filesystem cannot do are greyed out.
- **Properties** (`props.rs`, a popup): name, path, size, times, mode, owner, link target and
  the filesystem's own extras; what the file *is* (`file(1)`'s description, MIME type);
  hashes computed in one pass in the background, each with a copy button.
- **Text editor** (`editor.rs`): gpui-kit's code editor, every tree-sitter grammar bundled.
  The language comes from the file name, then a `#!` line (`language.rs`), and the status
  bar's picker overrides it. Line numbers, folding, search (the editor's own), Ctrl/Cmd+S,
  a dirty marker in the tab and a confirmation before closing or reloading over edits.
  Read-only when the `Vfs` is. A file that is not UTF-8 offers the hex viewer instead.
- **Table viewer** (`table.rs`, model in `table_model.rs`): delimited text in gpui-kit's data
  table. The delimiter comes from the extension, else the most consistent of comma, tab,
  semicolon and pipe, and can be chosen. Sorting by header click (numbers as numbers), a
  filter box over every cell, and a user-chosen number of header rows (their cells, joined,
  name the columns).
- **Data viewer** (`data.rs`, `data_model.rs`, `plist.rs`): files with a better form than their
  bytes or their text, chosen by what is in them. **Images** (PNG, JPEG, GIF, WebP, BMP, TIFF, ICO,
  SVG) with fit and zoom. **Markdown** (`.md`, `.markdown`, `.mdown`, `.mkd`; known by its name,
  since it is plain text), *Rendered* (headings, lists, code, tables, links — a web link opens in
  the browser) or *Source*. **Property lists**, known by content: Apple's binary `bplist00`, and XML
  `<plist>` documents, as a **tree** (two levels open at first; click a row to pick it, the chevron
  or a double click opens and closes a container; *Open all* / *Close all*; strings, integers,
  reals, booleans, dates as UTC times, data as its size and first bytes, UIDs, sets, dictionaries
  in file order) or as **XML** text; *Copy* puts what is picked, or the whole list, on the clipboard
  as XML. Picking a field shows where it is (`list › 3 › name`) and what can be taken out of it:
  *Copy value* (a string as it is, a number or date as written, data as hex, a structure as JSON),
  *Copy key*, *JSON* (data as `{"$data": base64}`, a UID as `{"$uid": n}`), *XML*, and for data
  *Hex*, *Base64* and *Save…* (the bytes to a file you name). **Data that is itself a property
  list** — binary or XML, up to six levels down — is read and is a child of the data
  (`↳ property list`), with its size and what it holds on the data's row. **Keyed archives**
  (`NSKeyedArchiver`) are recognised by their `$archiver`, `$top` and `$objects`, and shown as the
  objects they hold, with the UIDs followed: arrays, sets, dictionaries, strings, data, dates, URLs and
  UUIDs of Foundation put back as themselves; any other class as a dictionary with its `$class`
  and fields; `$null` as null; an object met again inside itself as a pointer, not for ever. *Objects* and *Raw* switch between that and the table as written. A
  keyed archive inside data in a list is shown as its objects. The reader is bounded (depth, objects, every offset checked against the file), so a
  damaged or hostile list is an error message, not a hang. The file explorer opens `.md` files,
  binary property lists and `.plist` XML in this viewer by default; the text editor and the hex
  viewer are one choice away in the context menu. One view, hosted as a panel or in a popup
  (`/look <path>`, or *Preview in popup*).
- **Hex viewer** (`hex.rs`): offset, bytes and text, scrolled lazily with a page cache, so
  size does not matter. Select by dragging in either column (Shift-click extends, Ctrl/Cmd+A
  selects all); the other column highlights the same bytes. Ctrl/Cmd+C, or the *Copy hex* /
  *Copy text* buttons, copies the selection (by default as the column it was made in).
  Search for text or hex bytes: every match is marked, the current one in a stronger
  colour, and the status line says *match N of M*; Enter and the arrows step through them,
  wrapping.
- **Closing.** Every panel closes with the X in its header or a middle-click on its tab label; an editor with unsaved edits asks first.
- **Opening.** `/open [edit|table|data|hex] <path>`; the viewer is otherwise chosen by
  `files::suggest`. Viewers are tabs in the centre; any number can be open.
- **References in the transcript** (`refs.rs`): file names in the assistant's text that exist
  are links. Click: the suited viewer (at the line, for `path:line`); Shift: hex; Ctrl/Cmd:
  editor; Alt: popup. Tool commands and output are not linked.

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
