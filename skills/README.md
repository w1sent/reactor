# skills/

Skills REactor writes itself. Expect very few files here, and treat adding one
as a decision that needs justifying.

The order of preference is
[ADR-0008](../docs/adr/0008-aggregate-upstream-skills.md):

1. the tool author's own skill, fetched at install into `~/.pi/reactor/skills/`
2. `<tool> --help` / `man <tool>`, read by the agent on demand
3. a skill here — only where neither of the above suffices

In practice that means a skill here should carry **cross-tool workflow
knowledge** that no single tool's `--help` could contain. If a draft skill reads
like a summary of one tool's options, it should not exist; the tool already
documents itself better and stays current when it is updated.

Every loaded skill puts its `name` and `description` into the system prompt
permanently. The body and `references/` cost nothing until read. So the count is
what to be careful about, not the length.

## Format

```
<skill-name>/
├── SKILL.md          frontmatter + instructions
├── scripts/          helper scripts, invoked by relative path
└── references/       detail, read on demand
```

```yaml
---
name: some-skill              # lowercase, digits, hyphens; 1–64 chars
description: ...              # REQUIRED — no description, no load
requires: [frida, adb]        # REActor-specific; inert to pi
---
```

pi's loader reads only `name`, `description` and `disable-model-invocation`
(verified — `docs/pi-api-notes.md`). `requires` lists catalogue ids and is what
binds a skill to activation state.
