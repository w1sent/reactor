# TODO

Design is settled (`docs/adr/`). Milestones 1, 2 and 3 are built and tested.
The milestones were ordered by dependency — the spine is a chain where each
link needed the one before it — and what is left of each is deferred to real
use rather than another round of building. Deferred ideas are deliberately not
tracked here: what moves anything forward is real use, not planning, and this
file records what is built.

## Milestone 1 — Spine — **done**

The smallest thing that delivers the core value: the agent knows what is on this
machine. No custom TUI components.

- **Catalogue schema** — settled. Install keys are package managers, ranked by a
  user-editable `[platform].prefer`
  ([ADR-0010](docs/adr/0010-install-recipes-keyed-by-package-manager.md));
  detection is `binary` or `python_module`; service probes report state plus an
  optional regex line count and nothing else.
- **`reactor` CLI** — `bin/reactor`, one stdlib-only file. Surface in
  `bin/README.md`; `--format json` everywhere and pinned by test. `install all`
  targets the whole catalogue in one call.
- **`scripts/install.py`** — symlink, seed without clobbering, `state.json`,
  fetch upstream skills, install bash/zsh/fish completions
  ([ADR-0015](docs/adr/0015-shell-completion-generated-not-hand-written.md)).
  Verified end to end against the real `bn-plugins` and `ipsw-skill`
  repositories.
- **`extensions/tool-registry/`** — block into the system prompt on
  `before_agent_start`, `skillPaths` on `resources_discover`, status line,
  `/reactor`. Tested through pi's own loader against the real CLI
  ([ADR-0012](docs/adr/0012-extensions-tested-through-pi-s-own-loader.md)).
- **Determinism** — a test, not an intention: `TestRegistryDeterminism` and
  `TestJsonContract.test_registry_block_is_stable_across_processes`.

The catalogue gaps this milestone originally tracked (QBDI, otool, a Binary
Ninja headless capability flag), the absence of any REactor-authored skill,
and `requires:` being inert were all deferred — none of them block the spine
being done, and none of them have an answer that doesn't want more real usage
first.

## Milestone 2 — Selector and status — **done**

### `extensions/selector/` — built

`/reactor-tools`: one overlay, two panes switched with Tab, fuzzy search over
id, name, description and tags (any key filters; `/` is the conventional
shortcut for it and resets the query rather than being typed into it), space
to toggle, Enter to inspect, Ctrl+R to
drop an override, Esc to close. `ctx.reload()` fires once on close if anything
was written. Inspected detail and skill bodies go in as out-of-context session
entries, because they are for the human and the model already has the block.

It turned out not to be "pure TUI over the CLI's JSON" after all. A selector
invites toggling, and `enable`/`disable` used to pin unconditionally, so idle
keystrokes would accrue overrides that quietly outrank every later toolset
change. Activation edits are now minimal and `reactor tools reset` exists
([ADR-0011](docs/adr/0011-selector-edits-overrides-not-outcomes.md)); `tools
list` gained an `override` field so a client can tell "on because of a toolset"
from "on because you said so".

### `extensions/status/` — built

`/reactor-status`: a footer entry, and a panel above the editor that toggles.
Both come from `reactor services`, a command added for it — the question a
status line asks is "what is up", not "what is installed", and probing only the
service-capable entries is what makes it cheap enough to ask every turn.

The sharing question is settled and turned out to have a forced answer
([ADR-0014](docs/adr/0014-extensions-share-the-cache-not-each-other.md)): pi
loads every extension with its own jiti instance, so a shared module holds
shared *code* and two copies of its state. Extensions share `cache.json`
through the CLI and nothing else.

### Toolset definitions — done

Thirteen sets, every one of them a task rather than an axis, and every tool in
at least one besides `all` — asserted, so a new catalogue entry cannot be
shipped without someone deciding where it lives.

### The toolbox toggle and `hiddenServices` — done

`toolbox: false` in pi's own agent directory (`reactor.json`, next to
`settings.json` but not inside it) removes `tool-registry/` and `selector/`
from a session as if neither were loaded; `hiddenServices` does the same for
individual services in `status/`
([ADR-0016](docs/adr/0016-extension-toggles-live-in-their-own-pi-side-file.md)).
Both are reachable from inside pi — `/reactor-toolbox [on|off]`,
`/reactor-status mute|unmute <id>` — since pi's own `/settings` has no
extension point to add a row to.

