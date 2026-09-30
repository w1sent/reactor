# REactor — the idea

## The problem

An agent doing reverse engineering on a real workstation is surrounded by
capability it cannot see. Binary Ninja may be open with the target already
loaded. `frida` may be installed and a phone may be attached over ADB. `joern`
may be sitting there with a CPG ready to query. The model knows all these tools
*exist in the world* — that knowledge is in its weights — but it has no way to
know which of them are on **this** machine, at **this** moment, and reachable.

So it does the predictable thing: it reaches for `objdump`, `strings` and a
Python script, because those are the tools it can be confident are present. The
expensive, specialised tooling sits unused a few characters away.

The naive fix is to write documentation for every tool and put it in the
context. That fails twice over. It costs context permanently, and it goes stale —
a hand-written summary of `frida`'s options is worse than `frida --help` the
moment `frida` is updated, and it was never better than `--help` to begin with.

## The inversion

REactor's core claim is that **tool usage does not need documenting, but tool
presence does**.

- *How* a tool works is already documented, by its author, in `--help`, `man`,
  and — increasingly — in an Agent Skill the author ships themselves. An agent
  can read all of that on demand, at the moment it matters, at zero standing
  cost.
- *That* a tool is here, usable right now, and what it is for in one line — that
  is machine-specific, time-varying, and knowable by nobody but the machine.

So REactor probes the machine and injects a compact registry of what is actually
present and active:

```
## Available RE tools (this machine)
bn      Binary Ninja RE framework       [BN running, 1 binary open]
frida   dynamic instrumentation         17.2
jadx    Android/Java decompiler
adb     Android device bridge           [2 devices]
joern   code property graph / dataflow
lief    ELF/PE/Mach-O parsing (python)

Use `<tool> --help` for usage. `reactor tools` for detail.
```

Six lines and a pointer. The agent now reaches for `bn` instead of `objdump`,
and looks up `bn --help` when it needs to know how. That is the entire core
mechanism; everything else in REactor is either feeding that block or letting
the user shape it.

