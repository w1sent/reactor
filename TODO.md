# TODO

Design is settled (`docs/adr/`). Milestones 1, 2 and 3 are built and tested.
The milestones were ordered by dependency — the spine is a chain where each
link needed the one before it — and what is left of each is deferred to real
use rather than another round of building, recorded under "Later — not
scheduled" below.

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
and `requires:` being inert have all moved to "Later — not scheduled" — none
of them block the spine being done, and none of them have an answer that
doesn't want more real usage first.

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
definitions has moved to "Later — not scheduled".

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

The first scenario ships with the package: `prompts/scenarios/triage/` —
triage, static, dynamic, report — the four phases used as the running example
throughout `docs/concept.md` and ADR-0009.

Tested through pi's own loader like every other extension
(`tests/extensions/scenario.test.mjs`), against a throwaway scenario for the
state machine and against the real shipped `triage` scenario for shape only
(four steps, each with its own title), the way `TestShippedConfig` checks the
catalogue in the Python suite rather than pinning its exact text.

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

## Later — not scheduled

### Catalogue gaps
25 entries against the list in `docs/concept.md`. Each new entry's `desc`
lands in every system prompt, so adding them is editorial work, not data
entry. Three outstanding:

- **QBDI** — still guesswork until someone has one to check against.
- **otool** — macOS-only, with `llvm-otool` as the Linux stand-in: it covers
  most of the same ground behind a similar interface. *Similar*, not identical,
  which is the whole design constraint. Ship them as **two catalogue entries**
  rather than one entry detecting either binary: the agent has to know which
  one it is invoking, because the flags diverge at the edges, and a single
  entry would have to lie in its `invoke` column about one platform or the
  other. Two entries also needs no schema change — `detect` takes exactly one
  binary today, and widening it to an ordered list would mean `invoke` has to
  follow the match, which is a real change to the narrowest part of the schema
  for a case that does not need it. Absent tools are not listed, so at most one
  of the two ever reaches the prompt on a given machine.

  This is also the honest test of the absent-tool path, which is currently
  untested against reality: this machine reports 25 present, 0 absent.

- **Binary Ninja headless** — not a separate entry. Headless is the Python API,
  and using it means the agent runs a Python script directly instead of putting
  it through `binja-cli`. Same scripts either way; only the invocation differs.

  So what the agent needs is not a catalogue row but a **capability flag on the
  existing `bn` entry**: headless available, or route through `binja-cli`.
  Guessing wrong wastes a turn in each direction. It gates on a commercial
  licence, so it is a per-machine fact REactor cannot infer — and one this
  machine cannot exercise, since there is no commercial licence here.

  Two pieces, in order:

  1. **Upstream, in `binja-cli`: a command that reports headless support**,
     most likely folded into `health`, which `bn`'s service probe already runs.
     Nothing on the REactor side can start before this exists.
  2. **Here: surface it in the registry annotation.** The channel exists but is
     count-shaped — `service.count` is a regex over the probe output yielding a
     number and a noun. A capability is a boolean, so this wants either a small
     schema addition (a pattern whose *match* is the annotation) or an
     abuse of the count that would read as "1 headless". Prefer the former, and
     note that a licence flag is about as stable as an annotation gets, which
     is what [ADR-0006](docs/adr/0006-registry-injected-into-system-prompt.md)
     requires of anything entering the block.

### `requires:` in REactor-authored skills is inert
`skills/` is a conventional directory at the package root, so pi's *package*
loader discovers everything in it before the extension's `resources_discover`
runs — those skills load whether or not their tools are present or active.
Only fetched upstream skills are gated today. Fixing it means serving
`skills/` from `resources_discover` too, which means moving it out of the
conventional layout.

The first skill has landed (`skills/pi-subagent/`,
[ADR-0022](docs/adr/0022-pi-subagent-gets-a-skill-so-it-gets-a-clearer-id.md))
without forcing this decision: its tool is the harness itself, present by
construction in any session that could load the skill at all, so unconditional
loading and gated loading are observably the same for it. Still open for
whichever skill lands next whose tool is not always present.

