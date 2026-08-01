# `tools.toml` is the single source of truth, and it lives in `~/.pi/reactor/`

Everything REactor knows about a tool lives in one catalogue entry: identity, the
one-line description that appears in the registry, detection and version probes,
service probe, per-platform install recipes, tags, and the upstream skill source
if the tool ships one. Four consumers read it — the CLI, the doctor, the
installer, and the tool-registry extension — and none of them carries its own
copy of that knowledge.

The catalogue ships in this repo as `tools.toml`, but the file everything reads
at runtime is `~/.pi/reactor/tools.toml`, seeded from the shipped copy at install
time. `toolsets.toml` is handled identically.

## Why one file

The alternative was declaring dependencies in each `SKILL.md`'s frontmatter and
having the doctor aggregate across skills. That colocates a skill with its
requirements, which is genuinely nice, but it duplicates install recipes across
every skill that shares a tool, and it leaves tools that have no skill — ripgrep,
`jq`, `file` — with nowhere to live. Since most tools will never have a
REactor-authored skill ([ADR-0008](0008-aggregate-upstream-skills.md)), that is
the common case, not the edge case.

So the catalogue is central and skills reference it by id:

```yaml
---
name: some-skill
description: ...
requires: [frida, adb]
---
```

`requires` is inert to pi — its skill loader reads only `name`, `description`
and `disable-model-invocation` (verified in `dist/core/skills.js`) — so the key
is free for REactor to define and costs nothing when pi loads the skill.

## Why the runtime copy lives outside the package

pi's package update path runs `git reset --hard` followed by `git clean -fdx`
inside the installed package directory
([ADR-0002](0002-package-ships-assets-tools-are-sibling-repos.md)). Anything a
user edited there would be destroyed on the next update, silently. A catalogue
the user cannot safely edit is a catalogue that cannot carry a distro-specific
install recipe, a locally-built tool, or a tweaked service probe.

`~/.pi/reactor/` is outside pi's package tree and survives updates. It sits under
`~/.pi/` rather than XDG because pi is the only target harness and splitting
state across two roots buys nothing ([ADR-0001](0001-pi-is-the-only-target-harness.md)).

```
~/.pi/reactor/
├── tools.toml       live catalogue — user-editable, survives pi updates
├── toolsets.toml    live toolsets — same
├── state.json       which tools and toolsets are currently active
└── skills/<tool>/   upstream skills fetched at install time
```

Install seeds a file only if it is absent; it never clobbers an existing one.
How shipped changes reach an already-seeded file is
[ADR-0004](0004-config-updates-via-plain-diff.md).

Project-scoped overrides (`./.reactor/state.json`) apply to activation state
only. The catalogue itself is machine-scoped, because what is installed is a
property of the machine, not of the directory you happen to be analysing in.

## Schema sketch

```toml
version = 1

[tool.bn]
name    = "Binary Ninja"
desc    = "reverse engineering framework, live BN session"  # ← system prompt
binary  = "bn"
tags    = ["static", "native"]
service = { probe = ["bn", "health"], label = "BN session" }
skill   = { source = "git+https://…/plugins", path = "skills/binja-cli", ref = "main" }

[tool.bn.install]
git = "…"
```

`desc` is load-bearing in a way the other fields are not: it appears in every
system prompt, for every session, for as long as the tool is installed and
active. It gets a soft length budget and is written for a model, not for a
README.

## Consequences

- Adding a tool is one TOML block. No code changes in the CLI, the doctor, the
  installer or the extension.
- The extensions never parse TOML. They shell out to `reactor … --format json`
  ([ADR-0005](0005-reactor-cli-stdlib-python.md)), so the TUI can never disagree
  with the CLI about what the catalogue says.
- Python reads it with `tomllib` from the standard library (3.11+). That is a
  hard floor on the supported Python version and is recorded as such.
- Since the runtime catalogue is a user file, "no user-level catalogue" is not a
  meaningful restriction any more — there is exactly one catalogue and the user
  owns it. An *overlay* mechanism (package authoritative, user file holding only
  deltas) remains a possible later refinement.

## Considered and rejected

- **Per-skill frontmatter as the source of truth** — rejected; duplicates
  recipes across skills and has no home for tools without skills.
- **A Python module holding the data as code** — rejected. Maximum
  expressiveness, but nothing that is not Python can read it, which rules out
  the option of the extension reading it directly if that ever becomes wanted.
- **JSON instead of TOML** — rejected. Both Python and TypeScript parse JSON
  with zero dependencies, which is a real advantage, but the extensions shell
  out rather than parse, so the advantage never gets used — and hand-maintaining
  a catalogue of per-platform shell one-liners in a format with no comments is
  unpleasant.
- **Leaving the catalogue inside the package and reading it there** — rejected;
  `git clean -fdx` on every update makes user edits unsafe.
