# pi is the only target harness

REactor targets pi and nothing else. Claude Code, Codex, OpenCode and DeepAgents
are explicitly not targets, and portability to them is not a design constraint.

This reverses the position [ADR-0038 in the plugins
repo](https://github.com/w1sent/bn-plugins/blob/main/docs/adr/0038-binja-cli-skill-frontend.md) took for `bn`,
where CLI+Skill was chosen as the primary front end precisely *because* it
back-ports to other harnesses. That reasoning was correct for `bn` and remains
correct: `bn` is a tool, tools should work everywhere, and `bn` continues to.
REactor is not a tool. It is a harness integration, and a harness integration
that refuses to depend on its harness has given up most of what makes it worth
building.

## What this buys

The whole registry mechanism — probe the machine, inject what is present into
the system prompt, gate skill visibility on activation state, advance scenarios
by tool result — depends on pi's extension API. There is no portable equivalent.
Designing around a lowest common denominator that all four harnesses share would
mean shipping a skills-and-scripts collection with no dynamic behaviour at all,
which is a different and much weaker product.

Concretely, targeting pi alone lets REactor use, without hedging:

- `before_agent_start` returning a replacement system prompt
- `resources_discover` to decide which skills and prompts exist at all
- `ctx.reload()` to make a toggle take effect immediately
- `pi.registerTool` with the `details` field for out-of-context state
- `ctx.ui.custom` TUI components for the selector and status panels

## What it does not cost

The tools themselves stay portable. Every standalone tool REactor develops lives
in its own repository as a plain CLI
([ADR-0002](0002-package-ships-assets-tools-are-sibling-repos.md)), usable from
any shell, any harness, or no harness. The catalogue's contents are facts about
external programs, not pi-specific artefacts. Upstream skills fetched by REactor
([ADR-0008](0008-aggregate-upstream-skills.md)) are standard Agent Skills that
work in any harness implementing that convention.

So the pi-specific part is narrow: the extensions, and the glue that renders a
catalogue into a system prompt. If another harness becomes interesting, the tool
repositories are reused unchanged and a new *flavor* of REactor is built against
that harness's extension model. That is a rewrite of the thin layer, not of the
project.

## The state-location consequence

An earlier draft put REactor's state under XDG (`~/.config/reactor/`) so the CLI
would work identically outside pi. With pi as the only target that argument
evaporates, and splitting state across two roots is worse than the coupling it
avoided. Everything REactor owns at runtime lives under `~/.pi/reactor/`
([ADR-0003](0003-tools-toml-single-source-of-truth.md)).

The `reactor` CLI is still a CLI, and still does not require pi to be running —
that follows from pi's own CLI-over-MCP philosophy
([ADR-0005](0005-reactor-cli-stdlib-python.md)), not from portability.

## Considered and rejected

- **Harness-agnostic by construction** — rejected. It caps REactor at a static
  skills collection, because every dynamic behaviour it wants is a pi extension
  API with no counterpart elsewhere.
- **pi-first with a degraded fallback for other harnesses** — rejected as
  premature. Two code paths, one of them untested because nobody here runs the
  other harnesses, is worse than one path plus an honest scope statement.
- **A shared abstraction layer over several harnesses' extension APIs** —
  rejected outright. Designing that abstraction requires knowing two harnesses'
  APIs well; only one is in use, so the abstraction would be invented from a
  single example and be wrong.