`skills/decompile-python/` is that case: `python3` is not guaranteed just
because pi is running (pi itself needs no Python; only `reactor`, a separate
CLI, does), so its `requires: [decompile-python]` is a real gap today rather
than a coincidentally-moot one — the skill loads and advertises itself even
on a machine with no Python at all. Landed anyway because it is still the
correct *eventual* behaviour once gating exists, and because ADR-0008's case
3 (no upstream skill, no `--help` to fall back to — the technique itself is
the tool) applies cleanly regardless of when this is fixed.

### Selector: probe cost and `SettingsList`
Opening the selector costs a live probe rather than reading the cache, so the
first screen is honest rather than a wall of `unknown`. Warm that is
imperceptible; cold it is the ~520 ms path, with no spinner in front of it.
Separately, `SettingsList` was rejected for reasons that may not survive pi
upgrades — if its search ever covers descriptions and Enter stops being
overloaded, most of the custom rendering could go.

### Status: network reachability and probe coverage
Network reachability is not modelled, though the original sketch said so.
Nothing in the catalogue schema describes a reachability probe, and inventing
one to fill a bullet is how a schema gets a feature nobody asked for — it
wants a real need first. Separately, only two catalogued tools declare a
service probe (`bn`, `adb`), so the footer's collapse-to-counts path is
exercised by tests and by nothing else.

### Toolset design questions
The sets are **tight on purpose** and meant to be combined, which is only
ergonomic because the selector makes activating two of them two keystrokes.
Whether that holds up is a question for real sessions, and the answer might be
that a handful of sets should absorb `general` rather than leaving it to be
added back. The other unknown is `debugging` ⊂ `dynamic` — two sets differing
by `frida` and `objection`, which is either a useful distinction or one row of
noise.

