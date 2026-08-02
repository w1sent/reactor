# TODO

Design is settled (`docs/adr/`). Milestone 1 is built and tested; Milestones 2
and 3 are not started. The milestones are ordered by dependency — the spine is a
chain where each link needs the one before it.

## Milestone 1 — Spine — **done, with the gaps listed below**

The smallest thing that delivers the core value: the agent knows what is on this
machine. No custom TUI components.

- **Catalogue schema** — settled. Install keys are package managers, ranked by a
  user-editable `[platform].prefer`
  ([ADR-0010](docs/adr/0010-install-recipes-keyed-by-package-manager.md));
  detection is `binary` or `python_module`; service probes report state plus an
  optional regex line count and nothing else.
- **`reactor` CLI** — `bin/reactor`, one stdlib-only file. Surface in
  `bin/README.md`; `--format json` everywhere and pinned by test.
- **`scripts/install.py`** — symlink, seed without clobbering, `state.json`,
  fetch upstream skills. Verified end to end against the real `bn-plugins` and
  `ipsw-skill` repositories.
- **`extensions/tool-registry/`** — block into the system prompt on
  `before_agent_start`, `skillPaths` on `resources_discover`, status line,
  `/reactor`. Exercised through `jiti` with a stubbed API.
- **Determinism** — a test, not an intention: `TestRegistryDeterminism` and
  `TestJsonContract.test_registry_block_is_stable_across_processes`.

### Still open in Milestone 1

**The catalogue is a starter set, not the target surface.** 21 entries against
the list in `docs/concept.md`. Missing at least: ImHex, blutter, lldb, qbdi,
aapt2, otool, ilspy, binja headless. Each new entry's `desc` lands in every
system prompt, so adding them is editorial work, not data entry. Entries with an install to check against can be catalogued from a
real binary rather than from memory.

**No REactor-authored skills yet**, which is the expected state
([ADR-0008](docs/adr/0008-aggregate-upstream-skills.md)) — they are only worth
writing where `--help` and upstream skills genuinely do not suffice, meaning
cross-tool workflow knowledge. Deciding *which* should follow real sessions
rather than precede them.

**The extension has no automated test.** It was driven by hand through `jiti`
with a stubbed `ExtensionAPI`. There is no TypeScript test runner in the
package; adding one is more justifiable once the selector exists too.

**`requires:` in REactor-authored skills is inert.** `skills/` is a conventional
directory at the package root, so pi's *package* loader discovers everything in
it before the extension's `resources_discover` runs — those skills load whether
or not their tools are present or active. Only fetched upstream skills are gated
today. Fixing it means serving `skills/` from `resources_discover` too, which
means moving it out of the conventional layout. Costs nothing while the
directory is empty; decide before the first skill lands in it.

**REactor is not actually installed on a development host.**
`~/.pi/reactor/` does not exist, so every `reactor` invocation here reads the
shipped catalogue through the fallback path and writes no cache. Everything has
been verified against scratch config directories via `REACTOR_CONFIG_DIR`, which
is a faithful but not identical arrangement. Running `scripts/install.py` for
real is the obvious next validation, and a prerequisite for using REactor in
anger.

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

### Shared probe cache across extensions
The registry and status extensions both want current probe results. `cache.json`
in `~/.pi/reactor/` already gives them a shared store, and it is TTL-keyed so two
readers cannot disagree for longer than `service_ttl` — but there is no locking,
and two processes probing concurrently will both write. Last-writer-wins is
probably fine for this data; confirm that before the status extension makes it
a real concurrency, and note that a corrupt cache degrades to a re-probe rather
than an error.

### Cache invalidation on activation change
`cache.json` is keyed on a stamp of `tools.toml` (mtime + size), so editing the
catalogue drops stale probe results. `state.json` changes do **not** invalidate
it, which is correct — activation does not change what is installed — but it
means `reactor tools enable X` shows `X` from cache, possibly minutes stale.
Acceptable now; revisit if the selector makes toggling frequent.

### Where scenario definitions live
Prompt templates in `prompts/`, or a richer format the extension reads? Deferred
with Milestone 3, but the answer shapes whether `resources_discover` is enough.

## Resolved

- **Install recipes** → all 49 package references now check out against their
  managers' real indexes, and `scripts/verify-recipes.py` keeps them honest.
  Five were wrong: `pacman -S rr` and `pacman -S apktool` (both AUR-only on
  Arch, and `apktool` is `android-apktool` there), `brew install frida` (no such
  formula or cask), `brew install blacktop/tap/ipsw` (promoted to core), and
  `brew install android-platform-tools` (a cask). Fixing the Arch pair meant
  declaring `paru` and `yay` as managers, which ADR-0010's model already covers
  — an AUR helper is a package manager and its binary is a testable predicate.
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
