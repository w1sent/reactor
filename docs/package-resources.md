# The package's pi resource directories

`skills/`, `prompts/` and `themes/` at the package root carry conventional names,
so pi discovers what is in them automatically. This file documents all three.

It lives in `docs/` rather than as a `README.md` inside each of them, because
**a pi resource directory is a namespace pi enumerates, not a place for repo
docs.** The prompt-template loader is the one that proves it: it registers every
`.md` file in `prompts/`, named by basename, with no frontmatter requirement and
no ignore-file support (`prompt-templates.js`). A `prompts/README.md` therefore
becomes a `/README` command in the editor.

The other two only look safe. `themes/` is filtered to `.json`, and a skill with
no `description:` is refused — but both are incidental properties of pi's
loaders, not a promise, and neither is a reason to keep documentation somewhere
it can be mistaken for content.

## `skills/`

Skills REactor writes itself. Expect very few files here, and treat adding one
as a decision that needs justifying.

The order of preference is
[ADR-0008](adr/0008-aggregate-upstream-skills.md):

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

### Format

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
(verified — `pi-api-notes.md`). `requires` lists catalogue ids and is intended to
bind a skill to activation state.

> **`requires` is not enforced yet.** Skills in this directory are discovered by
> pi's *package* loader because `skills/` is a conventional directory at the
> package root — which happens before, and independently of, the tool-registry
> extension's `resources_discover`. So a skill here loads whether or not its
> tools are present or active, and `requires` is currently documentation.
>
> Only *fetched upstream* skills in `~/.pi/reactor/skills/<tool>/` are gated
> today, because those are handed to pi by the extension. Gating this directory
> too would mean moving these skills out of the package's conventional layout
> and serving them from `resources_discover` as well. That is a real change and
> it is not made yet — see `TODO.md`. Since the expected count here is near
> zero, it has cost nothing so far.

## `prompts/`

pi prompt templates — Markdown with YAML frontmatter, invoked as `/name` in the
editor. The filename minus `.md` is the command name. Discovery is
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

Every top-level `.md` here is a command, so nothing else at this level may be
one.

### `scenarios/`

The one subdirectory, and it is not auto-discovered as commands the way the
top level is — a bare scenario step is not a useful thing to invoke on its
own, since advancing is stateful and a raw prompt command has no memory of
which step came before it. `extensions/scenario/` reads these files directly
with `node:fs` instead of through `resources_discover`
([ADR-0017](adr/0017-scenario-steps-are-read-directly-not-pi-prompts.md),
resolving the question this section used to leave open in `TODO.md`).

```
prompts/scenarios/triage/
  01-triage.md
  02-static.md
  03-dynamic.md
  04-report.md
```

Two frontmatter fields, hand-parsed rather than a full YAML parser: `title`
(the briefing's header) and an optional `toolset` (activated, additively, when
the agent reaches that step). Everything else — which tools just became
relevant, what not to start yet — is prose the step's author writes directly
into the body; nothing here parses it out.

## `themes/`

pi themes, as JSON.

Nothing here yet. A REactor theme is only worth adding if the selector and status
panels need colours the built-in themes do not provide — extensions should take
`theme` from the render callback and work under whatever the user has chosen, so
this is likely to stay empty.
