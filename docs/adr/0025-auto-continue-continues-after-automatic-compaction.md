# auto-continue continues after automatic compaction, bounded, and never into pi's own retry

`extensions/auto-continue/` adds a toggleable behaviour on top of pi's
compaction: when an *automatic* compaction ends the agent's turn, the
extension sends the model a short user message (default `continue`) at
`agent_settled`, so a session that got interrupted by compaction picks its
work back up without a human typing it. Off by default;
`/auto-continue [on|off]` opts a session in, `pi-auto-continue.json` carries
the global default, the message text and `maxConsecutive`.

## Why

pi's compaction leaves the turn ended in two of its three automatic cases.
Verified against pi 0.85.1 (`agent-session.js`, `_handlePostAgentRun` /
`_checkCompaction` / `_runAutoCompaction`):

1. **Overflow with retry** (`willRetry: true`): pi removes the failed
   message, compacts, and the post-run loop calls `agent.continue()` itself.
   The turn resumes without a human. Queueing a "continue" here would inject
   a message into the retrying run — excluded, on purpose.
2. **Overflow with a preserved response** (`willRetry: false`): the response
   completed, compaction ran, the turn is over. The agent is idle.
3. **Threshold** (`willRetry: false`): same shape, the common case.

Cases 2 and 3 are where a session sits parked until someone types something —
exactly the gap this extension fills: `session_compact` with those shapes
sets a pending flag, and `agent_settled` (pi's own settle point, the same one
reporting/ level 2 dispatches from) sends the continuation. The send goes
through `pi.sendUserMessage(message, { deliverAs: "followUp" })`:
`sendUserMessage` maps `deliverAs` onto `prompt()`'s `streamingBehavior`, so
if another extension's own `agent_settled` handler started a run first
(reporting's revert resend and this continue can land on the same settle),
the message queues and runs after it instead of throwing "already
processing".

## What it deliberately does not continue

- **A failed compaction** (`session_compact_failed`, or no
  `session_compact` at all): compaction never shrank the context, so a
  "continue" would re-overflow. The pre-prompt compaction check in
  `prompt()` will compact before the next prompt anyway.
- **A manual `/compact`**: deliberate housekeeping, not an interruption.
- **A pre-prompt compaction**: compaction can run *before* a user's prompt
  (`_checkCompaction` from the pre-prompt path). `before_agent_start` clears
  any pending flag, so a housekeeping compaction can never be mistaken for
  an interruption of pending work.

## The runaway guard

Compaction → "continue" → compaction can in principle cycle forever when
every turn refills the window — each cycle a full turn of LLM calls, with no
human in the loop once the extension is on. The guard: `before_agent_start`
sees each turn's prompt; a turn starting with exactly the configured
continuation message keeps a consecutive count, any other prompt resets it
and lifts a pause. Past `maxConsecutive` (default 10) the extension sends
nothing and notifies once; the next real prompt resumes it. A hand-typed
"continue" also counts — the count is about turns nudged with that exact
word, which is the pattern that needs a bound.

## Costs

- It sends a bare "continue" with no explanation of why. The compaction
  summary is in context by then (that is what compaction produces), so the
  model has what it needs; the message text is configurable for anyone who
  wants richer wording.
- The count-based pause is crude: a legitimately long autonomous task that
  compacts every turn will trip it. The notification says how to resume, and
  `maxConsecutive` is a config knob.
- Two extensions prompting from one `agent_settled` still serialize through
  the queue rather than truly interleaving — `followUp` makes that a
  delay, not a failure.

## Considered and rejected

- **Sending from the `session_compact` handler directly.** Compaction is
  still in progress inside the agent run; `prompt()` throws "Cannot submit a
  prompt while compaction is in progress". `agent_settled` is the first
  point pi guarantees safety.
- **Queueing from `agent_end`** (the documented pattern behind
  `hasQueuedMessages()`): the queueing API for extensions,
  `pi.sendMessage`, sends a *custom* message, not a user message — the
  agent would see an extension note, not "continue" as user input.
- **Continuing after overflow recovery** (`willRetry: true`): pi already
  retries; a second nudge would double the turn.
- **Continuing after failed compactions**: re-overflows by construction.
- **No bound at all**: the whole point is hands-off operation; an unbounded
  send loop is the one failure mode worse than parking the session.