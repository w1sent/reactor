# Typed input is a fact in the log before it is sent, and it is delivered by its class

A message typed while a turn runs was held in the window's memory ("scheduled"). It was sent
after the whole prompt, was lost if the window died, and could not be seen from anywhere else.
A correction halfway through a long trace meant interrupting and redoing what had been done.
This makes the pending input an entry of the session log, and gives it two ways to arrive.

## Decisions

**Admission is an entry.** `Custom { key: "inbox", item, class, text }` is appended the moment the
person submits. Delivery appends the text as an ordinary user message and then
`Custom { key: "inbox_end", item, how: "delivered", entry }`. Taking it back appends
`how: "cancelled"`. Nothing is rewritten or deleted ([ADR-0036](0036-reactor-owns-its-session-store-format.md),
[ADR-0046](0046-the-analysis-database-is-sqlite-inside-the-case.md): entries are append-only). What waits
is the admissions on the branch with no end after them. They are `Custom` entries because that is
what session state owned by a module already is: the projection skips them, a reduction treats
them like the state entries it already meets, an older build reads the log unchanged, and the
entry format ([`FORMAT_VERSION`](../../crates/reactor-agent/src/store.rs)) does not change.

**The item id is the caller's, and the first admission wins.** Submitting an id the branch has
admitted, delivered or taken back is a no-op.

**Two classes say when.**
- **Steer** reaches the model at the next step boundary of the running turn: after the tool
  calls of the round have settled and before the next request, or, when a reply has no calls,
  before the turn is called finished — in which case the turn gets one more round instead of
  ending and starting again. The turn is not interrupted and nothing in flight is lost. The text
  is delivered exactly as a prompt is: `#` references in it are attached ahead of it.
- **Queue** waits until the turn is over and then runs as a prompt of its own.

At rest, whatever waits runs oldest first, whatever its class (`Agent::prompt_pending`).
That is also what makes a restart safe: a session reopened with input waiting sends it first.

**Delivery is an ordinary user message.** The prompt bytes are not touched at render time
([ADR-0006](0006-registry-injected-into-system-prompt.md)), the budget manager is not consulted
([ADR-0037](0037-context-reduction-is-one-budget-manager.md)), and a steer that follows tool results
is its own message after them, as any user text there already was
([ADR-0057](0057-a-turn-recovers-from-a-rejected-or-cut-off-request.md) keeps results apart from text).

**In the window, Enter steers and Ctrl/Cmd+Enter queues** while the agent works. Each waiting item
is a marker in the transcript, "steering" or "queued", read from the log. Interrupt stops the
turn, takes everything waiting back (tombstones) and gives the text to the composer, as before;
↑ in the empty prompt takes back the latest. The window no longer holds any of this itself.

**A turn sent back by reporting takes what waits with it.** Level-2 reporting forks from the last
user message; input admitted since is admitted again on the new branch (same ids), not left on
the branch it abandons.

## Not decided here

- **ADR-0049 is not amended.** The candidate was that the first delivered steer after a hand-moved
  phase is that phase's summary. A phase cannot be moved by hand while a turn works, and a
  pending summary is consumed by the next prompt at its start, so a steer never meets one.
- **Another process cannot enqueue into a session a window holds.** The session lock
  (`.reactor/run/`) still says which frontend owns a session, and ADR-0046 allows no socket and no
  daemon. A second surface reads what waits; it does not write to a session it does not own.
- **No per-surface default.** Enter steers in the window; a headless surface would choose its own
  when it has one. A setting (ADR-0038) is for when two surfaces need to differ.

## Consequences

- Delivery is at least once: the user message and its `inbox_end` are two entries, and a crash
  between them delivers the text again on resume. The window between them is two appends.
- A steer that was delivered on a branch reporting then abandoned stays on that branch with the
  rest of that turn; it is in the log, not lost.
- `history_index` lists inbox entries like any other `Custom` entry.

## Considered and rejected

- **A new `Kind` for admissions.** It would be the typed choice and would make an older build fail
  to read a log; `Custom` does what is needed.
- **Interrupt appends tombstones and leaves the text in the log only.** The text going back to the
  composer is what a person who pressed Interrupt wants; the tombstone records that it left.
- **An opencode-style inbox with projections and a daemon.** The store has one writer per process
  ([ADR-0046](0046-the-analysis-database-is-sqlite-inside-the-case.md)); an inbox that needs more is
  an inbox for a different program.
