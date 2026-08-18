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

## Why a CLI, and why pi

pi's stated philosophy is skills and CLI tools over MCP servers, on the grounds
that a CLI composes: its stdout can be filtered by `jq`, `rg` or a shell script
*before* it becomes a tool result, so the unfiltered data never reaches the
model's context. An MCP tool call has no shell stage between return value and
context. REactor takes the same position, and takes it for its own surfaces too:
`reactor` is a CLI, and the TUI extensions shell out to it rather than
reimplementing its logic in TypeScript.

pi is also the only harness REactor targets. That is a deliberate narrowing —
see [ADR-0001](adr/0001-pi-is-the-only-target-harness.md).

## Architecture

```
      ┌──────────────────────────────────────────────────────────────┐
      │  this repo (pi package)                                      │
      │    tools.toml  toolsets.toml  bin/reactor                    │
      │    extensions/  skills/  prompts/  themes/                   │
      └───────────────┬──────────────────────────────────────────────┘
                      │  scripts/install.py
                      ▼
      ┌──────────────────────────────────────────────────────────────┐
      │  ~/.pi/reactor/                                              │
      │    tools.toml      ← live catalogue, user-editable           │
      │    toolsets.toml   ← live toolsets, user-editable            │
      │    state.json      ← which tools/toolsets are active         │
      │    skills/<tool>/  ← upstream skills, fetched at install     │
      └───────────────┬──────────────────────────────────────────────┘
                      │
        ┌─────────────┴──────────────┬───────────────────────┐
        ▼                            ▼                       ▼
   reactor CLI               tool-registry ext         selector ext
   doctor                    probe + cache             search / inspect
   tools list|show           before_agent_start        toggle tool/toolset
   skills list|show          → system prompt           → ctx.reload()
   install                   resources_discover
   diff-config               → gate skills/prompts     status ext
   overwrite-config          ctx.ui.setStatus          services, devices
```

Three things to notice about that diagram:

1. **The catalogue is the only source of truth.** The CLI reads it, the
   extensions read it through the CLI, the doctor reads it, the installer reads
   it. Adding a tool is one TOML block, not a code change in four places.
2. **Extensions never parse the catalogue themselves.** They shell out to
   `reactor ... --format json`. Same discipline the project applies everywhere
   else, and it means the TUI can never disagree with the CLI.
3. **The runtime catalogue lives in `~/.pi/reactor/`, not in the package.** The
   package copy is a seed. This matters because pi's update path does
   `git reset --hard` followed by `git clean -fdx` on the installed package —
   anything the user edited inside the package directory would be destroyed on
   every update.

## The four surfaces

### 1. The catalogue and the CLI

`tools.toml` describes each tool: identity, the one-line description that will
appear in the registry, how to detect it, how to get its version, whether it is
a service and how to check it, per-platform install recipes, and the upstream
skill source if there is one.

`reactor` is a stdlib-only Python CLI on PATH. `reactor doctor` reports what is
present and prints the install command for what is not, chosen for the platform
it is running on. `reactor install <tool>...` runs them. `reactor tools` and
`reactor skills` list and inspect. Every subcommand supports `--format json`,
which is how the extensions consume it.

### 2. The tool-registry extension

Probes the catalogue's entries, caches the result, and returns the rendered
registry from `before_agent_start`, where pi allows an extension to replace the
system prompt for that turn. Rendering is deterministic: if nothing about the
machine changed, the string is byte-identical to last turn's and the provider's
prompt cache is unaffected. It invalidates only when reality changed — a service
started, a device was plugged in, a tool was installed — which is exactly when
invalidation is worth paying for.

The same extension gates skill and prompt-template visibility through pi's
`resources_discover` event, so deactivating a toolset also removes its skills
from the system prompt, and `ctx.reload()` makes a toggle take effect
immediately.

Together with the selector, this pair is "the toolbox" — a `toolbox: false` in
pi's own agent directory (`reactor.json`, next to `settings.json`) removes both
from a session as if neither were loaded: no commands, no status line, nothing
injected. `/reactor-toolbox [on|off]` flips it from inside pi itself, since
pi's own `/settings` has no extension point for a third party's fields to
appear in ([ADR-0016](adr/0016-extension-toggles-live-in-their-own-pi-side-file.md)).

### 3. The selector and status extensions

The selector is the user's view of the same data: search the catalogue, inspect
an entry, read its skill if it has one, toggle individual tools and toolsets.
The status extension surfaces live state — which services are up, which devices
are attached — in the footer and a panel; individual services can be hidden
from it the same way, via `hiddenServices` in the same file.

Both are pure TUI over `reactor --format json`. Neither owns any logic.

### 4. Scenarios

A scenario is a chain of steps — Markdown files under `prompts/scenarios/<id>/`,
one per step, ordered by filename, read directly by `extensions/scenario/`
rather than through pi's own prompt-command machinery
([ADR-0017](adr/0017-scenario-steps-are-read-directly-not-pi-prompts.md)). The
agent advances by calling `reactor_step_complete(summary)`, and the tool's
*return content is the next step's briefing* — what to do now, what not to do
yet, which tools just became relevant. State rides in the tool result's
`details` field and in a `pi.appendEntry` record, both of which pi persists in
the session without ever sending them to the model, and both are restored on
`session_start` ([ADR-0009](adr/0009-scenarios-advance-by-tool-result.md)).

`/reactor-scenario list|start <id>|status|next [summary]|stop` is the human's
window onto the same state — `list` and `start` before the agent has anything
to advance, `next` as the manual override when the human, not the model, is
the better judge that a step is done.

Scenarios exist because an eager model finishes triage and immediately starts
patching. A briefing that says "do not start dynamic analysis yet" costs one
tool result and redirects it. Like the rest of REactor it persuades; it does not
enforce — a step may activate a toolset as it advances, but never deactivates
the one before it.

The first scenario ships with the package: `triage` — triage, static, dynamic,
report — the same four phases used as the illustrative example throughout this
document and in ADR-0009.

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
- **It is not portable to other harnesses.**
  ([ADR-0001](adr/0001-pi-is-the-only-target-harness.md))

## Target tool surface

Not all of these land at once; see `TODO.md` for phasing. The catalogue is meant
to grow to cover at least:

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
