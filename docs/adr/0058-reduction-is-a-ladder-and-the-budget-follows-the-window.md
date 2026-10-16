# Reduction is a ladder, and the budget follows the window

[ADR-0037](0037-context-reduction-is-one-budget-manager.md) fixed the budget's numbers for a
large window: a 16k reserve, a trigger at 90%, a keep window of half of what is left. At 100k
that spends a summarizer call for every 33k tokens of new work, and in a window of 16k to
32k the reserve is most of the window. Eight other harnesses were read for how they keep a
long session inside a window ([ADR-0057](0057-a-turn-recovers-from-a-rejected-or-cut-off-request.md)
has what they do about failure). They agree on more than they differ: a keep window of 15 to
20% of the window, tool output reduced before prose is summarized, a summary that updates the
last one rather than restating it, and a reserve that shrinks with the window.

This records the shape the budget takes. **Parts A to D are built.**

## Part A — built: the numbers follow the window

- **The reserve is an eighth of the window, between 1,024 and 16,384** (`default_reserve`),
  and follows the window if the window is changed, unless a reserve was chosen. A 100k window
  keeps 12.5k and a 32k one 4k, where both kept 16k.
- **The reply limit is a quarter of the window up to 32k** (`default_output`), cut to the room
  left ([ADR-0057](0057-a-turn-recovers-from-a-rejected-or-cut-off-request.md)).
- **The summarizer is sized to its window** (the same ADR).

## Part B — built: the ladder

**Reduction is a ladder, cheapest first, and each stage is tried only if the one before did
not reach its target.**

```
over the trigger
 └─ 1. mask       no model: old tool results become stubs, prose is untouched
     └─ 2. summarize   a model: today's `auto`, over a range already smaller
         └─ 3. fade       no model: today's `fade`, only when stage 2 fails
             └─ 4. shrink the latest result: when nothing older is left to reduce
```

**1 — mask** is a second shape of reduction. It replaces the *content* of a tool result in
place with a stub — `#id`, the tool, a digest of the arguments, the size, and a short head and
tail, because errors and final lines live there — so the pairing with its call is never at
risk. It is one `reduction` entry (`masks`, no `covers`); `restore` and `history_read` work as
before; nothing is deleted. It takes the oldest results first, until the context is at 60% of
the usable budget (`maskTarget`). The target sits below the trigger so the stage does not fire
again after two more results. What it leaves alone:
- the newest 15% of the window (`protect`);
- **a result the model has not answered yet.** It has not read it. A single result bigger than
  the protected slice would otherwise be masked before the model ever saw it, which is what an
  early build of this did;
- a result under 1 KiB, and one that is a stub already;
- a result the model read back with `history_read` is taken last, not never: it asked for it.

A stub keeps the call and its arguments, the size, where to read it, and the start and the end
of the output — a fiftieth of the window each, between 120 and 400 characters.
It is used only in `auto`: `compact` asks for summaries and `fade` for the range to go.
It is used **only if it alone reaches the target.** Otherwise stage 2 runs on the unmasked range,
so the summarizer still sees every entry before any bytes leave it, as ADR-0037 requires. The
relaxation of ADR-0037's one-pass rule is therefore only for a reduction that writes no
summary, which needs no finding captured.

**2 — summarize** is `auto`, with these changes:
- the trigger is 85% and the keep window 28% of the usable budget (from 90% and 50%);
- the summary's size scales with what it covers, a sixth of it (17%), between 1,500 tokens and
  5,000 or an eighth of the window, whichever is less;
- the earlier summary is given to the summarizer in `<previous-summary>` tags, with "keep what is
  still true, let newer entries replace what they contradict";
- the summary has fixed headings (goal; established, with names, addresses, hashes and paths;
  ruled out; open; next). Three of the five will do; a reply with fewer is asked for once more,
  and the second is taken as it is, since a small model that cannot follow a template can
  still write useful notes. A summary no shorter than what it covers (over 400 tokens) is
  refused;
- thinking is left out of what the summarizer reads (ADR-0037 listed it among the prose to
  summarize; it is the bulkiest and the least likely to hold a conclusion that the reply
  after it does not);
