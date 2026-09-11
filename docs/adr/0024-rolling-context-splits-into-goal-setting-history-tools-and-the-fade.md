# rolling-context splits into goal-setting, history-tools and the fade

`extensions/rolling-context/` was one 815-line extension carrying three
features with different audiences and different reasons to exist: the
**manifest** (goal, guidelines, agent-maintained steps, `update_steps`,
`/goal`, `/guidelines`, `/frame`), the **history recovery tools**
(`history_index`/`_search`/`_read`), and the **fade** (the `context` cut,
`/rolling`, compaction preemption). It is now three extensions:

| Extension | Keeps | Its own switch |
|---|---|---|
| `goal-setting/` | manifest, guidelines, steps, `update_steps` | `/manifest [on\|off]` |
| `history-tools/` | `history_index`/`_search`/`_read`, serialization | `/history-tools [on\|off]` |
| `rolling-context/` | the fade: `context` cut, compaction preemption, `/rolling` | `/rolling [on\|off]` |

Names, tool ids and command names are unchanged, so nothing outside the
package's own docs referenced them by file.

## Why

The three features are useful under different conditions. The manifest is
durable session memory that works under *pi's own compaction* too — there is
no reason its fate was tied to `/rolling off`, which today disabled
`update_steps` and hid the history tools along with the fade. The history
tools are read-only recovery over the session file, worth having in any long
session. Only the fade needs the fade. The coupling was an artifact of one
file growing from "an alternative to compaction" into three products.

## The manifest moves into the system prompt, not into the message array

Pre-split, the fade rendered the manifest as a prepended `custom` message
from its `context` handler, decorated with a per-turn
`[context: visible from line N of M]` pointer. Splitting ownership raised a
question the old code never faced: **who prepends the manifest, and in what
handler order?** pi composes `context` handlers by chaining — each handler
receives the previous handler's result (`runner.js` `emitContext`) — but the
*extension order* the chain follows is the unsorted `readdirSync` order of
the package's `extensions/` directory (`collectAutoExtensionEntries`), which
is filesystem enumeration order: not alphabetical by guarantee, not
controllable from the package, and therefore not something any design may
rest on. A fade that runs *after* a manifest-prepending handler would see the
manifest as an old message and cut it first.

The resolution removes the ordering dependency entirely: the manifest goes
into the **system prompt** via `goal-setting/`'s `before_agent_start` block.
That composition is order-independent by construction — every handler
appends a section to `event.systemPrompt` — and it is what pi's own
extensions-and-prompt plumbing already does for three other extensions here
(tool-registry's registry, reporting's block, the old rolling-context
guidance). Two further consequences, both verified against pi 0.85.1:

- **The fade's accounting stays exact with zero coupling.** The chained
  `before_agent_start` result is written into `agent.state.systemPrompt`
  (agent-session.js:932), and the `context`-handler ctx's
  `ctx.getSystemPrompt()` returns exactly that (agent-session.js:2088). The
  fade subtracts `estimateStringTokens(ctx.getSystemPrompt())` from the
  budget, so goal-setting's block is accounted for without either extension
  knowing the other exists (ADR-0014: no shared modules — and now none
  needed, where the old design needed the fade to estimate a manifest it
  rendered itself).
- **The per-turn line pointer dies with the message-manifest.** It was
  cosmetic — the fade's own comment called it cosmetic and never used it to
  decide anything — and it cannot survive the split, because the manifest
  renderer cannot know what the fade cut. The fade's user-facing notification
  (`dropped N message(s)`) and the in-content `ARCHIVE_NOTE` for the
  hard-budget path carry what remains worth saying; the model learns that
  fading exists from the standing guidance, which is always true while the
  fade is on.

The block changes only when goal/guidelines/steps change, so the system
prompt — the prefix every provider-side cache keys on — is now byte-identical
across turns where nothing was written. The old message-manifest was rebuilt
every turn with a new line pointer, busting the cache on every fade; that
regression class is gone rather than mitigated.

## Switches

- **`update_steps` is active while a goal is set *and* the switch is on.**
  With the switch defaulting to on, that makes "no goal set" the resting
  gate: the tool is inactive in fresh sessions and says so when called.
  `/manifest off` is the manual override. The gate lives in `execute()`
  (the repo idiom), not `pi.setActiveTools()` — that list is shared across
  all extensions, and ADR-0017 records why touching it for one tool's
  visibility is a bad trade.
- **`history-tools` is on by default and always registered.** Its usefulness
  is independent of the fade; `/history-tools off` is for a model that calls
  it too often, and rides its own per-session `custom` entry so the choice
  survives a reload. The frugality guidance ("a recovery path, not a
  browsing habit") stays in the *fade's* system-prompt block, where the
  economics that motivate it live; the tools themselves carry no opinion.
- **`/rolling` is unchanged**, and keeps the `pi-rolling-context`
  `customType` for its `{ enabled }` entry. Entries written before the split
  also carried goal/steps fields; the fade reads only `enabled` and ignores
  the rest.

## Clean break

REactor is in alpha; no backwards compatibility is carried. The three
extensions read three config files (`pi-goal-setting.json`,
`pi-history-tools.json`, `pi-rolling-context.json`), each with only its own
knobs — the one-file-per-extension rule the old header already stated. Old
files with unknown keys are read with defaults for the missing ones and
ignore the rest; old `pi-rolling-context` goal/steps state is not adopted by
`goal-setting/`, which uses its own `pi-goal-setting` entry type. The
`pi-rolling-context` name stays with the fade because the extension keeps
its name — that is ownership, not migration.

## Costs

- `estimateStringTokens` and the state-restore walk are duplicated in two or
  three files. This is the price ADR-0014 names for every split; the
  duplicates are small and their drift surface is a single formula.
- The guidance prose now lives in two files (goal-setting's steps discipline,
  the fade's fading explanation), and the fade's wording is written to be
  true whether or not goal-setting and history-tools are loaded — "when the
  session provides a manifest…", "when available" — because nothing in pi
  enforces co-presence of extensions.
- `history_index` counts zero-content entries (its own toggle writes among
  them) — pre-split semantics, preserved, now visible in a suite that has no
  `/rolling on` in its fixtures to explain the +1s.

## Considered and rejected

- **Manifest stays a prepended message, fade made order-robust** (protect
  leading `custom` messages, count their tokens, fixed `manifestReserve`
  otherwise). Kept the documented "front of every prompt" shape, but it
  needs a generic never-cut-custom rule in the fade to be order-independent,
  plus a reserve knob or a measure-if-visible dance for the budget — three
  moving parts where M2 needs none, for a shape nobody asked to keep.
- **The fade keeps rendering the manifest** (status quo), reading
  goal/steps state from goal-setting somehow. Rejected: the only channels
  are a shared module (ADR-0014 forbids) or a filesystem rendezvous both
  extensions invent — the exact drift the no-shared-modules rule exists to
  prevent.
- **Rely on directory names for load order** (e.g. naming so the fade sorts
  last). Rejected on the verified fact that the order is unsorted
  `readdirSync`, not alphabetical — nothing controllable, nothing documented,
  and a rename away from breakage.
- **Gate `update_steps` with `pi.setActiveTools()`** so it vanishes from the
  tool list entirely. Rejected for the ADR-0017 reason: the active-tools
  list is shared extension-wide state, and hiding one tool for prompt
  tidiness buys a coordination hazard no other extension here accepts.
- **Migrate old state/config.** Rejected per the alpha stance: every
  migration path is code that must itself be tested, for users who can
  re-run `/rolling on` and `/goal`.