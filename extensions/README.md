# extensions/

pi extensions, TypeScript. Auto-discovered by pi because this directory carries
a conventional name at the package root — see `docs/pi-api-notes.md` (and note
the `"pi": {}` trap recorded there).

Single-file extensions are `<name>.ts`; extensions needing npm dependencies are
`<name>/` with their own `package.json` and `src/index.ts`, and `npm install`
run in that directory.

**Every RE-tool extension here shells out to `reactor … --format json`.** None
of them parses `tools.toml`, and none reimplements catalogue semantics —
[ADR-0005](../docs/adr/0005-reactor-cli-stdlib-python.md). `rolling-context/`
is the one exception: it is a general-purpose context-management extension,
unrelated to the catalogue and switched independently of everything else here
([ADR-0019](../docs/adr/0019-rolling-context-ships-here-general-purpose.md)).

## Built

| Extension | Does |
|---|---|
| `tool-registry/` | Appends the registry block to the system prompt from `before_agent_start`; answers `resources_discover` with the skill directories of active, present tools; `ctx.ui.setStatus` shows `RE <present>/<catalogued>`; `/reactor [refresh\|show]`; `/reactor-toolbox [on\|off]`. Registers nothing else when `toolbox: false` (ADR-0016). |
| `selector/` | `/reactor-tools` — one overlay, two panes (Tab), fuzzy search (any key filters; `/` resets it), space to toggle, Enter to inspect, Ctrl+R to unpin. Every write is a `reactor tools\|toolsets …` call; `ctx.reload()` once on close if anything changed. Registers nothing at all when `toolbox: false` (ADR-0016). |
| `status/` | `/reactor-status [refresh\|hide\|mute <id>\|unmute <id>]` — footer entry plus a toggleable panel above the editor, from `reactor services`. Refreshes on `session_start` and once per turn; no timer. `hiddenServices` (ADR-0016) omits muted catalogue ids from both. |
| `scenario/` | `reactor_step_complete(summary)` — a tool the LLM calls; its own return content is the next step's briefing. `/reactor-scenario [list\|start <id>\|status\|next [summary]\|stop]` — the human's view of the same state, and the manual override. Steps are Markdown files under `prompts/scenarios/<id>/`, read directly (ADR-0017). |
| `reporting/` | Off by default; `/report on\|off\|level <0\|1\|2>\|status\|folder <path>\|reset` opts a session in. Appends a "document as you go" block to the system prompt; levels 1–2 track tool-call "steps" since the reporting folder last changed on disk (a size/mtime snapshot diff, not tool-call inspection) and escalate — level 1 nags every LLM call once a threshold is crossed, level 2 reverts the ignored turn and re-demands the prompt, up to `maxReverts` times, before falling back to nagging (ADR-0023). Footer entry `reporting mode`/`· low`/`· strict`. |
| `rolling-context/` | Off by default; `/rolling [on\|off]` opts a session in. Instead of pi's summarization compaction, keeps a small manifest (goal + agent-maintained steps) at the front of every prompt and fades everything else out of the *next* `context` call once it stops fitting a configurable budget — the session file itself is untouched. Measures and cuts the same way pi's own compaction does, never overflowing the real window (ADR-0020). `/goal`, `/guidelines`, `/frame`; `update_steps`, `history_index`/`_search`/`_read` tools. General-purpose, not catalogue-aware (ADR-0019). |
| `context-editor/` | `/context-editor` (landscape overlay: toggle which entries are visible) and `/context-editor manual` (same entries as a text file, opened in `$VISUAL`/`$EDITOR`/`nano`). Either way, ends by asking whether the edit forks a new session (default) or filters the current one going forward — pi's session store is append-only, so those are the two real mechanisms, not a preference (ADR-0021). Independent of `rolling-context/` and the toolbox; general-purpose. |

## The toolbox toggle and hidden services (ADR-0016)

