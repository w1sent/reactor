# prompts/

pi prompt templates — Markdown with YAML frontmatter, invoked as `/name` in the
editor. The filename minus `.md` is the command name. Discovery here is
**non-recursive**; subdirectories need explicit configuration.

```markdown
---
description: Triage an unknown binary
argument-hint: "<path> [--deep]"
---
Triage `$1`. Identify format and architecture, detect packing, ...
```

Substitution: `$1`, `$2`, `$@` / `$ARGUMENTS`, `${1:-default}`, `${@:N}`,
`${@:N:L}`. Use `<angle brackets>` for required arguments in `argument-hint` and
`[square brackets]` for optional ones.

## Planned

Scenario steps (Milestone 3) live here: one template per step of a multi-step
analysis, with the agent advancing via `reactor_step_complete` —
[ADR-0009](../docs/adr/0009-scenarios-advance-by-tool-result.md). Whether plain
templates are expressive enough for that, or the scenario extension needs a
richer format of its own, is an open question in `TODO.md`.

Templates are exposed through `resources_discover`, so they are gated by
activation state like everything else.
