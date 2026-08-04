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

It renders nothing itself: the block arrives pre-rendered in
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

## Planned

| Extension | Milestone | Does |
|---|---|---|
| `status/` | 2 | Live service, device and connectivity state in the footer and a panel. |
| `scenario/` | 3 | Registers `reactor_step_complete`; its result is the next step's briefing. |

## Rules

- Take `theme` from the render callback; never import it globally.
- Call `tui.requestRender()` after state changes in input handlers.
- Never emit a line wider than the `width` passed to `render`.
- Degrade cleanly when there is no terminal — the registry matters in `print`
  and `json` modes, only the TUI surfaces need one. Gate an overlay on
  `ctx.mode === "tui"`, not on `ctx.hasUI`: `hasUI` is also true for RPC, where
  `ctx.ui.custom` has nothing to mount into.
- Use `StringEnum` from `@earendil-works/pi-ai` for enum tool parameters;
  `Type.Union` breaks on Google's APIs.
