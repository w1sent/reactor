# Gated tools advertise by state, and explain themselves when stale

A tool whose usefulness depends on session state — a goal set
(goal-setting's `update_steps`), a scenario running (`reactor_phase_complete`)
— is **registered once, always**, so its definition, description and backstop
message exist for the whole session. Its **advertisement** — presence in the
active tools list — follows its gate predicate, driven by the owning extension
with read-modify-write `setActiveTools` at the transitions of its own state. A
transition that does not change visibility writes nothing. The gate stays soft
([ADR-0007](0007-deactivation-is-soft.md)): a stale list that still carries the
tool degrades to the tool's own explanation, never to a block. And the two
gated tools do not share a noun: the scenario's stages are **phases**
(`reactor_phase_complete`, "Phase N/M" briefings), the manifest's durable
items are **steps** (`update_steps`).

This scopes the rejection in
[ADR-0017](0017-scenario-steps-are-read-directly-not-pi-prompts.md) and the
registration note in [ADR-0009](0009-scenarios-advance-by-tool-result.md):
dynamic tool visibility via `setActiveTools` was rejected there on the risk of
two writers racing on one shared active-tools list. Re-reading pi's loader
changed the facts, and the observed failure changed the stakes.

## Why

**The invitation is the failure.** Both tools were advertised in every state,
and agents reached for them: `update_steps` with no goal (the gate message
answers, but an agent cannot act on the teaching — the goal is the user's act,
so the lesson is wasted on its addressee), and `reactor_step_complete` as a
"final call" of `update_steps`, because both tools said *step*. Removing the
tool from the list removes the failure; the gate message remains for the one
case that can still produce it — a stale list.

**The mechanics are cheap, and they were re-measured.** `_refreshToolRegistry`
*preserves* the active list across reloads — it filters the previous names and
adds only newly registered tools — so an extension's write sticks and is not
clobbered by configuration. The writers (`/goal`, `/reactor-scenario start`,
the selector) are sequential user actions in one TUI thread, so
read-modify-write is a sequence, not a race. Each visibility transition
rebuilds the base system prompt once; goal transitions coincide with the
manifest block changing anyway, and scenario start/stop are a handful of
events per session. What ADR-0017 called a race is, measured, a sequence with
a small, rare cost.

**This is advertisement, not enforcement.** The tool stays registered and
callable; nothing blocks. Un-advertising an inactive tool is the same act as
deactivating an unused toolset — shaping what the agent is told so the list
carries no standing invitation to a dead end.

## Consequences

- **Each extension owns its tool's advertisement.** The write is always
  `getActiveTools()` → mutate → `setActiveTools()`, so every other writer's
  choices are preserved. The predicate that drives visibility is the same one
  the gate message answers — one function per extension, so visibility and
  backstop can never disagree.
- **No-op transitions are skipped.** Same visibility, no write, no prompt
  rebuild.
- **The active list changes a few times per session** — goal set/cleared,
  scenario started/stopped/finished — and each real change rebuilds the base
  system prompt once. That is the whole cache cost.
- **The scenario's persisted state keeps `stepIndex` and the
  `reactor-scenario` entry type.** Renaming stored fields would orphan
  in-flight sessions for zero model-facing benefit; the phase vocabulary is
  the model-facing surface only.
- **Both descriptions cross-reference**, so a model seeing both tools active
  can tell the manifest's steps from the scenario's phases.
- `tests/extensions/statusbar.test.mjs` and the per-extension tests drive
  every transition through the harness's active-tools recorder.

## Considered and rejected

- **Gate messages only (the status quo).** Keeps the invitation; the teaching
  is wasted on the agent, the wasted call is real, and the standing noise in
  the tools list is exactly what
  [ADR-0007](0007-deactivation-is-soft.md) says activation exists to remove.
- **Renaming `update_steps` instead.** The manifest's "steps" is the user's
  own language, the goal row's, and the glossary's; the scenario side yields.
- **A shared visibility manager owning the whole active list.** A second owner
  of every tool's state, and a cross-extension module holding coupling
  ADR-0014 forbids. Each extension writes at its own transitions instead.
- **Hiding at registration time.** Registration cannot know the session state;
  the sync runs on `session_start`, which is before any turn.