### Graph visualisation panel
An infinite explorable plane the agent can present results into — call graphs,
dataflow, module boundaries. Substantial on its own: layout, pan/zoom,
hit-testing, and a rendering model inside a TUI's line-based `render(width)`
contract. Needs a feasibility spike before it is scheduled at all. Relationship
to the node-canvas work in the plugins repo
([ADR-0029](https://github.com/w1sent/bn-plugins/blob/main/docs/adr/0029-node-canvas-architecture.md)) should be
checked before starting from scratch.

### Reference-documentation research tool
User-configurable sources (paths, URLs), searchable by both agent and user,
including PDFs and possibly OCR. Also substantial: ingest, index, rank. Likely a
sibling repo with a catalogue entry rather than an extension
([ADR-0002](docs/adr/0002-package-ships-assets-tools-are-sibling-repos.md)) —
it is a tool, not a harness integration.

### Catalogue overlay model
Package catalogue authoritative, user file holding only deltas — dissolves the
merge problem instead of managing it. Recorded as the better answer in
[ADR-0004](docs/adr/0004-config-updates-via-plain-diff.md) if hand-merging turns
out to be annoying in practice.

### `diff-config --tool`
Point the diff at `vimdiff`/`delta` rather than plain `diff(1)`. Convenience only.

### More than one scenario, and richer scenario tooling
Only `investigation` exists. Whether more scenarios want `reactor-scenario list` to
group or tag them, whether a step should be able to name more than one
toolset, and whether `pi.setActiveTools()`-based dynamic visibility for
`reactor_step_complete` is worth the shared-state risk
([ADR-0017](docs/adr/0017-scenario-steps-are-read-directly-not-pi-prompts.md))
are all questions for after a scenario has been run in anger.

## Open questions

### Cache invalidation on activation change
`cache.json` is keyed on a stamp of `tools.toml` (mtime + size), so editing the
catalogue drops stale probe results. `state.json` changes do **not** invalidate
it, which is correct — activation does not change what is installed — but it
means `reactor tools enable X` shows `X` from cache, possibly minutes stale. The
selector sidesteps this by probing live when it opens and never re-probing
while it is up, so a session's worth of toggling reads one snapshot. Fine while
the snapshot is seconds old; wrong if the overlay is ever left open.

## Resolved

- **Context optimisation** → the concrete complaint arrived: real sessions
  showed `rolling-context/`'s fade measuring tokens in a different unit than
  what actually went over the wire, occasionally cutting mid tool-call/
  tool-result pair, and cancelling pi's own overflow recovery along with the
  threshold compaction it was meant to preempt. Fixed by measuring and
  cutting the same way pi's own compaction does, plus a hard ceiling the fade
  may never cross ([ADR-0020](docs/adr/0020-rolling-context-measures-and-cuts-like-pi-does.md)).
  Alongside it, `extensions/context-editor/` gives a person the same
  decision by hand — a landscape overlay or a `$EDITOR` text file, applied by
  forking a new session or filtering the current one, since pi's session
  store cannot be rewritten in place
  ([ADR-0021](docs/adr/0021-context-editor-forks-or-filters-never-rewrites.md)).
- **No REactor-authored skills yet** → the first one has landed,
  `skills/pi-subagent/`: how to scope a delegated `pi -p` subagent, which
  `--help` alone does not connect — pi's own `--tools`/`--no-tools` flags are
  an enforced boundary on what it can call, REactor's toolbox/toolset
  activation is advisory only (ADR-0007), and conflating the two is the
  mistake worth a skill over. `[tool.pi]` renamed to `[tool.pi-subagent]`
  alongside it, so the skill's `requires:` has a name that means "delegating
  to a subagent" rather than "the harness itself"
  ([ADR-0022](docs/adr/0022-pi-subagent-gets-a-skill-so-it-gets-a-clearer-id.md)).
- **Where scenario definitions live** → Markdown files under
  `prompts/scenarios/<id>/`, read directly by `extensions/scenario/` rather
  than surfaced as pi prompt commands
  ([ADR-0017](docs/adr/0017-scenario-steps-are-read-directly-not-pi-prompts.md)).
  A bare step is not a useful thing to invoke on its own — advancing is
  stateful, and a slash command has no memory of what came before it.
- **A pi-side toggle to hide the toolbox, and to mute a status service** →
  `reactor.json` in pi's own agent directory, next to `settings.json` but not
  inside it, since `Settings` has no extension point for a third party's
  fields
  ([ADR-0016](docs/adr/0016-extension-toggles-live-in-their-own-pi-side-file.md)).
  Reachable from inside pi with `/reactor-toolbox` and `/reactor-status
  mute|unmute`, since pi's own `/settings` is a closed component with nothing
  for an extension to add a row to.
- **Shared probe cache across extensions** → the cache *is* the sharing, and
  nothing else is ([ADR-0014](docs/adr/0014-extensions-share-the-cache-not-each-other.md)).
  The obvious design — a module both extensions import — does not work at all:
  pi loads each extension through its own `createJiti(..., {moduleCache: false})`,
  so the import is instantiated twice and the two caches drift while appearing
  to work. `pi.events` would work and was rejected for coupling.

  Last-writer-wins confirmed safe: every writer loads the file, replaces only
  the keys it probed, and writes it back, so a lost update costs a re-probe and
  never a wrong answer. One real hole was found and closed — `_write_json` used
  a fixed `<name>.tmp`, and two processes could interleave into it and rename
  the mixture into place. The temp name now carries the pid.

  Cost of not sharing: one extra process spawn per turn, measured at ~70 ms of
  Python startup with everything cached. If a third extension arrives that
  stops being free and the answer changes.
- **Skill name collisions with `~/.agents/skills/` are out of scope** → decided,
  not fixed. pi discovers `~/.agents/skills/` and `<project>/.agents/skills/` on
  its own (`package-manager.js:279,1941`) — an ecosystem-wide convention shared
  with other agent tools. `loadSkills` keys skills by their declared `name:`,
  adds those directories *before* extension-contributed paths, and on a
  duplicate keeps the first. So an independently installed copy wins and
  REactor's pinned one is discarded, which makes the commit in
  `.reactor-skill.json` fiction and defeats gating: `resources_discover` is
  additive by construction (`extendResources` merges into `lastSkillPaths` and
  treats an empty array as a no-op, `resource-loader.js:242`), so no extension
  can retract a skill it did not add. Reproduced on a real install.

  REactor's own `skillPaths` is correct in both directions; everything that goes
  wrong goes wrong downstream, inside pi's loader, over directories REactor
  neither writes nor owns. Detecting and reporting the clash would be REactor
  taking responsibility for another tool's installation, and the fix belongs in
  pi. Left as a known interaction rather than a REactor defect.
- **The extensions had no automated test** → `node --test`, loading each one
  through pi's own loader against the real CLI
  ([ADR-0012](docs/adr/0012-extensions-tested-through-pi-s-own-loader.md)). 42
  tests: the registry's failure modes (missing CLI, garbage, error payload,
  last-good-block), skill gating, and the selector driven by keystroke —
  including the off-then-on round trip that leaves `state.json` byte-identical,
  and the two display bugs that were real. Checked by mutation rather than
  trusted for being green; one mutation showed a wrongly-opened overlay *hung*
  the suite instead of failing it, which the fake host now prevents.
- **`enable` after `disable` left a pin, and there was no way back** → activation
  edits are minimal and `reactor tools reset <id>` drops an override outright
  ([ADR-0011](docs/adr/0011-selector-edits-overrides-not-outcomes.md)). The wart
  was tolerable through a CLI and would not have been through a selector, which
  is what forced the decision. The cost is recorded in the ADR: an override is
  forgotten when the base catches up with it.
- **ILSpy → `ilspycmd`** → catalogued as the CLI, installed with
  `dotnet tool install --global ilspycmd`, which added `dotnet` as a manager.
  The GUI is not a thing the agent can invoke, so it is not the entry.
- **blutter is not a catalogue entry** → it is a Binary Ninja plugin, with no
  binary to detect and no install recipe meaningful outside a BN installation.
  It ships from `bn-plugins`, and knowledge about it belongs in the `bn` skill
  fetched from that repo. Reasoning in `docs/concept.md`.
- **ImHex is not a catalogue entry** → it has a binary, so it passed the test
  that rules out Binary Ninja plugins, but it cannot be scripted. The registry
  exists so the *agent* reaches for the right tool; an entry it can never invoke
  costs a line in every system prompt and buys nothing, because the only reader
  it serves is the human, who already knows what they installed. Reasoning in
  `docs/concept.md`.
- **Install recipes** → all 56 package references now check out against their
  managers' real indexes, and `scripts/verify-recipes.py` keeps them honest.
  Six were wrong: `pacman -S rr` and `pacman -S apktool` (both AUR-only on
  Arch, and `apktool` is `android-apktool` there), `brew install frida` (no such
  formula or cask), `brew install blacktop/tap/ipsw` (promoted to core),
  `brew install android-platform-tools` (a cask), and `apt install jadx` (in
  neither Debian nor Ubuntu, at any suite). Fixing the Arch pair meant declaring
  `paru` and `yay` as managers, which ADR-0010's model already covers — an AUR
  helper is a package manager and its binary is a testable predicate.

  The jadx one only surfaced after the verifier itself was fixed:
  `packages.debian.org` serves its "No such package" page with **status 200**,
  so checking the status code passed every string ever handed to it, and the
  Launchpad half never got to matter because Debian had already "found" it. The
  check now reads the body. A verifier that cannot fail is worse than no
  verifier, because it is quoted as evidence.
- **Probe cost at session start** → measured; there was no problem. 79 ms warm
  (the per-turn path, ~90% of it Python startup), 520 ms cold, once. Service
  probes are cheap; *version* probes on JVM and interpreter-backed tools
  dominate the cold path. `detect_ttl`/`service_ttl` left alone, and session
  start still probes services. Numbers and consequences in
  [ADR-0006](docs/adr/0006-registry-injected-into-system-prompt.md).
- **Platform detection granularity** → package-manager keys, ranked by a
  user-editable preference list, free text never executed
  ([ADR-0010](docs/adr/0010-install-recipes-keyed-by-package-manager.md)). No
  `/etc/os-release` parsing anywhere.
- **Version-drift reporting for fetched skills** → `reactor doctor
  --check-skills` compares the recorded commit against `git ls-remote`. Opt-in,
  so the default `doctor` stays offline and fast.
- **Relationship to the plugins repo** → confirmed. The repo is
  `github.com/w1sent/bn-plugins`; `bn` ships from
  `ai/mcp-server/skills/binja-cli/scripts/bn` and REactor fetches that skill
  directory by URL. Nothing migrates here.
