# The cache is measured before it is tuned, and the start of a request can be kept for an hour

[ADR-0058](0058-reduction-is-a-ladder-and-the-budget-follows-the-window.md) turned Anthropic's
automatic caching on and said, of its own work, that it "has not been run against a live key".
`TODO.md` asked the question underneath: which surfaces rebuild the system prompt, what each
transition costs against the cached prefix, whether any of it churns per turn. A separate
placement policy was proposed for the request (breakpoints on the tools, the system prompt and
the tail, at most four). It is not built: rig already places them, counts any a caller placed,
and stops at the provider's limit of four, so a second pass in REactor would argue with the
first.

This records the two things that are missing: a way to see what the cache did, and one setting.

## Decisions

**A reply records what it wrote to the cache, not only what it read.** `Usage` gains
`cache_write_tokens` (omitted when zero, so old logs read as before). With `cached_input_tokens`
it gives, for any session, the account of its cache from the log alone: no new table and no
second record.

**`/cache` reports the current branch.** Replies counted, input sent, the part read from the
cache and the part written to it, and the requests after the first that read nothing — with how
many of those came after a pause longer than five minutes. The last number separates the two
reasons a request goes cold: the clock (the cache expired) and the bytes (the start of the
request changed). The first is cured by a longer lifetime; the second by finding what changed.
Nothing is sent to anyone; it is a reading of the log.

**`cache.prefixTtl` in `settings.json` (`"5m"` or `"1h"`) keeps the system prompt and tool
definitions for an hour.** Anthropic only; unset is what it was. rig marks the final tool
definition and the system prompt at that lifetime and leaves the moving tail at five minutes,
which suits an agent whose start is the same for hours and whose tail is worthless soon after.
An hour's write costs about twice what a five-minute one does, so it pays only if the start is
byte-identical across the pauses of a case. [ADR-0006](0006-registry-injected-into-system-prompt.md)
guarantees that for an unchanged machine; the scenario block, the goal and the activation state
([ADR-0030](0030-gated-tools-advertise-by-state.md)) are in the system prompt and do change, and
each change is a write at the dearer rate. The setting is global only, since a provider is built
once per session ([ADR-0038](0038-settings-resolve-global-then-session.md) has no session half
for it), and it is off.

## Why

The numbers decide whether the hour is worth it, and they are in the log after a day of work.
Turning it on by default on a guess would charge twice for every phase change.

## Considered and rejected

- **A REactor pass that places breakpoints** (opencode's `AUTO` policy). rig does it and
  enforces the limit.
- **Turn off cache writes for summary requests** (pi). ADR-0058 replays the main request's
  start so that the summary request finds it cached; the opposite is what it built.
- **One hour by default.** See Why.
- **A session-level cache lifetime.** The provider is built once; it would need rebuilding
  the model mid-session for a cost that is a property of the machine, not of the session.

## Not done

- A provider other than Anthropic is not asked to cache any differently. OpenAI and the local
  servers cache by repeating the start; `cached_input_tokens` already shows whether they did.
- The settings window has no field for it yet.
