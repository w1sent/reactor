# The tool registry is injected into the system prompt, from a cached probe

The tool-registry extension probes the catalogue's entries, caches the result,
and returns a rendered registry block from pi's `before_agent_start` event, which
allows an extension to replace the system prompt for that turn.

The block carries one line per *present and active* tool: name, the catalogue's
one-line description, how to invoke it, and live service state where the tool
has a service probe. It ends with a pointer, not with documentation.

```
## Available RE tools (this machine)
bn      Binary Ninja RE framework       [BN running, 1 binary open]
frida   dynamic instrumentation         17.2
jadx    Android/Java decompiler
adb     Android device bridge           [2 devices]
joern   code property graph / dataflow

Use `<tool> --help` for usage. `reactor tools` for detail.
```

## Why the system prompt, and not a message

The registry is a standing fact about the environment, and it must be true *now*,
not true at session start. A message injected once at `session_start` goes stale
the moment a service starts or a device is unplugged, and correcting it means
appending a second message that contradicts the first — both remain in context,
and the model sees two registries.

The system prompt is the one place a fact can be *replaced* rather than appended.

## Why prompt-cache stability is a design constraint, not an optimisation

Replacing the system prompt invalidates the provider's cached prefix for that
turn. Doing it carelessly — say, embedding a timestamp, or re-ordering entries
by probe completion — would invalidate the cache on *every* turn of *every*
session, which is a real and continuous cost.

So rendering is deterministic: fixed ordering, fixed formatting, no timestamps,
no probe-duration noise. If nothing about the machine changed, this turn's string
is byte-identical to last turn's and the cache is untouched. It changes when
reality changed — a service came up, a tool was installed, a toolset was toggled
— which is exactly when paying for invalidation is correct.

Probe scheduling follows from the same constraint:

- **Detection probes** (is the binary on `PATH`) run at session start and on
  explicit refresh. Installing a tool mid-session is rare and the user can say so.
- **Service probes** (`bn health`, `adb devices`) run on a slow timer, because
  they genuinely change during a session. They are the only source of routine
  churn, and they are reported coarsely — up, down, count — so that ordinary
  fluctuation does not produce new text.
- **`/reactor refresh`** and any toolset toggle force a full re-probe.

## Skill and prompt visibility ride along

The same extension answers pi's `resources_discover` event, returning the skill
and prompt paths that correspond to currently-active tools. Deactivating a
toolset therefore removes both its registry lines and its skills' descriptions
from the system prompt, and `ctx.reload()` makes a toggle take effect
immediately.

## Where the block is rendered

In the CLI, not in the extension: `reactor registry --format json` returns the
finished string alongside the structured data, and the extension concatenates it
onto `event.systemPrompt` without inspecting it.

Decided during implementation. The alternative — the extension formatting the
entries itself — puts the byte-stability requirement in TypeScript while the
probe results, the ordering and the coarsening rules all live in Python, so the
property would have to be defended in two languages and could only be tested in
the one that does not own the data. Rendering here makes determinism a single
Python function with a single test, and leaves the extension a pure transport.
The cost is that a future non-pi front end inherits this block's formatting
rather than choosing its own; that is acceptable while pi is the only target
([ADR-0001](0001-pi-is-the-only-target-harness.md)).

## Consequences

- The registry's size is bounded by the number of *installed and active* tools,
  not by the size of the catalogue. A machine with six RE tools gets six lines
  regardless of how many the catalogue knows about.
- The `desc` field in the catalogue is the highest-leverage text in the project.
  It is written for a model deciding whether to reach for the tool, not for a
  human browsing a README.
- The extension must degrade cleanly when `ctx.hasUI` is false — the registry is
  useful in `print` and `json` modes too, and only the status/selector surfaces
  require a TUI.
- If a probe hangs, the turn stalls. Probes need timeouts and a cached-value
  fallback, and a probe that times out is reported as unknown rather than
  absent.

## Considered and rejected

- **A `session_start` message** — rejected; goes stale, and refreshing it leaves
  a contradicted copy in context permanently.
- **The `context` event, rebuilding before every LLM call** — rejected. Freshest
  possible, but it fires once per agent-loop iteration rather than once per user
  turn (so ~7× per turn with six tool calls), and rewriting the message array is
  the most cache-hostile option available.
- **Pull-only: no injection, just "run `reactor tools`" in the prompt** —
  rejected. Zero context cost and always fresh, but it inverts the actual
  problem: a model that does not know Binary Ninja is available does not think
  to go looking for it. It writes a Python parser instead. Being told is the
  entire point.
- **Registering each tool as a pi tool via `registerTool`** — rejected; it
  reintroduces exactly the per-tool schema bloat that CLI-over-MCP avoids, and
  the tools already have interfaces.
