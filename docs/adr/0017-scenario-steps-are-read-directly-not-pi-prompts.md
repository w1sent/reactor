# Scenario steps are read directly from prompts/scenarios/, not surfaced as pi prompt commands

Resolves the question ADR-0009 and `TODO.md` left open: scenario definitions
are Markdown files with a two-field YAML-lite frontmatter (`title`, optional
`toolset`), one file per step, ordered by filename, grouped one directory per
scenario:

```
prompts/scenarios/triage/
  01-triage.md
  02-static.md
  03-dynamic.md
  04-report.md
```

```markdown
---
title: Static analysis
toolset: native
---
Disassemble or decompile. Map the interesting functions: entry point,
suspicious imports, anything that looks like C2 or persistence. Do not run
the sample yet.
```

`extensions/scenario/` reads these with `node:fs`, directly — **not** through
`resources_discover`'s `promptPaths`, so a step file never becomes an
individually-invokable `/01-triage` slash command the way a top-level
`prompts/*.md` template does.

## Why

**A raw step is not a useful thing to invoke on its own.** Advancing a
scenario is stateful — which step comes next depends on which one just
finished, tracked across the session. A bare pi prompt command has none of
that: typing `/02-static` would insert step 2's body as a user message with no
memory of step 1 having happened, no step-count header, and no
`reactor_step_complete` in play. Exposing steps as slash commands would create
a second, weaker way to "run" a scenario that looks like the real mechanism
and is not.

**Two fields is not enough to justify a new file format, but is enough to
justify not reusing pi's prompt frontmatter as-is.** A step needs a title (for
the "Step N/M: Title" header the tool result renders) and, sometimes, a
toolset to activate — pi's prompt frontmatter has `description` and
`argument-hint`, shaped for a command's help text and argument list, neither
of which fits. Everything else about a step — "newly relevant: bn, angr",
"do not start dynamic analysis yet" — is prose the scenario's author writes
directly into the body; the extension does not parse it out, because it has
nothing to do with it beyond showing it to the model.

**Reading the files directly keeps the state machine in one place.** The
extension already owns `reactor_step_complete` and needs to compute "what is
step N" to build a briefing; having it read the same files it will render from
is simpler than asking pi's resource loader to hand back paths and then
re-deriving structure pi already discarded (a `RegisteredCommand`, not the raw
frontmatter+body pair the renderer needs).

**The tool is registered once, always, whether or not a scenario is
running.** `pi.setActiveTools()` exists and could hide `reactor_step_complete`
between scenarios, but doing so means reading and rewriting the *global*
active-tools list from inside this extension — a shared, cross-cutting piece
of state other extensions and the user's own config also touch. A tool that
answers "no scenario is active, `/reactor-scenario start <id>`" when called
outside one costs a single line in the system prompt's "Available tools"
(`promptSnippet`) and no risk of clobbering someone else's active-tools
choice.

**Activating a step's toolset is additive only — nothing is auto-deactivated
when a scenario moves on.** `reactor toolsets enable <id>` runs, best-effort,
when a step names one; nothing runs `disable`. Steers, does not restrict
([ADR-0007](0007-deactivation-is-soft.md)): the previous step's toolset
disappearing on its own would narrow what the agent is told about in a way
nobody asked for, mid-task.

## Consequences

- **`resources_discover` is untouched by this extension.** Scenario steps are
  not gated by activation state the way skills and prompt templates are —
  `docs/package-resources.md`'s original sketch said they would be; this
  supersedes that. A scenario's steps are visible regardless of which
  toolsets are active, same as the catalogue always lists every tool in
  `doctor` even when deactivated.
- **No `ctx.reload()` after activating a step's toolset.** The registry block
  itself is fresh every turn regardless (it re-reads `state.json` on every
  `before_agent_start`), so the tool list catches up on its own. Only
  `resources_discover`-gated skills would lag until the next reload; accepted,
  since most catalogued tools have no fetched skill to begin with.
- **State persistence follows ADR-0009 as written**: `pi.appendEntry("reactor-scenario",
  state)` on every transition, restored on `session_start` by taking the
  *last* matching entry from `ctx.sessionManager.getEntries()` — including a
  `stop`, which persists `undefined` and correctly restores to "no scenario
  active".
- **One scenario at a time, per session.** `start` while one is already
  running is refused rather than replacing it silently; `stop` first.
- **A hand-authored scenario needs no schema change to add a step** — drop a
  new numbered file in the directory. Renumbering to insert a step in the
  middle is a manual file rename, same cost as reordering toolset members in
  `toolsets.toml`.
- **`REACTOR_SCENARIOS_DIR` overrides where `extensions/scenario/` looks**,
  mirroring `REACTOR_CONFIG_DIR`. Primarily so tests use a throwaway scenario
  instead of coupling to this package's own shipped prose; incidentally also
  lets a person point at their own scenarios without editing the installed
  package. No seed step or `diff-config` story for it, unlike `tools.toml` —
  out of scope until someone other than a test wants one.

## Considered and rejected

- **Plain pi prompt templates, exposed via `resources_discover`'s
  `promptPaths`**, matching `docs/package-resources.md`'s original sketch.
  Rejected per the "not a useful thing to invoke on its own" point above — it
  would ship a command that looks like it runs a scenario step and does not
  track any state.
- **A richer per-scenario manifest** (one YAML/TOML file per scenario naming
  its steps, order, and toolsets, mirroring `toolsets.toml`). Rejected as more
  structure than two fields need; filename order and one frontmatter field are
  already unambiguous, and a manifest is one more file to keep in sync with
  the step files it describes.
- **Dynamic tool visibility via `pi.setActiveTools()`**, registering
  `reactor_step_complete` only while a scenario runs. Rejected: it requires
  reading and rewriting the extension-wide active-tools list, which several
  things can already touch (the selector, direct user config), and a single
  always-present tool that explains itself when idle costs less than the risk
  of two writers racing on one shared list.
- **Auto-deactivating the previous step's toolset when a new one activates.**
  Rejected as enforcement dressed as tidiness — the point of activation is to
  advertise, and un-advertising something the agent may still need (a static
  finding worth re-checking mid-dynamic-analysis, say) is a narrowing nobody
  asked for.