This is the same reasoning [ADR-0038 in the plugins
repo](https://github.com/w1sent/bn-plugins/blob/main/docs/adr/0038-binja-cli-skill-frontend.md) already applied to
`bn` — "a lean SKILL.md covers orientation; `bn --help` is the actual
progressive-disclosure mechanism" — generalised from one tool to the whole
toolbox.

## Why a CLI

A CLI composes: its stdout can be filtered by `jq`, `rg` or a shell script
*before* it becomes a tool result, so the unfiltered data never reaches the
model's context. An MCP tool call has no shell stage between return value and
context. REactor takes that position for its own surfaces too: `reactor` is a
CLI first, and the agent and the GUI use the same library the binary is built on
([ADR-0034](adr/0034-reactor-cli-becomes-a-rust-library-with-a-binary.md)).

REactor began as a package for the [pi](https://pi.dev) harness
([ADR-0001](adr/0001-pi-is-the-only-target-harness.md)) and now owns its harness
([ADR-0033](adr/0033-reactor-is-a-rust-project-on-rig.md)). The pi flavor is in git
history (last at commit `014a9b8`).

## Architecture

```
      ┌──────────────────────────────────────────────────────────────┐
      │  this repo                                                   │
      │    tools.toml  toolsets.toml   (shipped seeds)               │
      │    crates/  skills/  prompts/                                │
      └───────────────┬──────────────────────────────────────────────┘
                      │  reactor setup
                      ▼
      ┌──────────────────────────────────────────────────────────────┐
      │  ~/.reactor/                                                 │
      │    tools.toml      ← live catalogue, user-editable           │
      │    toolsets.toml   ← live toolsets, user-editable            │
      │    state.json      ← which tools/toolsets are active         │
      │    settings.json   ← models, context budget, identity        │
      │    skills/<tool>/  ← upstream skills, fetched at install     │
      │    sessions/<id>/  ← session store (+ per-session overrides) │
      └───────────────┬──────────────────────────────────────────────┘
                      │
        ┌─────────────┴──────────────┬───────────────────────┐
        ▼                            ▼                       ▼
   reactor-cli               reactor-agent             reactor-gui
   doctor, tools, skills     loop over rig, session    panels, transcript,
   install, diff-config      store, context budget,    context and tool
   --format json             registry in the prompt    controls
        └──────── all three build on reactor-core ───────────┘
```

Three things to notice about that diagram:

1. **The catalogue is the only source of truth.** The CLI, the agent's registry,
   the doctor, the installer and the GUI panels all read it through
   `reactor-core`. Adding a tool is one TOML block, not a code change in four
   places.
2. **Nothing else parses the catalogue.** The GUI and agent call the library (or
   `reactor ... --format json`, whose shape is frozen), so no surface can
   disagree with the CLI.
3. **The runtime catalogue lives in `~/.reactor/`, not in the checkout.** The
   repo copy is a seed and is compiled into the binary; a reinstall never
   destroys the user's edits.

## The surfaces

### 1. The catalogue and the CLI

`tools.toml` describes each tool: identity, the one-line description that will
appear in the registry, how to detect it, how to get its version, whether it is
a service and how to check it, per-platform install recipes, and the upstream
skill source if there is one.

`reactor doctor` reports what is present and prints the install command for what
is not, chosen for the platform it is running on. `reactor install <tool>...`
runs them. `reactor tools` and `reactor skills` list and inspect. Every
subcommand supports `--format json`.

### 2. The registry

The agent probes the catalogue's entries, caches the result, and puts the
rendered registry in the system prompt. Rendering is deterministic: if nothing
about the machine changed, the string is byte-identical to last turn's and the
provider's prompt cache is unaffected. It invalidates only when reality changed —
a service started, a device was plugged in, a tool was installed — which is
exactly when invalidation is worth paying for. Deactivating a toolset also
removes its skills from the prompt.

Activation is scoped: a toggle in a session applies to that session, and can be
promoted to the machine default or dropped
([ADR-0042](adr/0042-the-gui-hosts-the-agent-in-process.md)).

### 3. The agent and its session

`reactor-agent` is the loop: an append-only session store you can branch and
resume, calls to the model through rig, and a context budget that replaces old
ranges with a stand-in message before the window fills — with preview and undo
([ADR-0036](adr/0036-reactor-owns-its-session-store-format.md),
[ADR-0037](adr/0037-context-reduction-is-one-budget-manager.md),
[ADR-0041](adr/0041-the-agent-loop-owns-the-message-list.md)). `reactor-context`
supplies the session blocks: the manifest (goal, steps, guidelines), the working
identity, reporting, and scenarios.

### 4. Scenarios

A scenario is a chain of phases — Markdown files under `prompts/scenarios/<id>/`,
one per phase, ordered by filename. The agent advances by calling
`reactor_phase_complete(summary)`, and the tool's *return content is the next
phase's briefing* — what to do now, what not to do yet, which tools just became
relevant. Phases are the scenario's own stages, distinct from the manifest's
steps. State rides in the session store and is restored on resume
([ADR-0009](adr/0009-scenarios-advance-by-tool-result.md)).

Scenarios exist because an eager model finishes triage and immediately starts
patching. A briefing that says "do not start dynamic analysis yet" costs one
tool result and redirects it. Like the rest of REactor it persuades; it does not
enforce — a step may activate a toolset as it advances, but never deactivates
the one before it.

The first scenario ships with the repo: `investigation` — seventeen stages
covering the full arc from scoping and evidence acquisition through triage,
static, dynamic and deep analysis, timeline, detection, reporting and
remediation to analysis-derived tooling.

## What REactor deliberately does not do

- **It does not enforce anything.** Deactivating a tool hides it from the
  registry; it does not block execution. The agent has bash, always, and a tool
  that cannot run reports that itself — `bn` errors when no Binary Ninja session
  is open, which is a better error than any guard we could write.
  ([ADR-0007](adr/0007-deactivation-is-soft.md))
- **It does not write tool documentation.** `--help` and upstream skills.
  ([ADR-0008](adr/0008-aggregate-upstream-skills.md))
- **It does not build or vendor tools.** Sibling repos, independently released.
  ([ADR-0002](adr/0002-package-ships-assets-tools-are-sibling-repos.md))
- **It does not merge config for you.** `reactor diff-config` shells out to
  `diff(1)`; you fix it by hand.
  ([ADR-0004](adr/0004-config-updates-via-plain-diff.md))
- **It does not host other harnesses.** The `reactor` CLI is the portable
  surface: machine facts, `--format json`
  ([ADR-0035](adr/0035-portable-surface-is-machine-facts.md)).

## Target tool surface

Not all of these land at once — the catalogue grows as real analysis needs it,
not by plan. It is meant to cover at least:

| Area | Tools |
|---|---|
| Static, native | Binary Ninja (`bn`), angr, joern, LIEF, otool (macOS) / llvm-otool (elsewhere) |
| Static, managed | jadx, ilspycmd |
| Dynamic | frida, QBDI, objection |
| Debugging | lldb, gdb, rr |
| Mobile | adb, apktool, aapt2, ipsw, objection, mobilecli |
| Firmware / carving | binwalk, LIEF |
| Pattern matching | YARA |
| Network | tshark/Wireshark, scapy |
| Databases | usql, redis-cli, mongosh, cqlsh, influx (Influx CLI) |
| Solving | z3, angr |
| Parsing / transformation | tree-sitter |

Some of these get nothing but a catalogue entry — a name, a line of description,
and `--help`. That is the expected outcome for most of them, and it is not a gap.

Where a decompiler has both a GUI and a CLI, the CLI is the entry: `ilspycmd`
rather than ILSpy. The agent invokes commands, so a tool it cannot invoke is not
a tool as far as the catalogue is concerned.

### What is not a catalogue entry

**Binary Ninja plugins.** blutter is the example: it is a plugin, so it has no
binary to detect, no `--help` to read and no install recipe that means anything
outside a BN installation. Cataloguing it would put an entry in every system
prompt for something the agent cannot invoke.

Plugins reach the agent through `bn` instead — they are shipped from the
`bn-plugins` repository, which is also where REactor fetches the `bn` skill from
([ADR-0008](adr/0008-aggregate-upstream-skills.md)). Knowledge about what a
plugin does and when to reach for it belongs in that skill or in a reference
beside it, written by the people who ship the plugin, and it arrives already
scoped to a machine that has Binary Ninja.

The general rule: REactor catalogues things with an interface of their own. A
thing that only exists inside another tool is that tool's business
([ADR-0002](adr/0002-package-ships-assets-tools-are-sibling-repos.md)).

**GUI-only programs.** ImHex is the example. It has a binary, so unlike a plugin
it *can* be detected — but `imhex --version` prints nothing, `--help` prints a
screen of blank lines, and passing it an argument makes it try to open a dialog.
It cannot be scripted.

It was catalogued briefly, on the theory that "this machine has ImHex" is worth
knowing and one honest line is cheap. That was wrong, and the test that shows
why is the one at the top of this document: the registry exists so the agent
reaches for the right tool instead of `objdump`. A tool the agent cannot invoke
can never be the thing it reaches for, so the line buys nothing and costs a line
in every system prompt. Its real audience is the human sitting in front of the
machine, and they already know what they installed.

So: having a binary is necessary but not sufficient. The entry has to be
something the agent can *run*.