- **the cut may fall inside the current turn.** `plan()` stopped the keep window at the
  latest message of the person's, so nothing after it could ever be reduced, and one prompt
  that drives dozens of tool calls could fill the window with nothing reducible. Now your last
  message is always kept, and the rest of the turn is reducible. ([ADR-0041](0041-the-agent-loop-owns-the-message-list.md)
  said "no later than the last user message": what it wanted was that the instruction is never
  cut away, and that stays.)

**3 — fade** is the fallback when stage 2 fails for any reason: an error, an empty reply, a
policy block, a refused summary, or the shrinking in
[ADR-0057](0057-a-turn-recovers-from-a-rejected-or-cut-off-request.md) running out. It needs no
model and still shrinks the context, so the rule that the loop never continues on a context that
did not shrink holds, and a notice says what happened. The person's messages stay as they said
them, newest first, within a twentieth of the window (an older one that does not fit is left as
its first words, and the newest that does not fit whole is cut rather than lost); the calls it
dropped are listed with 160 characters of their arguments. A repeated fade keeps only that much,
not every message ever dropped. When the summarizer is what failed, the notice is the way a
person finds out the summaries have stopped.

**4 — shrink the latest** replaces "the context is full" when only the latest work is left and it
is over the usable budget. It is the same masking, with nothing spared — the result the model has
not read yet is cut too — and with each stub as big as the room allows, not a fiftieth of the
window. The model gets the start and the end of the dump, and `history_read` for the middle. The
turn fails only if nothing in the context is big enough to cut (a message of the person's bigger
than the window), and says so.

**Also built:**
- **The cut falls inside the turn.** The reduction covers everything in the range *except* the
  person's last message, and records it as an `after` anchor so the stand-in goes right behind it.
  A reduction that would cover nothing but an earlier stand-in is "nothing to reduce"; without
  that, the stand-in behind the task was reduced with itself until the attempts ran out.
- **Thinking is not shown to the summarizer.**
- The settings window, `/context` and its palette have `maskTarget` and `protect`; a mask is
  listed in the context pane as "masked" and undone like any other reduction.

## Part C — built: the cache, the summary's start, the cap and the loop

**Prompt caching is on for Anthropic.** It was off, so a prompt that was the last one plus a few
lines was paid for in full each time. rig's automatic caching puts a break point on the last block
and moves it forward as the conversation grows; the lifetime is five minutes. A write costs more
than a plain read of the same tokens and a hit costs far less, so a session that goes quiet for
longer than that pays the write again: the trade is good for an agent working in bursts, and worse
for one asked a question an hour. Caching is silently skipped below a model's minimum prompt (1,024
to 4,096 tokens), which a session passes quickly. The other providers cache what they are sent
without being asked (a repeated start), and a local server keeps its last prompt, so a stable
prompt ([ADR-0041](0041-the-agent-loop-owns-the-message-list.md)) is what earns it.

**Usage is counted in one way.** Anthropic reports its input apart from the part it read from or
wrote to its cache; with caching on, `input_tokens` is a few hundred while the prompt is tens of
thousands, and the learned token ratio would have fallen to its floor. `Usage.input_tokens` is now
everything that was sent for every provider, and `cached_input_tokens` the part of it that came
from a cache.

