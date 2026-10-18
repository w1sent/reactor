# A repeated call is first a notice, and only much later a stop

[ADR-0058](0058-reduction-is-a-ladder-and-the-budget-follows-the-window.md) ended a turn after
five identical calls with identical results. That is the one place the loop stops a model for
what it did rather than for what the provider said, and five came without a line to the model
that it was near. A model that is told it is going round in circles can say what it is waiting
for, or change course; one that is only stopped cannot. [ADR-0007](0007-deactivation-is-soft.md)
and [ADR-0033](0033-reactor-is-a-rust-project-on-rig.md) already settle the posture: the model
is eager and under-informed, not adversarial, and nothing here blocks a call.

## Decisions

**A result carries one line when a threshold is crossed.** The line follows the tool's own
words, after a blank line, and is added after the result was cut, so it is never lost to the
cut. It is part of the stored result, so a reduction, `history_read` and a resumed session see
what the model saw. Nothing is added to the system prompt, and nothing is sent unless it fires.

- **The same call with the same result, three times in a row** (`repeatNotice`, 3): "This is the
  3rd identical call to `bash` with the same result. If you are waiting for something to change,
  say what; otherwise try something different."
- **The same tool failing on three calls in a row** (`failureNotice`, 3), whatever the
  arguments: "`bash` has failed 3 times in a row. Say what you think is going wrong before the
  next attempt." A success of any tool, or a failure of another, starts the count again.

Each fires when the count *reaches* the threshold, not on every call after, so a streak makes one
line. If both reach theirs on one result, only the repeat is said: it is the more specific.
The wording asks and never forbids. A frontend also gets the line as a notice.

**A call is the same when its arguments are, not when they are written the same.** The
arguments are hashed with every object's keys in order. The JSON keeps the model's order
(`preserve_order` is on in the workspace), so `{command, session}` and `{session, command}`
were two calls before this.

**The stop stays, as a backstop, at eight.** `maxRepeats` was five and is eight, so that the
notice at three has five calls to be heard in. Zero still turns the stop off, and each notice
has its own zero. These are `AgentConfig` fields, as the stop was; no settings key yet.

**The counts belong to a turn.** They start over with each prompt, so a resumed session cannot
say something about calls its model no longer has in view.

## Considered and rejected

- **No stop at all.** A model can go round in circles for a hundred calls on a paid provider;
  eight identical calls with identical results is not a thing a person wanted done.
- **An LLM auditor every N turns** (gemini-cli). A second model's call over the history for a
  problem that three equal hashes already show. Default-off in the source; not built here.
- **A user-side message instead of a line on the result.** Two user messages in a row between
  tool results and the next request fight the provider's pairing rules
  ([ADR-0057](0057-a-turn-recovers-from-a-rejected-or-cut-off-request.md)), and a separate
  entry is one more thing for a reduction to cover.
- **A chunk-repetition check on the model's text** (gemini-cli's repeated 50-character
  chunks). A loop in prose has not been seen here; the calls are where the cost is.

## Consequences

- `Seen` replaces the bare count in `run_tools`; a result may now end in a `[reactor]` line.
- ADR-0058's "five identical calls end the turn" is now eight, after a notice at three.
