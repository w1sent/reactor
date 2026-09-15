# REactor

A reverse-engineering agent harness built as a pi package: extensions, skills,
prompt templates, themes and a `reactor` CLI that tell an agent what RE tooling
exists on the machine it is running on.

## Language

**REactor**:
The whole thing — this repository, installed as one pi package plus one CLI on
PATH. Capitalised "RE" is deliberate; it is not "Reactor".
_Avoid_: the harness, the framework, reactor-pi.

**Catalogue**:
`tools.toml` — the single source of truth for what tools REactor knows about:
identity, one-line description, detection probe, service probe, per-platform
install recipes, and the upstream skill source if the tool ships one. Shipped in
this repo, copied to `~/.pi/reactor/tools.toml` at install time; the installed
copy is the one everything reads at runtime.
_Avoid_: registry (that is the injected block), manifest, tool database, index.

**Tool**:
One catalogue entry — an external binary or library REactor can detect, describe
and install. `bn`, `frida`, `jadx`, `yara`, `rg`. A tool is not written by us;
even the ones we do write live in their own repositories and enter REactor only
as a catalogue entry. Every entry declares its `source` — the tool's true
upstream, https only — which is what the manual install paths are trusted
relative to ([ADR-0028](docs/adr/0028-every-tool-declares-its-source.md)).
_Avoid_: integration, plugin, backend.

**Toolset**:
A named group of tools the user can activate or deactivate as a unit — `triage`,
`android`, `firmware`. Predefined ones ship in `toolsets.toml`; the user edits
their own copy at `~/.pi/reactor/toolsets.toml`. Activation is purely about what
gets advertised to the agent; it never blocks anything.
_Avoid_: profile, preset, bundle, workspace.

**Registry**:
The compact block REactor injects into the system prompt each turn: one line per
*present and active* tool — name, one-line description, how to invoke it, and
live service state where applicable. Rendered deterministically so that an
unchanged machine produces byte-identical text and the prompt cache holds.
_Avoid_: context block, tool list, inventory, preamble.

**Probe**:
The command that answers one question about a tool. A *detection* probe answers
"is it installed"; a *version* probe answers "which version"; a *service* probe
answers "is the thing it talks to actually running" (e.g. `bn health` against a
live Binary Ninja session). Probes are cached; only service probes are re-run on
a timer.
_Avoid_: check, health check, test, ping.

**Manager**:
A package manager declared in `[platform.manager]` — `pacman`, `brew`, `uv`,
`cargo`. Install recipes are keyed by manager, never by distribution, so that a
recipe key is a testable predicate: the manager's binary is on `PATH` or it is
not. Ranked by `[platform].prefer`, which the machine's owner edits.
_Avoid_: platform, distro, backend, provider.

**Recipe** / **note**:
A *recipe* is an install command whose manager REactor has verified is present —
the only kind `reactor install` will ever run. A *note* is any other value in an
`[tool.*.install]` table: `manual`, a release URL, a command for a manager this
machine does not have. Notes are always shown and never executed — with one
deliberate exception: `manual-install-oneliner`, a runnable manual path that
`--auto-install-manual` (fallback) and `--force-install-manual` (override)
execute, promoting the result into the user-local PATH
([ADR-0027](docs/adr/0027-manual-install-oneliner-is-executed-only-under-explicit-flags.md)).
_Avoid_: instruction, hint, fallback (for either).

**Upstream skill**:
An Agent Skill written by a tool's own author (`bn`'s skill in the plugins repo,
`ipsw-skill`) that REactor fetches at install time into
`~/.pi/reactor/skills/<tool>/` rather than writing its own. The tool's author
knows the tool best. A configured-but-unfetchable upstream skill is a warning; a
tool with no upstream skill at all is normal and simply relies on `--help`.
_Avoid_: vendored skill, external skill, third-party doc.

**Scenario**:
A multi-phase analysis workflow expressed as prompt templates — the shipped
`investigation` scenario runs from scoping and evidence acquisition through
triage, analysis, timeline, detection and reporting to lessons learned and
analysis-derived tooling. The agent advances by calling
`reactor_phase_complete`, whose *return value is the next phase's briefing*.
Scenarios steer the agent's sequencing; like everything else in REactor they
persuade rather than enforce. A scenario's stages are *phases* — distinct from
the manifest's *steps* (see goal-setting).
_Avoid_: workflow, pipeline, playbook, state machine.

**Phase briefing**:
The text returned by `reactor_phase_complete` — what the next phase is, what it
should not do yet, and which tools and skills just became relevant. It is a tool
result, not an injected message, so it costs nothing extra in context.
_Avoid_: stage prompt, step prompt.

**Spine**:
The non-optional chain every other feature builds on: catalogue → `reactor` CLI
→ tool-registry extension. Each needs the one before it. Used when talking about
the core the rest of the package assumes.
_Avoid_: core, base, foundation layer.

**Sibling repo**:
A standalone tool REactor develops but does not contain — its own repository,
its own release cycle, installed independently and referenced only by a
catalogue entry. They are cloned, released and installed independently,
and REactor assumes nothing about where they sit.
_Avoid_: submodule, vendored tool, subproject.

**Manifest**:
The session block `goal-setting/` keeps in the system prompt: the user's goal,
the agent's self-maintained steps (each a conceptual summary with a 3-word
status), and the session guidelines. Injected on content only, so an untouched
session's prompt stays byte-identical. It is what survives both pi's
compaction and the fade — keeping it current is how work outlives either.
_Avoid_: goal list, todo list, step list (the steps are one part of it), memory (too broad).

**Fade**:
`rolling-context/`'s alternative to pi's summarization compaction: only the
newest messages that fit a configurable budget go to the model; older ones are
left out of the next request only — the session file is untouched, and the
history tools recover what was faded. Measures and cuts the same way pi's own
compaction does ([ADR-0020](docs/adr/0020-rolling-context-measures-and-cuts-like-pi-does.md)).
_Avoid_: truncation (that is the hard-boundary archive path, a last resort), summary (the fade summarizes nothing), context pruning.

**History tools**:
`history_index` / `history_search` / `history_read` — line-addressed recovery
over the serialized session history, on by default in any session, independent
of the fade. A recovery path, not a browsing habit; the fade's guidance is
where that economics lives.
_Avoid_: search tools, replay, rollback.

**Identity**:
The working persona `identity/` injects into the system prompt: a built-in
(`reverse-engineer`, `cyber-forensics`, `forensics`, `software-engineer`,
`infrastructure`, `publisher`), a saved user identity, or an adhoc custom text. The
human selects it; the model never does ([ADR-0026](docs/adr/0026-identity-is-a-persona-block-in-the-system-prompt.md)).
_Avoid_: role, profile, mode.
