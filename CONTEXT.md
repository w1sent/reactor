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
as a catalogue entry.
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
machine does not have. Notes are always shown and never executed.
_Avoid_: instruction, hint, fallback (for either).

**Upstream skill**:
An Agent Skill written by a tool's own author (`bn`'s skill in the plugins repo,
`ipsw-skill`) that REactor fetches at install time into
`~/.pi/reactor/skills/<tool>/` rather than writing its own. The tool's author
knows the tool best. A configured-but-unfetchable upstream skill is a warning; a
tool with no upstream skill at all is normal and simply relies on `--help`.
_Avoid_: vendored skill, external skill, third-party doc.

**Scenario**:
A multi-step analysis workflow expressed as prompt templates — triage, then
static, then dynamic, then report. The agent advances by calling
`reactor_step_complete`, whose *return value is the next step's briefing*.
Scenarios steer the agent's sequencing; like everything else in REactor they
persuade rather than enforce.
_Avoid_: workflow, pipeline, playbook, state machine.

**Step briefing**:
The text returned by `reactor_step_complete` — what the next step is, what it
should not do yet, and which tools and skills just became relevant. It is a tool
result, not an injected message, so it costs nothing extra in context.
_Avoid_: stage prompt, phase instruction.

**Spine**:
The non-optional chain every other feature builds on: catalogue → `reactor` CLI
→ tool-registry extension. Each needs the one before it. Used when talking about
milestone scope.
_Avoid_: core, base, foundation layer.

**Sibling repo**:
A standalone tool REactor develops but does not contain — its own repository,
its own release cycle, installed independently and referenced only by a
catalogue entry. They are cloned, released and installed independently,
and REactor assumes nothing about where they sit.
_Avoid_: submodule, vendored tool, subproject.
