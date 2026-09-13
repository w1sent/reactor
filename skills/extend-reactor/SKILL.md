---
name: extend-reactor
description: Add a tool or toolset to REactor's catalogue and toolsets -- entry schema, install recipes, verification and the byte-stability rules. A development skill, invoked by the user when extending REactor itself; not tied to any catalogued tool.
---

# extend-reactor

Add a `[tool.<id>]` entry to the shipped `tools.toml` and/or a
`[toolset.<id>]` to `toolsets.toml`, so the agent is told about a new
capability on every machine that installs REactor.

## Before writing anything

Read the two how-to guides in this repository — they are the detailed
reference and this skill does not repeat them:

- `docs/howto/add-a-tool.md` — the catalogue entry, install recipes,
  `scripts/verify-recipes.py`, and the verification flow.
- `docs/howto/add-a-toolset.md` — intersecting tags vs. explicit members, the
  shipped copy vs. the user's copy, and scenario-step wiring.

The full schema comment block lives at the top of `tools.toml` and
`toolsets.toml` themselves; `docs/concept.md` lists the target surface.

## Where to edit

The repo-root `tools.toml` / `toolsets.toml` are the **shipped seeds**. A user
machine reads the installed copies at `~/.pi/reactor/` — never edit those in
the package tree on a user machine (pi's package update runs `git clean
-fdx`). After an update, `reactor diff-config` shows the difference against
installed copies and the user merges by hand.

## Hard invariants

These are the failure modes; the how-tos explain the reasoning.

- **`desc` is one line, capability not usage.** It lands in every system
  prompt for as long as the tool is installed and active — the
  highest-leverage text in the project.
- **`detect` is exactly one of** `binary` or `python_module`. Anything else is
  a schema conversation, not a hack.
- **A service probe's registry detail is at most a count** (`count = { pattern
  = '…', noun = "…" }`) — never free text, or the registry block rewrites the
  system prompt every time a service jitter
  ([ADR-0006](../../docs/adr/0006-registry-injected-into-system-prompt.md)).
- **Install recipes are keyed by package managers** (`[platform.manager]`),
  ranked by `[platform].prefer`; other keys are notes, shown but never
  executed. Run `python3 scripts/verify-recipes.py` after touching any
  install table.
- **The registry block must stay byte-identical** for an unchanged machine —
  if your `desc` is two lines or your probe emits free text, determinism
  breaks first.
- **Toolset tags intersect** ([ADR-0013](../../docs/adr/0013-toolset-tags-intersect.md)):
  `["static", "native"]` is *static AND native*. Prefer `tags` over explicit
  `tools`; name a tool only when it genuinely lacks the tag. Keep sets tight —
  they are meant to be combined, not self-sufficient.
- **Activation is additive and advisory**
  ([ADR-0007](../../docs/adr/0007-deactivation-is-soft.md)): enabling a
  toolset never deactivates another, and nothing is ever blocked.

## Verification

```bash
python3 scripts/verify-recipes.py    # install recipes against manager indexes
python3 tests/test_reactor.py        # shipped-catalogue shape + CLI suite
reactor doctor                       # the tool appears, present or missing
reactor registry                     # the block, as the agent sees it
reactor toolsets enable <id>         # what a toolset activation advertises
```

A new entry that matches an area already listed in `docs/concept.md` needs no
further documentation; one that opens a *new* area updates that list — and a
schema change wants an ADR before the code.
