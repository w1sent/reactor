# extensions/

pi extensions, TypeScript. Auto-discovered by pi because this directory carries
a conventional name at the package root — see `docs/pi-api-notes.md` (and note
the `"pi": {}` trap recorded there).

Single-file extensions are `<name>.ts`; extensions needing npm dependencies are
`<name>/` with their own `package.json` and `src/index.ts`, and `npm install`
run in that directory.

**Every extension here shells out to `reactor … --format json`.** None of them
parses `tools.toml`, and none reimplements catalogue semantics —
[ADR-0005](../docs/adr/0005-reactor-cli-stdlib-python.md).

## Built

| Extension | Does |
|---|---|
| `tool-registry/` | Appends the registry block to the system prompt from `before_agent_start`; answers `resources_discover` with the skill directories of active, present tools; `ctx.ui.setStatus` shows `RE <present>/<catalogued>`; `/reactor [refresh\|show]`. |
| `selector/` | `/reactor-tools` — one overlay, two panes (Tab), fuzzy search, space to toggle, Enter to inspect, Ctrl+R to unpin. Every write is a `reactor tools\|toolsets …` call; `ctx.reload()` once on close if anything changed. |
| `status/` | `/reactor-status` — footer entry plus a toggleable panel above the editor, from `reactor services`. Refreshes on `session_start` and once per turn; no timer. |

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

## Tests

```bash
node --test "tests/extensions/*.test.mjs"
```

All three are driven through **pi's own loader** against the **real** CLI
— `pi.exec` is pi's, and the `reactor` it finds on `PATH` is a shim over
`bin/reactor` pointed at a fixture catalogue
([ADR-0012](../docs/adr/0012-extensions-tested-through-pi-s-own-loader.md)).
Only the host is faked: the context, its `ui`, and the TUI/theme/`done` triple
that `ctx.ui.custom` hands a component.

Two consequences for anyone adding a test. The theme is identity rather than
ANSI, because width is most of what is worth asserting about a list that must
not overflow. And `handleInput` launches its work with `void`, so a keystroke is
awaited via `press()`, which waits for the overlay to stop being busy rather
than guessing at a delay.

## Planned

| Extension | Milestone | Does |
|---|---|---|
| `scenario/` | 3 | Registers `reactor_step_complete`; its result is the next step's briefing. |

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
