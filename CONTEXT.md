# REactor

A reverse-engineering agent harness in Rust: a `reactor` CLI and an agent loop
(over rig) with a native GUI, that tell an agent what RE tooling exists on the
machine it is running on and keep long sessions workable.

## Language

**REactor**:
The whole thing — this repository: the `reactor` CLI, the agent, and the GUI.
Capitalised "RE" is deliberate; it is not "Reactor".
_Avoid_: the harness, the framework, reactor-pi.

**Catalogue**:
`tools.toml` — the single source of truth for what tools REactor knows about:
identity, one-line description, detection probe, service probe, per-platform
install recipes, and the upstream skill source if the tool ships one. Shipped in
this repo, copied to `~/.reactor/tools.toml` at install time; the installed
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
their own copy at `~/.reactor/toolsets.toml`. Activation is purely about what
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
`~/.reactor/skills/<tool>/` rather than writing its own. The tool's author
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
→ registry block in the agent's system prompt. Each needs the one before it.
Used when talking about the core the rest assumes.
_Avoid_: core, base, foundation layer.

**Sibling repo**:
A standalone tool REactor develops but does not contain — its own repository,
its own release cycle, installed independently and referenced only by a
catalogue entry. They are cloned, released and installed independently,
and REactor assumes nothing about where they sit.
_Avoid_: submodule, vendored tool, subproject.

**Manifest**:
The session block `reactor-context` keeps in the system prompt: the user's goal,
the agent's self-maintained steps (each a conceptual summary with a 3-word
status), and the session guidelines. Injected on content only, so an untouched
session's prompt stays byte-identical. It survives every reduction — keeping it
current is how work outlives them.
_Avoid_: goal list, todo list, step list (the steps are one part of it), memory (too broad).

**Reduction**:
How the agent keeps a session inside the context window
([ADR-0037](docs/adr/0037-context-reduction-is-one-budget-manager.md)): one stand-in
message replaces a range of old entries, in mode `fade` (stubs only), `compact`
(a summary) or `auto`. The session log is never rewritten — a reduction is an
entry, and a `restore` entry undoes it.
_Avoid_: truncation, pruning, compaction (that is one mode).

**Identity**:
The working persona `reactor-context` injects into the system prompt: a built-in
(`reverse-engineer`, `cyber-forensics`, `forensics`, `software-engineer`,
`infrastructure`, `publisher`), a saved user identity, or an adhoc custom text. The
human selects it; the model never does ([ADR-0026](docs/adr/0026-identity-is-a-persona-block-in-the-system-prompt.md)).
_Avoid_: role, profile, mode.