`tool-registry/` and `selector/` are, together, "the toolbox": what the agent
is told about and the UI for curating it. Both read the same file,
`<agent dir>/reactor.json` (normally `~/.pi/agent/reactor.json`, next to pi's
own `settings.json` — not inside it, since `Settings` has no extension point
for a third party's fields):

```json
{
  "toolbox": false,
  "hiddenServices": ["adb"]
}
```

- **`toolbox: false`** removes `tool-registry/` and `selector/` from the
  session as if neither were loaded — no `/reactor` or `/reactor-tools`
  command, no status-line entry, no system-prompt injection, no
  `resources_discover` answer. Checked once at registration, before any event
  exists to react to the file changing.
- **`hiddenServices`** (`status/` only, independent of `toolbox`) is a list of
  catalogue ids to leave out of the footer and the panel — `bn` and `adb` are
  the two that currently declare a service probe. Read fresh on every refresh.

Both default to "everything on" when the file is absent, unreadable, or has a
field of the wrong shape.

**Toggle them from inside pi with a command, not by hand-editing the file**
(pi's own `/settings` is a closed, hardcoded component with no extension
point to add rows to — this is the closest equivalent):

```
/reactor-toolbox           # reports on or off
/reactor-toolbox off       # writes reactor.json, then ctx.reload()s at once
/reactor-toolbox on

/reactor-status mute adb   # writes reactor.json, repaints an open panel at once
/reactor-status unmute adb
```

`/reactor-toolbox` is registered **unconditionally**, in `tool-registry/`,
before that extension's own `toolbox: false` gate — otherwise turning the
toolbox off would remove the only command able to turn it back on. It calls
`ctx.reload()` after writing, which re-runs every extension's factory
function, so the flip is visible in the same session. `mute`/`unmute` need no
reload: `status/` already re-reads `hiddenServices` on every refresh, so the
command just triggers one. Only a *hand* edit of `reactor.json` (outside
either command) needs a manual `/reload` to be picked up.

`tool-registry` renders nothing itself: the block arrives pre-rendered in
`reactor registry --format json`, so the byte-stability the prompt cache depends
on is tested once, in Python
([ADR-0006](../docs/adr/0006-registry-injected-into-system-prompt.md)).

Two behaviours worth knowing before editing it:

- **A failed probe keeps the last good block.** An unreachable CLI is not
  evidence the tools vanished, and an emptied registry would tell the agent
  something false.
- **`pi.exec` resolves rather than throwing**, including on ENOENT — a missing
  `reactor` looks exactly like a crashed one (code 1, empty stdout). Both are
  handled as "unavailable", warned once, then silent.

And two for the selector:

- **It renders its own list rather than using `SettingsList`.** The reasons are
  specific and are recorded in
  [ADR-0011](../docs/adr/0011-selector-edits-overrides-not-outcomes.md) — search
  over descriptions, a key for inspect that is not Enter, and a column for
  presence.
- **A toggle's response is the new state.** `reactor tools enable …` returns the
  recomputed `active` list and the whole `state`, so the overlay applies that
  rather than re-deriving anything or re-listing the catalogue.

And two for the status panel:

- **It shares nothing with `tool-registry` but `cache.json`**, which both reach
  through the CLI ([ADR-0014](../docs/adr/0014-extensions-share-the-cache-not-each-other.md)).
  There is no shared module between extensions and there cannot be a useful
  one: pi loads each extension with its own jiti instance, so an import common
  to two of them is instantiated twice and the two copies drift.
- **It refreshes per turn, never on a timer.** The state that matters is the
  state at the moment the agent acts, which is turn time; a background probe
  would cost a process spawn per tick to be wrong slightly less often.

And three for scenarios:

- **`reactor_step_complete` is always registered**, whether or not a scenario
  is running — calling it with none active is an answered case ("start one
  with `/reactor-scenario start <id>`"), not an error. `pi.setActiveTools()`
  could hide it between scenarios; rejected as touching a shared,
  extension-wide list for one line in the system prompt
  ([ADR-0017](../docs/adr/0017-scenario-steps-are-read-directly-not-pi-prompts.md)).
- **Steps are read with `node:fs`, not through `resources_discover`.** A raw
  step file is not a useful thing to invoke on its own — advancing is
  stateful, and a bare pi prompt command has no memory of which step came
  before it. `REACTOR_SCENARIOS_DIR` overrides where they are read from,
  mirroring `REACTOR_CONFIG_DIR`.
- **Activating a step's toolset never deactivates the previous one.** Steers,
  does not restrict ([ADR-0007](../docs/adr/0007-deactivation-is-soft.md)) —
  narrowing what is advertised mid-scenario is not this extension's call to
  make.

And two for rolling-context:

- **The manifest is the only permanent memory.** Everything else the fade
  drops is still in the session file, never in the model's next prompt,
  unless the agent copies it into the steps via `update_steps` first —
  recovery via the history tools is one-shot, since a tool result fades like
  any other recent message.
- **`GLOBAL_CONFIG_PATH` reads through `getAgentDir()`, not a hand-rolled
  `homedir() + ".pi/agent"`.** The one behaviour change this port makes: the
  standalone draft ignored `PI_CODING_AGENT_DIR` when set, silently reading
  the wrong file. Same default path either way.
- **The fade measures and cuts in pi's own units, not its own.** It calls
  pi's exported `estimateTokens(message)` against the live `event.messages`
  array directly, using the same never-start-on-a-`toolResult` rule pi's own
  `findCutPoint` encodes, rather than a separately-serialized approximation
  of a differently-counted array. A hard ceiling (`window - reserve`) below
  the soft fade budget archives (truncates) the newest turn's content rather
  than ever send more than that, however small that leaves it
  ([ADR-0020](../docs/adr/0020-rolling-context-measures-and-cuts-like-pi-does.md)).

And one for context-editor:

- **Fork replays messages; current-branch persists a filter — never a
  rewrite.** `SessionManager` is append-only, so there is no third option.
  Both close over tool-call/tool-result pairs the same way rolling-context's
  cut point does, so hiding one half never orphans the other
  ([ADR-0021](../docs/adr/0021-context-editor-forks-or-filters-never-rewrites.md)).

And three for reporting:

- **Detection is a filesystem probe, not tool-call inspection.** A recursive
  size/mtime snapshot of the reporting folder, diffed on every
  `tool_execution_end`, catches a `write`/`edit` tool call, a `bash`
  redirect, `git checkout`, or a hand edit in another window identically —
  nothing here is special-cased to one tool, unlike the `tool_call`-watching
  design this replaced during review. Same "ask the machine" stance
  `status/` takes for services, not the command-line-guessing heuristic
  [ADR-0007](../docs/adr/0007-deactivation-is-soft.md) rejected — see
  [ADR-0023](../docs/adr/0023-reporting-enforcement-is-a-filesystem-probe-not-a-heuristic.md)
  for why that's a different case, not an exception to it.
- **Level 2's revert triggers from `agent_settled`, never `turn_end`.**
  `navigateTree` throws while the agent is still streaming, and `turn_end`
  fires mid-loop where that is very often still true; `agent_settled` is the
  first point pi itself guarantees it is safe
  (`docs/pi-api-notes.md`). Getting there from a plain event handler at all
  depends on a second fact recorded in the same file:
  `pi.sendUserMessage("/reactor-report-enforce", { expandPromptTemplates:
  true })` dispatches a real `ExtensionCommandContext` — the only way this
  extension reaches `navigateTree`, which ordinary event handlers are not
  given. No other extension here self-dispatches a command like this.
- **`maxReverts` is a hard stop, not a suggestion.** After that many
  consecutive reverts the extension falls back to level-1-style nagging
  (already running underneath, since level 2 is level 1 plus reverts) and
  notifies the user once, so a model that will not comply can never leave a
  session reverting forever.

## Tests

```bash
node --test "tests/extensions/*.test.mjs"
```

All seven are driven through **pi's own loader**
([ADR-0012](../docs/adr/0012-extensions-tested-through-pi-s-own-loader.md)).
For the four RE-tool extensions that means the **real** CLI too — `pi.exec`
is pi's, and the `reactor` it finds on `PATH` is a shim over `bin/reactor`
pointed at a fixture catalogue; `rolling-context/`, `context-editor/` and
`reporting/` never call `reactor` at all, so their tests exercise pi's own
history/session/context APIs (or, for `reporting/`, real files under a
temporary `ctx.cwd`) instead. Only the host is faked: the context,
its `ui`, and the TUI/theme/`done` triple that `ctx.ui.custom` hands a
component.

Two consequences for anyone adding a test. The theme is identity rather than
ANSI, because width is most of what is worth asserting about a list that must
not overflow. And `handleInput` launches its work with `void`, so a keystroke is
awaited via `press()`, which waits for the overlay to stop being busy rather
than guessing at a delay.

`scenario.test.mjs` adds one more: `REACTOR_SCENARIOS_DIR` isolates the state
machine's tests from this package's own shipped `prompts/scenarios/triage/`,
the same way `REACTOR_CONFIG_DIR` isolates everything else from
`~/.pi/reactor/`. One test runs that real shipped scenario end to end and
checks its *shape* — four steps, each with its own title — rather than its
exact prose, the way `TestShippedConfig` does for the catalogue in the Python
suite.

`rolling-context.test.mjs` needs `ctx.sessionManager.getBranch()` to return
actual conversation messages, not just custom entries — the first extension
here that does. `makeContext`'s `branch` option seeds it; `model` and
`getSystemPrompt()` were added alongside it, both otherwise unused by
anything else in this repo.

`context-editor.test.mjs` needs two more fakes `makeContext` grew for it:
`ui.select` answers from a `selectAnswers` queue (empty = the dialog was
dismissed), and `newSession` runs its `setup` callback against a minimal
recorder exposing only `appendMessage`, logging what got appended for a test
to assert on. Neither models pi's real tree or compaction machinery — that
stays pi's own code, exercised by `scripts/check-in-pi.mjs`'s real process,
not by this mock.

`reporting.test.mjs` needs two more fakes: `runtime.sendUserMessage` (a
recorder alongside `sendMessage`/`appendEntry`, since `loadExtension()`
otherwise leaves it a throwing stub) and `ctx.navigateTree` (a recorder that
returns `{cancelled: false}`, the same shape `newSession` already fakes).
Both only *record the call* — neither reproduces pi's real command dispatch
or session-tree navigation, so level 2's actual self-dispatch chain
(`agent_settled` → `pi.sendUserMessage("/reactor-report-enforce", ...)` →
`_tryExecuteExtensionCommand` building a real command context) is exercised
only by `scripts/check-in-pi.mjs` against a real process, same as the
`ctx.reload()` staleness class of bug below. `folderPath(ctx)` resolves
against `fixture.dir`, which is a real temporary directory, so folder-change
detection is tested by actually writing files there rather than faking
`fs`.

`ctx`/`pi` here are still mocks, though — `harness.mjs`'s optional `guard`
(see the Rules below) only catches a stale-ctx-after-`reload()` bug once a
test knows to wire it in; `newSession` (and `fork`/`switchSession`) set the
same `guard.stale` `reload()` does, for the same reason. `scripts/check-in-pi.mjs`
is the check that needs no simulation: it drives a **real** `pi --mode rpc`
process with these same extension files loaded for real, and watches for
`extension_error`. Run it after touching anything that calls `ctx.reload()`
and its siblings.

## Rules

- Take `theme` from the render callback; never import it globally.
- Call `tui.requestRender()` after state changes in input handlers.
- Never emit a line wider than the `width` passed to `render`.
- Degrade cleanly when there is no terminal — the registry matters in `print`
  and `json` modes, only the TUI surfaces need one. Gate an overlay or a widget
  on `ctx.mode === "tui"`, not on `ctx.hasUI`: `hasUI` is also true for RPC,
  where `ctx.ui.custom` and `ctx.ui.setWidget` have nothing to draw into.
- Extensions do not talk to each other. Shared state goes through the CLI and
  `cache.json` ([ADR-0014](../docs/adr/0014-extensions-share-the-cache-not-each-other.md));
  a shared TypeScript module gives shared *code* and two copies of its state.
- Use `StringEnum` from `@earendil-works/pi-ai` for enum tool parameters;
  `Type.Union` breaks on Google's APIs.
- `await ctx.reload()` (also `newSession`/`fork`/`switchSession`) invalidates
  the `ctx` — and the closed-over `pi` — that called it: pi throws on any
  further use of either. Make the reload the *last* line of a handler; do
  every `ctx`/`pi` action first (`docs/pi-api-notes.md`). Pass the same
  `guard` to `loadExtension()` and `makeContext()` in a test that exercises
  reload, so a reordering mistake fails the test instead of passing silently.
