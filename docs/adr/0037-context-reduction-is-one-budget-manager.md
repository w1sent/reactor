# Context reduction is one budget manager with three modes, not two mechanisms

The fade and summarization compaction stop being alternatives. One budget
manager owns both, checks the budget at every tool-result boundary, and
applies them by *entry kind* rather than by user preference:

```
keep window     the newest slice of the budget -- always sent verbatim
--------------  ------------------------------------------------------
reclaimable     mechanical entries (tool results)      -> fade
  range         conceptual entries (user turns,        -> summarize
                assistant prose, thinking)
```

The mode chooses where each strategy applies:

| Mode      | Mechanical | Conceptual |
|-----------|------------|------------|
| `fade`    | drop       | drop       |
| `compact` | summarize  | summarize  |
| `auto`    | drop       | summarize  |

`auto` is the default. A manual reduction is available in every mode, at any
time, and means summarize; a manual fade exists for the case where a burst of
dumps should simply go.

This supersedes [ADR-0020](0020-rolling-context-measures-and-cuts-like-pi-does.md)
and [ADR-0025](0025-auto-continue-continues-after-automatic-compaction.md)
outright, and the remaining half of
[ADR-0019](0019-rolling-context-ships-here-general-purpose.md).

## Why

**The two strategies are good at different halves of the same range, not at
different situations.** A `strings` dump, a disassembly listing or a `logcat`
capture is bulk whose conclusion already lives in the assistant's next
message; summarizing it spends a model call to restate what is written down.
A long reasoning exchange is the opposite: dropping it loses the *why*, which
is the failure [ADR-0019](0019-rolling-context-ships-here-general-purpose.md)
cites for summarization-that-fixates and which the fade has in its own form.
Splitting the range by kind applies each where it wins.

**Two mechanisms that cannot coordinate contradict each other.**
[ADR-0020](0020-rolling-context-measures-and-cuts-like-pi-does.md) has the
fade cancelling pi's proactive threshold compaction, because the alternative
is both firing. Owning both removes the cancellation entirely — there is one
budget, one decision point, one policy.

**RE sessions overflow between tool calls, not between turns.** pi checks at
turn boundaries and handles mid-turn only reactively, on overflow with retry.
A single dump can exceed the window inside one turn, which is the case that
path serves worst. Checking at every tool-result boundary makes reduction
proactive and makes continuation intrinsic: there is no turn to park, so there
is nothing for an `auto-continue` to restart.

**The user still chooses.** Automatic selection that cannot be overridden is
worse than either strategy alone, because the one time it picks wrong is the
time the session mattered. `auto` is a default, not a replacement for the
switch.

## Consequences

- **Summarize before dropping, in one pass.** The summarizer must see the
  mechanical entries while writing, or it cannot capture a finding before the
  bytes go. The order is classify, summarize the whole range with everything
  visible, then drop the mechanical entries. Two independent passes produce a
  summary that is missing exactly what it most needed.
- **A faded entry leaves a stub, not a hole**: tool name, arguments, byte
  count, history address. Without it the surviving summary references material
  that is not visibly recoverable, and the model cannot tell that
  `history_read` would reach it. This matters more in `auto` than in `fade`,
  because the summary will reference the dropped material by construction.
- **Classification is deterministic and costs nothing.** Entry kind and size
  are already known. `auto` adds no latency beyond the summarizer call that
  `compact` would also pay.
- **Two invariants survive from
  [ADR-0025](0025-auto-continue-continues-after-automatic-compaction.md)**,
  now as loop rules rather than an extension's edge cases: a consecutive-
  reduction budget, because reduce → continue → reduce is still a real cycle
  and is now unconditional rather than opt-in; and never continue after a
  *failed* reduction, because the context did not shrink and the next request
  re-overflows.
- **Nothing injects a `"continue"` user message.** The assistant turn simply
  continues. Today's synthetic messages accumulate permanently in exactly the
  long sessions the behaviour exists for.
- **The generated blocks are never reclaimable.** Manifest, identity,
  registry and scenario phase are re-rendered from `reactor-context` on every
  request. Today they survive reduction because they sit in the system prompt,
  which works by consequence; here it is a guarantee.
- **The GUI previews and undoes.** What a reduction would discard, what it
  would summarize and how much it reclaims, shown before it runs; and a
  summarized range restorable afterwards, which
  [ADR-0036](0036-reactor-owns-its-session-store-format.md) makes possible by
  keeping originals. Neither is expressible in a footer, and both are part of
  why the frontend is a GUI.
- **The knobs stay the ones the fade already had** — a keep-window percentage
  and a reserve — resolved through
  [ADR-0038](0038-settings-resolve-global-then-session.md) like everything
  else. Whether the keep window should adapt is deferred until there is
  evidence from use.

## Considered and rejected

- **Fade only.** Rejected: it is the current default-off extension, and it
  loses the thread on long analytical sessions, which is the case REactor is
  for.
- **Compaction only.** Rejected: it spends a model call to restate a hex dump,
  mid-turn, while the agent waits.
- **Mode as a global preference with no per-situation logic** (the original
  design). Rejected as the *only* option, kept as two of the three: forcing one
  strategy across a mixed range is exactly what makes each one's weakness
  visible.
- **Tag catalogue entries with whether their output is re-derivable**, and
  fade the re-derivable more aggressively. Rejected: attributing a tool result
  to a catalogue entry means parsing bash command lines, which
  [ADR-0007](0007-deactivation-is-soft.md) already establishes is defeated by
  an absolute path, an alias or an interpreter one-liner. The classification
  stays general — tool result versus prose — which needs no attribution at all.
- **Let the model classify the range.** Rejected: a model call to decide
  whether to make a model call, with a nondeterministic answer, in the middle
  of a turn.
