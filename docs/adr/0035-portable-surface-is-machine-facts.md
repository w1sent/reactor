# The portable surface is machine facts; session state is REactor-native

The `reactor` CLI is the contract other harnesses get, and it carries exactly
one kind of thing: **facts about this machine**. `doctor`, `tools`,
`toolsets`, `services`, `registry`, `install`, `skills`, `state`. It gains no
session-scoped surface — no `--session`, no way for an outside harness to read
or write a manifest, an identity, a scenario phase or a transcript.

Those live in `reactor-context`, a crate consumed by the agent loop and the
GUI and by nothing else.

## Why

**A manifest without its history is someone else's notes.** The whole value of
the goal, the steps and the phase pointer is that they summarize a
conversation the reader also has. Handing that state to a harness running a
different session gives it a confident description of work it cannot see —
worse than nothing, because it reads as authoritative.

**Sharing a live session between harnesses is not achievable and not wanted.**
It would require a shared transcript format, which
[ADR-0036](0036-reactor-owns-its-session-store-format.md) deliberately
declines, and a shared notion of turn boundaries, which no two harnesses
agree on.

**What is genuinely worth exporting is the toolbox.** Knowing which RE tools
exist on this machine, whether they are installed, whether their services are
up, how to install the missing ones, and which subset to advertise — that is
REactor's original insight ([ADR-0006](0006-registry-injected-into-system-prompt.md)),
it is entirely machine-scoped, and it is useful to any agent anywhere. It is
also, not coincidentally, the half that already works this way.

**A session-scoped CLI would import problems for a use nobody has.** It needs
a session identity supplied from outside, file locking for concurrent
writers, and an answer to whether the on-disk store or the GUI's memory is
authoritative. All of that to serve a scenario ruled out above.

## Consequences

- **The pi package splits along this line.** `tool-registry`, `selector`,
  `status` and `scenario` consume the portable surface and stay maintained.
  `goal-setting`, `identity`, `reporting`, `rolling-context`, `auto-continue`,
  `context-editor` and `history-tools` are frozen at their current behaviour:
  still working, still tested, no longer developed. A pi user keeps everything
  they have today and gains nothing new in the workflow half.
- **"The toolbox, not the workflow" is the one-line statement of what another
  harness gets**, and it is a scope boundary rather than an apology — the
  workflow half needs a GUI to be worth using
  ([ADR-0033](0033-reactor-is-a-rust-project-on-rig.md)).
- **Activation state stays machine-global by default.** It is the one piece of
  the portable surface that is written rather than read, and an outside
  harness has no session to scope it to. Per-session overrides exist on the
  REactor side only ([ADR-0038](0038-settings-resolve-global-then-session.md)).
- **`reactor-context` is still a separate crate from `reactor-agent`**, even
  though only REactor consumes it. It holds state machines and deterministic
  rendering, testable with no model and no loop; keeping that boundary is what
  stops prompt text from drifting into the agent.

## Considered and rejected

- **`reactor <thing> --session <id>`, with per-session state on disk.**
  Rejected per the reasoning above: it serves session sharing, which is not
  achievable, and charges locking and an ownership question for it. Worth
  revisiting only if a concrete second consumer appears.
- **Export the manifest read-only, for display.** Rejected: the same
  history-free-notes problem, and a read-only surface still needs the session
  identity and the store layout it was meant to avoid.
- **Move nothing down; leave the four toolbox extensions as they are.**
  Rejected: they already shell out to the CLI, so this ADR mostly *records*
  where the line fell rather than moving it. What it adds is the rule that the
  line does not move again.
