# REactor aggregates upstream skills; `--help` is the documentation

REactor does not write documentation for the tools it exposes. The order of
preference is:

1. **The tool author's own Agent Skill**, if one exists — fetched at install time
   from a source named in the catalogue.
2. **`<tool> --help` / `man <tool>`**, which the agent reads on demand. This is
   the expected outcome for most tools and is not a gap.
3. **A REactor-authored `SKILL.md`**, only where the first two genuinely do not
   suffice.

## Why

The author of a tool knows it better than we do, and their documentation tracks
their releases. A hand-written summary of `frida`'s options is worse than
`frida --help` the day it is written and worse still six months later. Writing
and then maintaining twenty-five such summaries is a large, permanent cost that
produces something strictly inferior to what already ships with each tool.

This generalises [ADR-0038 in the plugins
repo](https://github.com/w1sent/bn-plugins/blob/main/docs/adr/0038-binja-cli-skill-frontend.md), which reached
the same conclusion for a single tool: "a lean `SKILL.md` covers orientation;
`bn --help` / `bn <subcommand> --help` is the actual progressive-disclosure
mechanism — static, full documentation, works offline, and is more granular than
splitting into multiple skill files."

Skills also cost context in a way `--help` does not: every loaded skill puts its
name and description into the system prompt permanently, whereas `--help` costs
nothing until it is run. Fewer, better-targeted skills is the correct direction.

## Fetching

A catalogue entry may name an upstream skill:

```toml
[tool.bn.skill]
source = "git+https://…/plugins"
path   = "skills/binja-cli"
ref    = "main"
```

`reactor install` fetches it into `~/.pi/reactor/skills/<tool>/`, pinned to the
given ref, recording source, ref and fetch time alongside it. Git sources are
required to work; plain URLs are supported for single-file skills. The
tool-registry extension exposes the directory through pi's `resources_discover`,
so a fetched skill is gated by activation state exactly like a REactor-authored
one, and never mixes with skills the user wrote in `~/.pi/agent/skills/`.

`reactor doctor` reports staleness against the recorded ref, so drift is visible
rather than silent.

## Warnings

A configured skill that cannot be fetched — network failure, moved repository,
bad path — is a **warning**, because something the catalogue promised is missing
and the user should know.

A tool with **no** configured skill is not a warning. It is the normal case. It
gets its registry line and relies on `--help`, and REactor says nothing about it.

## Consequences

- REactor carries an implicit dependency on upstream repositories staying where
  the catalogue says they are. Ref pinning plus a visible staleness report is the
  mitigation; a broken source degrades to case 2, which is a working fallback.
- Fetched content is third-party and executes nothing on its own, but it does
  instruct a model. `reactor skills show <tool>` exists so it can be reviewed
  before it is trusted, and fetching is part of the opt-in install step, not
  something that happens silently.
- REactor-authored skills are expected to be few, and to cover
  *cross-tool* workflow knowledge that no single tool's `--help` could contain —
  which is where the remaining value is.

## Considered and rejected

- **Writing a skill per tool** — rejected; duplicates `--help` worse, ages
  badly, and taxes the system prompt for every tool installed.
- **Fetching into `~/.pi/agent/skills/`** — pi discovers that directory
  automatically, so no `resources_discover` wiring would be needed and the skills
  would work with REactor's extension disabled. Rejected: it interleaves fetched
  third-party content with skills the user wrote, activation cannot gate it,
  names can collide, and uninstalling means deleting from the user's own skills
  directory.
- **Vendoring upstream skills into this repo** — rejected. It works offline and
  the exact content is reviewable in our own history, but it means redistributing
  others' work under their licences, and every upstream fix needs a REactor
  release to propagate.
- **Recording the URL but fetching lazily on demand** — rejected; the skill is
  absent at exactly the moment it would help, and a mid-session fetch needs a
  reload before pi sees it.
