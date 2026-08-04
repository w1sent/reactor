# The selector edits overrides, not outcomes

Activation is not stored. It is *derived*: a base set (everything, or the union
of the active toolsets) minus `tools.disabled` plus `tools.enabled`. A tool that
is on can be on for two unrelated reasons, and the file records only the second.

The selector shows the derived answer — a checkbox per tool — but writes the
override. So a toggle is not "set this tool to on"; it is **"make the smallest
edit to `state.json` that produces on"**. Concretely:

- turning a tool on that the base already includes clears any `disabled` entry
  and writes **nothing** else;
- turning it off when the base excludes it clears any `enabled` entry and writes
  nothing else;
- only a disagreement with the base produces a stored override.

`reactor tools enable|disable` behave this way too — the selector cannot have
semantics of its own ([ADR-0005](0005-reactor-cli-stdlib-python.md)) — and
`reactor tools reset <id>` drops an override without asserting an outcome.
`describe()` gains an `override` field (`"on"`, `"off"`, or `null`) so a client
can say *why* a tool is active without re-deriving anything.

## Why

Before this, `enable` always appended to `enabled` and `disable` always appended
to `disabled`. Through a CLI that is a wart: `disable X` then `enable X` leaves
an explicit entry where there was none, and there was no way back to the clean
state. Through a selector it is a defect, because a selector invites toggling.
Ten idle keystrokes leave ten pins, and the file stops meaning what it says: the
next toolset switch quietly does nothing to any of those tools, because each one
now outranks the base.

Minimal writes make the round trip an identity. Toggle a tool off and on again
and `state.json` is byte-identical to before — which is the property that lets
someone use the selector to *look* without committing to anything.

It also keeps the file readable by a human, which matters because it is a hint
file, not a database ([ADR-0007](0007-deactivation-is-soft.md)). An override
list containing only genuine deviations from the toolset is a short list, and a
short list can be read and understood.

## Consequences

- **An override is forgotten when the base catches up with it.** Pin `frida` on
  while no toolset is selected — base is everything, so nothing is written —
  then activate `native`, and `frida` goes off. The user's earlier click did not
  survive, because it never asserted anything the base did not already say. This
  is the cost, and it is why `reset` exists as the explicit third verb rather
  than a third state in the UI.
- The UI needs both facts per tool: `active` (what the agent is told) and
  `override` (what the file says). Showing only the first makes a toolset switch
  look arbitrary.
- Existing `state.json` files stay valid. Redundant entries already written are
  harmless — they are cleaned up the next time that tool is toggled.
- The selector is a pure client: it reads `--format json`, it mutates by
  spawning `reactor`, and it holds no state that outlives the overlay. A crashed
  overlay cannot corrupt anything.
- Every mutation is followed by `ctx.reload()`, which re-runs
  `resources_discover` and so re-gates skills. The registry block needs no
  invalidation: activation is not cached, it is recomputed from `state.json` on
  every call.

## Considered and rejected

- **Keep pinning; make the selector a three-state control** (`auto` / `on` /
  `off`, cycled with the same key). This is the most honest mapping onto the
  data model, and it preserves an intent that minimal writes discard. Rejected
  because a three-state checkbox is a bad control — it cannot be driven by
  space-to-toggle, the third state has no natural glyph, and the state it adds
  is one users would have to be taught the storage model to predict. `reset` on
  a separate key gives the same power to the person who wants it, at no cost to
  the person who does not.
- **Let the selector write `state.json` directly.** It is one small JSON file and
  the extension already knows its path from `reactor state`. Rejected outright:
  the derivation, the toolset union, and the tag semantics would then exist in
  two languages, which is exactly what
  [ADR-0005](0005-reactor-cli-stdlib-python.md) forbids. The write is not the
  hard part — knowing what to write is.
- **Reuse pi's `SettingsList` for the toggle screen.** It is close: a label, a
  right-hand value, `values` cycling on Enter/Space, `enableSearch`, and Esc to
  cancel. Rejected on three specifics. Its fuzzy search matches `label` only, so
  finding a tool by what it *does* — the thing a catalogue is for — would need
  the description folded into the label, which its column padding then breaks.
  An item may have `values` **or** a `submenu`, never both, so Enter cannot both
  toggle and inspect. And the presence/absence of a tool has no place to live:
  the value column is the checkbox. REactor's list renders `[x]`, an id, a
  description and a probe result on one row, and filters over all of them.
- **Put the detail view in the transcript with `pi.sendMessage`.** Rejected:
  `convertToLlm` maps a custom message to a **user** message, so every inspected
  tool would be paid for twice — once in the registry block, once as a wall of
  install candidates the model did not ask for. Detail is for the human, so it
  is an out-of-context session entry (`pi.appendEntry` plus an entry renderer).
  The skill body, being long and genuinely a document, is the same call for the
  same reason.
- **One command for everything (`/reactor select tools|toolsets`).** Rejected as
  undiscoverable: nobody types an argument they have not seen. Tools and
  toolsets are two panes of one overlay, switched with Tab, opened by one
  command.
