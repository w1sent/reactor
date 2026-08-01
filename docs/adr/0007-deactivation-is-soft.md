# Deactivation is soft: context management, not enforcement

Deactivating a tool or a toolset removes it from the injected registry and hides
its skills and prompt templates. It does not block anything. If the agent invokes
a deactivated tool anyway, the tool runs.

REactor never uses pi's `tool_call` blocking hook, never shims `PATH`, and never
inspects a bash command line to decide whether it is allowed.

## Why

Activation exists to shape what the agent is *told*, which is a context-quality
problem: a registry advertising forty tools when six are relevant is noise, and
noise degrades tool selection. It was never a safety problem. The agent is not
adversarial; it is eager and occasionally under-informed.

Enforcement would be theatre. pi ships no permission system by design, the agent
has bash unconditionally, and anything that detects `frida` in a command line is
defeated by an absolute path, an alias, or `python -c "import frida"`. Shipping
a guard that stops the honest case and misses every other one produces false
confidence — a mechanism users believe in that does not hold.

There is also a better error already available. A tool that cannot usefully run
says so itself, in its own words, with its own diagnostics: `bn` reports that no
Binary Ninja session is open; `adb` reports no devices; `frida` reports it cannot
attach. Those messages are more accurate and more actionable than anything
REactor could synthesise, because the tool knows why it failed and REactor only
knows that it was not on a list.

## Consequences

- A tool absent from the registry can still be used, and sometimes should be —
  the model's pretraining knowledge of a tool remains valid, and if it reaches
  for something correctly, nothing stands in the way.
- Scenarios ([ADR-0009](0009-scenarios-advance-by-tool-result.md)) steer by
  briefing, not by restriction. "Do not start dynamic analysis yet" in a step
  briefing is the mechanism; there is no second, harder mechanism behind it.
- Implementation is trivial and has no false positives: filter the registry,
  filter `resources_discover`. No shell parsing exists anywhere in REactor.
- Real confinement, if ever wanted, is a containerisation concern. pi documents
  three patterns for it (Gondolin, plain Docker, OpenShell) and that is the right
  layer for it — an actual boundary, not a heuristic.

## Considered and rejected

- **Advisory blocking** — detect the binary in the bash command line, block via
  `ToolCallEventResult { block: true, reason }`, and explain that it is outside
  the active toolset. Tempting, because a blocked call with a clear reason is a
  strong redirect and the model is not trying to evade it. Rejected: it is a
  heuristic dressed as a rule, it will produce false positives on command lines
  that merely mention a tool name, and the honest version of its behaviour —
  "usually stops you, sometimes does not" — is not a useful contract.
- **`PATH` confinement** — materialise a directory of symlinks to active tools
  and point pi's bash at it. Genuinely enforces the common case rather than
  guessing, but diverges from the user's own interactive shell, breaks tools that
  shell out to their own helpers, and is still bypassed by absolute paths and
  library imports. Rejected as high-friction for partial enforcement nobody asked
  for.
- **No deactivation at all** — rejected; curating the registry is the point of
  toolsets, and the registry is the product.
