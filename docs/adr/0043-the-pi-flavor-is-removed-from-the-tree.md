# The pi flavor is removed from the tree

[ADR-0035](0035-portable-surface-is-machine-facts.md) kept two pi packages alive: four
maintained extensions over the CLI, seven frozen ones. With the GUI on the Rust agent
([ADR-0042](0042-the-gui-hosts-the-agent-in-process.md)) nothing in the project runs on
pi, and the pi half was only costing attention: a Node toolchain in a Rust repo, a
second catalogue entry and skill for pi itself, and docs describing two products.

## Decision

**The pi flavor is deleted, not published.** `extensions/`, its test suite, `package.json`,
the pi smoke scripts, the pi API notes and the pi resource docs are removed. Anyone who
wants the pi version checks out commit `014a9b8`, the last one that has it, and installs
from there. The `reactor` CLI's `--format json` stays frozen, so an old pi checkout keeps
working against a new binary.

This retires the "publish and pin the pi shims" item of
[MIGRATE.md](../../MIGRATE.md) phase 5.

**Catalogue: pi is no longer an entry.** `pi-subagent`, the `agent` toolset and
`skills/pi-subagent/` go — they told the agent to delegate to a harness it is no longer
running in ([ADR-0018](0018-pi-itself-is-a-catalogue-entry-non-interactive-only.md),
[ADR-0022](0022-pi-subagent-gets-a-skill-so-it-gets-a-clearer-id.md) are history).

**The GUI crates live beside the others.** `gui/crates/*` → `crates/*`; the GUI spec sits
with its crate.

**Kept.** ADRs 0001–0032 as the record of the pi period; the one-time
`~/.pi/reactor` → `~/.reactor` state move (existing users need it); the goldens under
`crates/reactor-context/tests/golden/`, which become plain fixtures. Regenerating them
means running the capture script from `014a9b8`.

## Consequences

- The Rust suite is the only suite. The extension-loader tests (ADR-0012) and the
  golden `--check` no longer run in CI.
- Changing what a ported block says now means editing the golden on purpose.
