# TODO

Design is settled (`docs/adr/`); nothing is implemented. Milestones below are
ordered by dependency — the spine is a chain where each link needs the one
before it.

## Milestone 1 — Spine

The smallest thing that delivers the core value: the agent knows what is on this
machine. No custom TUI components.

### Catalogue schema and starter contents
`tools.toml` currently holds a starter set covering the tools verified present on
this workstation plus a few known-absent ones, to exercise both paths. Needs:
finalising the schema (probe shapes, service definitions, install-recipe keys,
skill sources), then filling in the target surface listed in `docs/concept.md`.
Every entry's `desc` is written for a model choosing a tool, not for a human
browsing — it lands in every system prompt, so it gets a soft length budget and
review. Platform keys need deciding: distro-level (`arch`, `debian`, `fedora`)
versus manager-level (`pacman`, `apt`, `brew`, `cargo`, `pipx`, `uv`), and how
detection picks between them on a machine with several.

### `reactor` CLI
Stdlib-only Python, 3.11+ floor for `tomllib`, symlinked to `~/.local/bin/reactor`
([ADR-0005](docs/adr/0005-reactor-cli-stdlib-python.md)). Subcommands:
`doctor`, `tools list|show`, `skills list|show`, `install`, `diff-config`,
`overwrite-config`. `--format json` on every one — that output shape is the
extensions' only interface and is part of the contract, so pin it early and
treat changes to it as breaking.

Probe execution needs timeouts and a cached-value fallback from day one; a
hanging probe stalls an agent turn once the extension is wired up.

### `scripts/install.py`
Symlinks the CLI, seeds `~/.pi/reactor/{tools,toolsets}.toml` without clobbering
existing files, creates `state.json`, fetches configured upstream skills into
`~/.pi/reactor/skills/<tool>/`, and reports which are missing. `--link` and
`--copy` modes mirroring the plugins repo installer. Warns only when a
*configured* skill could not be fetched
([ADR-0008](docs/adr/0008-aggregate-upstream-skills.md)).

### `extensions/tool-registry/`
Probe → cache → render → return from `before_agent_start`
([ADR-0006](docs/adr/0006-registry-injected-into-system-prompt.md)). Answers
`resources_discover` with the skill and prompt paths of active tools. One-line
status via `ctx.ui.setStatus`. Shells out to `reactor … --format json`; parses no
TOML.

The determinism requirement is a real test target: same machine state must
produce byte-identical output across turns. Worth an actual test rather than an
intention.

### First REactor-authored skills
Only where `--help` and upstream skills genuinely do not suffice
([ADR-0008](docs/adr/0008-aggregate-upstream-skills.md)) — which means
cross-tool workflow knowledge, not tool usage. Expect very few. Deciding *which*
is a task in itself and should follow real sessions, not precede them.

## Milestone 2 — Selector and status

Both are pure TUI over the CLI's JSON; no new backend.

### `extensions/selector/`
Search the catalogue, inspect an entry, read a fetched skill, toggle individual
tools and toolsets. `ctx.ui.custom` with `SelectList`; `ctx.reload()` after a
toggle so `resources_discover` re-runs. Persists to `~/.pi/reactor/state.json`,
with `./.reactor/state.json` overriding per project. Must degrade cleanly when
`ctx.hasUI` is false.

### `extensions/status/`
Live service and device state — which services are up, what is attached, network
reachability. Footer entry plus an expandable panel. Shares the registry
extension's probe cache rather than probing independently; how that sharing works
across two extensions needs designing (shared module, or one extension exposing
state the other reads).

### Toolset definitions
`toolsets.toml` needs a real predefined set, which depends on the catalogue being
filled in. User-defined toolsets live in the same file after seeding.

## Milestone 3 — Scenarios

`reactor_step_complete` and the step-briefing mechanism
([ADR-0009](docs/adr/0009-scenarios-advance-by-tool-result.md)), plus the first
scenario as prompt templates. Deliberately after the registry has been used in
anger, because the right step decomposition is not knowable in advance. A manual
`/reactor next` override ships alongside.

## Later — not scheduled

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

### Context optimisation
Reducing context, model-specific prompt variants. Underspecified; needs a concrete
complaint from real sessions before it becomes a design.

### Catalogue overlay model
Package catalogue authoritative, user file holding only deltas — dissolves the
merge problem instead of managing it. Recorded as the better answer in
[ADR-0004](docs/adr/0004-config-updates-via-plain-diff.md) if hand-merging turns
out to be annoying in practice.

### `diff-config --tool`
Point the diff at `vimdiff`/`delta` rather than plain `diff(1)`. Convenience only.

## Open questions

### Platform detection granularity
`reactor doctor` must map "this machine" to an install recipe. Distro detection
(`/etc/os-release`), package-manager detection, or both with a precedence rule —
and what happens on a machine with `pacman`, `cargo`, `uv` and `brew` all
present. Affects the catalogue's schema, so it wants settling in Milestone 1.

### Probe cost at session start
Detecting ~25 tools is ~25 `which` calls (cheap) plus service probes (not cheap —
`bn health` is an HTTP round trip, `adb devices` may start a daemon). Whether
session start blocks on service probes or renders without them and refreshes
shortly after is unresolved, and it is user-visible latency either way.

### Shared probe cache across extensions
The registry and status extensions both want current probe results. Two
independent probe loops would double the cost and could disagree. Needs a
decision on where that state lives.

### Version-drift reporting for fetched skills
`reactor doctor` should report a fetched skill as stale against its pinned ref.
Cheaply, without a network round trip per skill on every doctor run.

### Relationship to the plugins repo
`bn` ships from `plugins/skills/binja-cli/scripts/bn` and REactor's catalogue
merely references it. Confirm that stays true — that no part of `bn` migrates
here — and that the skill source in `tools.toml` points at the right ref.

### Where scenario definitions live
Prompt templates in `prompts/`, or a richer format the extension reads? Deferred
with Milestone 3, but the answer shapes whether `resources_discover` is enough.
