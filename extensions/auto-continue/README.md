# auto-continue

Keep the agent going. When an *automatic* compaction ends the agent's turn,
this sends the model a short user message (default `continue`) so it picks its
work back up — no one has to type it. Off by default;
`/auto-continue [on|off]` opts the session in.

It deliberately does not continue: overflow recovery (pi retries the turn by
itself), a failed compaction (the context never shrank), and a manual
`/compact` (deliberate housekeeping).

A runaway guard bounds the loop: consecutive continuations of the same
message are counted, the extension pauses after `maxConsecutive` (default 10)
with one notice, and your next real prompt lifts the pause.

Config in `~/.pi/agent/pi-auto-continue.json`: `enabled`, `message`,
`maxConsecutive`. The compaction semantics it distinguishes and the pi facts
it rides on: [ADR-0025](../../docs/adr/0025-auto-continue-continues-after-automatic-compaction.md).
