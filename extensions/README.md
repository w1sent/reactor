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

## Planned

| Extension | Milestone | Does |
|---|---|---|
| `tool-registry/` | 1 | Probes, caches, renders the registry into the system prompt via `before_agent_start`; answers `resources_discover` to gate skills and prompts on activation state; one-line `ctx.ui.setStatus`. |
| `selector/` | 2 | Search, inspect and toggle tools and toolsets. `ctx.ui.custom` + `SelectList`, `ctx.reload()` after a toggle. |
| `status/` | 2 | Live service, device and connectivity state in the footer and a panel. |
| `scenario/` | 3 | Registers `reactor_step_complete`; its result is the next step's briefing. |

## Rules

- Take `theme` from the render callback; never import it globally.
- Call `tui.requestRender()` after state changes in input handlers.
- Never emit a line wider than the `width` passed to `render`.
- Degrade cleanly when `ctx.hasUI` is false — the registry matters in `print`
  and `json` modes, only the TUI surfaces need a terminal.
- Use `StringEnum` from `@earendil-works/pi-ai` for enum tool parameters;
  `Type.Union` breaks on Google's APIs.
