# REactor is a Rust project on rig; pi becomes a client, not the host

REactor's harness is rewritten in Rust over [rig](https://rig.rs/), with
`reactor-gui` as its frontend. pi stops being the thing REactor is built
*inside* and becomes one of several harnesses that can consume the `reactor`
CLI from outside.

This reverses [ADR-0001](0001-pi-is-the-only-target-harness.md), which said pi
was the only target and portability was not a design constraint. That was
correct while the whole product was a pi package. It stops being correct once
the features REactor wants most are the ones a host harness will not give it.

## Why

**The four hardest things REactor does are all fights with a host that owns
the message list.** [ADR-0020](0020-rolling-context-measures-and-cuts-like-pi-does.md)
has the fade *cancelling* pi's own threshold compaction, because two
mechanisms that cannot coordinate will contradict each other.
[ADR-0021](0021-context-editor-forks-or-filters-never-rewrites.md) forbids
context-editor from rewriting history, because pi owns the session file.
[ADR-0025](0025-auto-continue-continues-after-automatic-compaction.md) is 220
lines of reasoning about which of pi's three `_checkCompaction` cases leave a
turn parked. [ADR-0032](0032-the-gui-extends-pis-rpc-through-existing-channels-only.md)
smuggles structured UI through line 0 of a string widget because the RPC
protocol carries nothing else. Every one of those is a workaround whose cost
is permanent and whose ceiling is somebody else's release cadence. Owning the
loop deletes all four problems rather than managing them.

**The half of REactor that is worth building next cannot be expressed in a
terminal.** Hand-curating what the model sees, previewing what a context
reduction is about to discard, restoring a summarized range, navigating the
session tree — these are the reason `reactor-gui` exists
([ADR-0031](0031-reactor-gui-lives-in-the-repo-and-installs-with-it.md)), and
a TUI is not a worse rendering of them, it is an impossible one. Other
projects build good TUIs; REactor does not need to be one of them.

**rig is a library, not a harness, and that is the accepted cost.** It gives
providers, a completion and agent builder, typed tools and structured output.
It gives no session store, no context management, no resource model and no UI.
Those have to be written. They are written anyway in any design that owns the
loop, and writing them against rig is strictly less work than writing them
against nothing.

**One language, and the frontend already picked it.** `reactor-gui` is 8,646
lines of Rust talking to pi over JSONL. Moving the loop into the same process
removes a protocol, a serialization boundary and a child process.

## Non-goals

Scope is defined as much by this list as by the one above. None of these are
built, and a feature request that requires one is refused rather than
accommodated:

- **No TUI.** Stated above. `pi` in a terminal remains a supported way to run
  the toolbox half ([ADR-0035](0035-portable-surface-is-machine-facts.md)).
- **No extension loader and no third-party extension API.** REactor is the only
  consumer of its own behaviours; they are compiled in. This removes
  `ctx.reload()` and its staleness class, the load-order question
  [ADR-0024](0024-rolling-context-splits-into-goal-setting-history-tools-and-the-fade.md)
  had to reason about, [ADR-0014](0014-extensions-share-the-cache-not-each-other.md)'s
  prohibition, and [ADR-0016](0016-extension-toggles-live-in-their-own-pi-side-file.md)'s
  per-extension toggle files.
- **No RPC mode.** The GUI is in-process.
- **No themes or prompt templates as a discovered resource type.** The GUI
  themes itself; [ADR-0017](0017-scenario-steps-are-read-directly-not-pi-prompts.md)
  already reads scenario steps directly.
- **No generic slash-command registry.** GUI actions, and a palette if one is
  wanted.
- **No permission, approval or sandbox model.**
  [ADR-0007](0007-deactivation-is-soft.md) argues that enforcement over an
  agent that has bash unconditionally is theatre; that argument does not
  weaken when moved from *what the agent is told* to *what the agent runs*.
  REactor cannot build a containment boundary that holds, so it does not
  pretend to: the documented posture is that an RE agent is run inside a VM or
  container, and no feature may be designed as if containment existed at the
  harness level.

## Consequences

- **The state root moves from `~/.pi/reactor/` to `~/.reactor/`.**
  [ADR-0001](0001-pi-is-the-only-target-harness.md) put it under pi's tree
  precisely because pi was the only target; with that premise gone the reason
  goes with it, and a root named after the harness that no longer hosts us is
  actively misleading.
- **The pi package splits into a maintained half and a frozen half.**
  `tool-registry`, `selector`, `status` and `scenario` already shell out to the
  CLI and keep working against the Rust binary unchanged. `goal-setting`,
  `identity`, `reporting`, `rolling-context`, `auto-continue`,
  `context-editor` and `history-tools` keep working as they are today but stop
  receiving features; their Rust counterparts are not required to match them.
  See [ADR-0035](0035-portable-surface-is-machine-facts.md).
- **[ADR-0012](0012-extensions-tested-through-pi-s-own-loader.md) narrows to
  the pi flavor.** The 281 extension tests keep testing what they test; they
  simply stop being the project's main test surface.
- **The catalogue is untouched.** `tools.toml`, `toolsets.toml`, the probes and
  the install recipes are facts about external programs
  ([ADR-0003](0003-tools-toml-single-source-of-truth.md),
  [ADR-0010](0010-install-recipes-keyed-by-package-manager.md)) and carry over
  verbatim.
- **Provider choice widens for free.** rig speaks to 20+ providers behind one
  API, where pi's model registry was whatever pi supported.

## Considered and rejected

- **Stay on pi and keep working around it.** Rejected: the four workarounds
  above are not a backlog that shrinks. Each new feature in the workflow half
  adds another, and [ADR-0032](0032-the-gui-extends-pis-rpc-through-existing-channels-only.md)
  already records that there is no upstream path for the protocol changes
  wanted.
- **Fork pi.** Rejected: it buys the same ownership at the price of a
  TypeScript codebase nobody here wrote, plus a permanent merge burden against
  a project moving faster than the fork would.
- **Keep the GUI as a permanent pi RPC client.** Rejected: it is the current
  design, and [ADR-0032](0032-the-gui-extends-pis-rpc-through-existing-channels-only.md)
  is the evidence for its ceiling — every capability has to be smuggled through
  a channel that already exists.
- **Write the loop from scratch, no rig.** Rejected: provider plumbing,
  streaming, tool-call serialization and structured output are solved, boring,
  and easy to get subtly wrong. rig is the part of this there is no reason to
  own.
- **Port onto a different Rust agent framework, or wait for a Rust harness to
  mature.** Rejected: a *harness* would reintroduce exactly the ownership
  problem this ADR exists to solve. The requirement is a library, and rig is
  the mature one.