**The summary request begins as the main request did.** When the summarizer is the main model and
the whole of that request fits, it is sent with the main request's system prompt, tools and
messages up to the end of the range, exactly as they were, and the instruction as the last message,
with the model told not to call a tool. A provider that caches the start of a prompt, or a local
server that keeps the last one, has most of it done. It cannot be shrunk, so a rejection as too
long, a reply that calls a tool anyway (some providers cannot be told, Ollama's among them), a reply
with no notes, or a request too big for the window, goes on to the flattened form, which has the
shrinking steps of [ADR-0057](0057-a-turn-recovers-from-a-rejected-or-cut-off-request.md). The
reminder for the headings and the extra room for a model that thinks work in both. The replayed
messages include the model's earlier thinking, since the start must be the same bytes; the
flattened form still leaves it out. How much of the cache a replay finds depends on the provider
(it was written against what each documents, and has not been run against a live key).

**What the model sees of one result is a share of the window.** Unset, `toolOutput.maxInlineBytes`
is a twentieth of the window (as many bytes as a fifth of its token count), between 2 KiB and 32 KiB: 3.2 KB in
16k, 20 KB in 100k, and what it always was in a large window. A limit the person set is theirs
whatever the window, and a call may still ask for another ([ADR-0053](0053-a-call-chooses-how-much-of-its-output-it-sees.md)).

**Five identical calls in a row end the turn.** "Identical" is the call, its arguments *and* its
result, so asking again for something that changes (waiting on a process) is not a loop. The calls
left in the reply are answered `not run`, and the message says what was repeated. Zero turns it off.

## Part D — built: counting, focus, the second look, the threshold and the warning

**The context is counted from what the provider last said.** The newest assistant reply of the
current model that is in view, with no reduction, mask or restore after it, carries the provider's
count of that request. What the next request carries is that count plus the reply's own tokens,
plus an estimate of what has come since (at the learned ratio) and of the fixed part having
changed (each reply records its estimate of it, so a toolset switched on in between is allowed
for). If anything changed the start of the request after the reply, or the model changed, the
count is stale and the estimate stands. Whether it is time to reduce is decided on these real
tokens against the real trigger, with no ratio between: the ratio is clamped, and a count far from
the estimate must not be hidden by it. The same count sizes the reply's room, and is what the
context bar shows. It is there from the first request of a reopened session, where the learned
ratio starts again at one.

**A reduction by hand can say what to keep.** `/reduce [auto|fade|compact] [what to keep]`; the
text is added to the summarizer's instruction ("the person asked you to keep this in particular,
in full"). A fade has no summary, so it is not asked of anyone.

**The second look.** After the notes are written the model is shown them and asked what they leave
out, and writes them again with it added or the same. It is one more call over the whole range, so
it is a setting (`context.verify`, off) — and always on for a reduction by hand, which is the
reduction a person has reason to want right. It is a check, not a rewrite: an error, an empty
answer, one that drops the headings, or one less than half as long leaves the first notes as they
were. It runs the main model's way too, with the notes as the model's own words in the
conversation.

**The deferral threshold follows the window.** Unset, a group of tools is left for `tool_search`
once it costs a hundredth of the window, between 250 and 1,000 tokens (1,000 is what it always was
for a large window). A threshold the person set is theirs.

**A cramped window is said so, once.** When the fixed part of a request (system prompt, tools, the
blocks generated each time) is over half of what the window leaves, the context preview lists it as
a warning, the context pane shows it, and a notice says so the first time. Nothing is refused: a
small window should work, with less room.

## Why

The numbers are the evidence of other harnesses, not a measurement of REactor. pi keeps 20k
verbatim, opencode 15k, hermes at most a fifth of the window, deepseek-harness 16%; reactor
kept half. opencode's reserve is a tenth of the window below 32k, and hermes found that a tail
that is most of a small window summarizes six rows and reclaims nothing. A mask stage is what
gemini-cli, hermes and deepseek-harness each have in some form, and deepseek-harness skips the
summary when trimming is enough: for a session of dumps, a summarizer call that restates what
the next assistant message already says is the waste ADR-0037 set out to avoid.

## Consequences

- ADR-0041's "a reduction is one stand-in for a contiguous range" gains a second shape, and
  its keep-window rule changes as above.
- `Reduction` gains the masks; old logs read as before, because the new field defaults.
- Frontends that list reductions show which shape and which stage.
- Defaults change for everyone who did not set them: a sooner trigger, a smaller keep window.
  Settings that were set stay.

## Considered and rejected

- **Mask first, always, then summarize.** The summarizer would never see the output that was
  masked, which is the failure ADR-0037's one-pass rule is there to prevent.
- **Keep fade range-only.** A fade that drops prose to save tool output loses the reasoning;
  masking keeps it.
- **Refuse small windows** (hermes below 64k). The point is that they work.
- **Compact in the background while a tool runs** (codex, goose, gemini-cli). It fights with the
  cache and with masking, and is worth building only once the summarizer's latency is measured.
- **Server-side compaction** (codex). OpenAI's, and REactor is multi-provider.
- **Redact secrets from the summarizer's input.** In malware analysis the secret is often the
  finding. At most an option when the summarizer is another provider than the main model.