What was left open against the selector, the status panel, and the toolset
definitions was deferred rather than tracked.

## Milestone 3 — Scenarios — **done**

`extensions/scenario/`: `reactor_step_complete(summary)` is a tool the LLM
calls, and the tool's own return content is the next step's briefing — no
extra message, no extra turn boundary
([ADR-0009](docs/adr/0009-scenarios-advance-by-tool-result.md)). Registered
once, always, not only while a scenario runs, so it costs one line in
"Available tools" rather than touching the extension-wide active-tools list;
calling it with nothing active is an answered case, not an error.

`/reactor-scenario list|start <id>|status|next [summary]|stop` is the human's
view of the same state and the manual override the ADR calls for — spelled
`/reactor-scenario next` rather than the ADR's literal `/reactor next`,
because `/reactor` is one of the commands `toolbox: false` removes and the
override has to survive that.

Step definitions are Markdown files under `prompts/scenarios/<id>/`, one per
step, ordered by filename, with a two-field frontmatter (`title`, optional
`toolset`) hand-parsed rather than pulling in YAML. Read directly with
`node:fs`, not surfaced as pi prompt commands — resolves the "where do
scenario definitions live" question this file used to leave open
([ADR-0017](docs/adr/0017-scenario-steps-are-read-directly-not-pi-prompts.md)).
`REACTOR_SCENARIOS_DIR` overrides the directory, mirroring
`REACTOR_CONFIG_DIR`.

State — which step, what each completed step reported — persists through
`pi.appendEntry` and is restored on `session_start` by taking the last
matching entry, so it survives a `/reload` or a resumed session. A step names
a toolset to activate as the scenario advances; nothing is ever
auto-deactivated when it moves on (steers, does not restrict — ADR-0007).

The shipped scenario is `prompts/scenarios/investigation/` — seventeen
stages from scoping and evidence acquisition through triage, static, dynamic
and deep analysis to timeline, detection, reporting and analysis-derived
tooling; the triage/static/dynamic/report four-phase flow used as the running
example throughout `docs/concept.md` and ADR-0009 lives inside it as stages
C, D, E and N.

Tested through pi's own loader like every other extension
(`tests/extensions/scenario.test.mjs`), against a throwaway scenario for the
state machine and against the real shipped `investigation` scenario for shape
only (seventeen steps, each with its own title), the way `TestShippedConfig`
checks the catalogue in the Python suite rather than pinning its exact text.

## Built beyond the milestones — the general-purpose half

The RE milestones above are not the whole package: extensions ship with it
that never touch the catalogue and never call `reactor`, each with its own
ADR and its own tests, deferred to real use the same way the milestones are.

- **`rolling-context/`** — the fade, an alternative to pi's summarization
  compaction ([ADR-0019](docs/adr/0019-rolling-context-ships-here-general-purpose.md),
  [ADR-0020](docs/adr/0020-rolling-context-measures-and-cuts-like-pi-does.md)),
  split into itself, **`goal-setting/`** (the session manifest: goal,
  guidelines, steps) and **`history-tools/`** (line-addressed recovery over
  the session file)
  ([ADR-0024](docs/adr/0024-rolling-context-splits-into-goal-setting-history-tools-and-the-fade.md)).
- **`auto-continue/`** — keeps the agent going after an automatic compaction
  ends its turn, bounded against runaway cycles
  ([ADR-0025](docs/adr/0025-auto-continue-continues-after-automatic-compaction.md)).
- **`identity/`** — the working persona in the system prompt: built-ins for
  the scenarios a security professional moves between, plus an adhoc custom
  identity that can be saved as a reusable one
  ([ADR-0026](docs/adr/0026-identity-is-a-persona-block-in-the-system-prompt.md)).
- **`context-editor/`**
  ([ADR-0021](docs/adr/0021-context-editor-forks-or-filters-never-rewrites.md))
  and **`reporting/`**
  ([ADR-0023](docs/adr/0023-reporting-enforcement-is-a-filesystem-probe-not-a-heuristic.md)).

What would move any of these forward is real use, not another round of
building — same rule as the milestones.
