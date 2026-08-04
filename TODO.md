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

**The catalogue is a starter set, not the target surface.** 24 entries against
the list in `docs/concept.md`. Missing: QBDI, otool, binja headless — each is
guesswork until someone has one to check it against. otool is macOS-only,
which anywhere else makes it the honest test of the absent-tool path.

Each new entry's `desc` lands in every system prompt, so adding them is
editorial work, not data entry.

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

**A fetched skill can lose a name collision to a directory REactor does not
control, and then gating it does nothing.** pi discovers `~/.agents/skills/` and
`<project>/.agents/skills/` on its own (`package-manager.js:279,1941`) — an
ecosystem-wide convention shared with other agent tools, nothing to do with
REactor. Anyone who has installed a tool's skill that way already has a copy of
what REactor fetches, usually at a different revision.

pi's `loadSkills` keys skills by their declared `name:`, adds the default
directories *before* extension-contributed paths, and on a duplicate name keeps
the first and records a collision diagnostic. The independently installed copy
therefore wins and REactor's pinned copy is discarded. Reproduced on a real
install.

Two things break. The pin in `.reactor-skill.json` becomes fiction — REactor
reports a commit for a file nobody loaded. And gating is defeated: deactivating
the tool removes REactor's path from `resources_discover`, and the other copy
stays, because REactor never contributed it and cannot retract it. The CLI's
`skillPaths` is correct in both directions, so this is entirely about what pi
does downstream of it.

`resources_discover` is additive by construction — `extendResources` *merges*
into `lastSkillPaths` and treats an empty array as a no-op
(`resource-loader.js:242`) — so no extension can remove a skill it did not add.
That is pi's design, not a bug to route around. The options are to detect the
collision and say so (`reactor doctor` can read both copies' `name:` and compare),
or to stop fetching a skill the user already has. Detection first: the failure
is silent today, and that is the worse half.

## Milestone 2 — Selector and status

### `extensions/selector/` — **built**

`/reactor-tools`: one overlay, two panes switched with Tab, fuzzy search over
id, name, description and tags, space to toggle, Enter to inspect, Ctrl+R to
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

Open against it:

- **No automated test**, same as `tool-registry`. It was driven through `jiti`
  with a stubbed pi API against the real CLI, which exercised every key path
  including the writes — but by hand, from a scratch harness that was not kept.
  Two untested extensions is now the argument for a TypeScript test runner that
  was deferred when there was one.
- **Opening it costs a live probe** rather than reading the cache, so the first
  screen is honest rather than a wall of `unknown`. Warm that is imperceptible;
  cold it is the ~520 ms path, with no spinner in front of it.
- **`SettingsList` was rejected for reasons that may not survive pi upgrades**
  — if its search ever covers descriptions and Enter stops being overloaded,
  most of the custom rendering could go.

### `extensions/status/`
Live service and device state — which services are up, what is attached, network
reachability. Footer entry plus an expandable panel. Shares the registry
extension's probe cache rather than probing independently; how that sharing works
across two extensions needs designing (shared module, or one extension exposing
state the other reads).

### Toolset definitions
`toolsets.toml` needs a real predefined set, which depends on the catalogue being
filled in. User-defined toolsets live in the same file after seeding.

Tag selection is a **union**, and that has now silently broken two shipped
toolsets — `all` built from a tag list dropped whatever nobody had tagged, and
`native` declaring `["static", "native"]` collected every static tool including
a Java decompiler. Both are fixed and
`test_a_toolset_named_after_a_tag_selects_only_that_tag` guards the second, but
the sharp edge is the semantics, not those two entries. If intersection turns
out to be what people reach for when writing their own, that is a schema
question (`tags` vs `all_tags`) to settle while the selector is being built and
toolset authoring becomes something users actually do.

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
means `reactor tools enable X` shows `X` from cache, possibly minutes stale. The
selector sidesteps this by probing live when it opens and never re-probing
while it is up, so a session's worth of toggling reads one snapshot. Fine while
the snapshot is seconds old; wrong if the overlay is ever left open.

### Where scenario definitions live
Prompt templates in `prompts/`, or a richer format the extension reads? Deferred
with Milestone 3, but the answer shapes whether `resources_discover` is enough.

## Resolved

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
