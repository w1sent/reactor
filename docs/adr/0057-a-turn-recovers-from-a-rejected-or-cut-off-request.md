# A turn recovers from a rejected or cut-off request, a bounded number of times, and says what it did

[ADR-0037](0037-context-reduction-is-one-budget-manager.md) made reduction *proactive*: the
budget is checked before every request and after every tool result. It said nothing about
what happens when that is not enough, and the loop did nothing: a provider's "prompt is
too long" ended the turn as a string error; a reply the output limit cut off while the model
was thinking looked like a finished turn with nothing in it; a busy server ended the turn;
and the summarizer, which has a window of its own, was sent whatever the range came to.
Small windows (a local model at 16k to 50k) hit all four far more often than a large one.

This records what the loop does about each. Every rule is bounded: it tries once, or a few
times, and then stops and says what it tried.

## Decisions

**Failures have kinds, and the kind decides.** A model error is one of `Overflow` (the
provider found the request too long), `Blocked` (its policy stopped it), `Transient` (a rate
limit, a busy or unreachable server, a stream that went quiet) or `Model` (anything else,
passed on as it is). `classify.rs` reads them from the provider's text; the phrase list is
ours, informed by the ones pi collected, and grows a line at a time when a provider is found
that says it differently. A rate limit that mentions tokens is not an overflow. Why a reply
ended is read the same way (`StopKind`: stop, tool use, length, filtered).

**A rejected request is reduced and sent again, once.** The reduction aims at 0.6 of the
usual keep window, because the estimate was low; the estimate's correction (ADR-0041's
calibration) is raised by a quarter so the budget believes it less from then on. A second
rejection, or a request with nothing older to reduce, stops the turn with a message that says
the window may be smaller than the one REactor assumes, and where to set it.

**A reply the output limit cut off is not a finished turn.**
- Cut off in the middle of tool calls: none of them is run (their arguments may be unfinished).
  Each is answered `not run: …` so the log stays valid, and the model is told why and asks
  again. Three such replies in a row stop the turn.
- Cut off with nothing said or done (the model thought until the limit): the reply is not
  recorded. The request is made again with half as much room again, after a reduction if the
  limit had been cut to what the window left. A second time stops the turn.
- Cut off with text: kept, and a notice says so.

**Every request carries a reply limit and the window it was sized for.** The limit is the
setting if there is one, else a quarter of the window up to 32k, and is always cut to the room
the window has left (window, less the context at the learned ratio, less 512 and 2%), never
below 256. Anthropic needs one, and rig knows only some model names' defaults; for the rest
the request used to be refused. An Anthropic thinking budget is kept under the limit with at
least 1,024 tokens left to answer with; if it cannot be, thinking is off for that request and
the request goes. Ollama sizes its window per request and silently cuts the prompt to its own
default otherwise, so its requests carry `num_ctx` — the window.

**A server that fails in a way worth another go gets up to three, with a pause that doubles
from a second,** each said in a notice. The call is watched: a model that says nothing for five
minutes (not even a thought) is given up on as a transient failure, since a hung local model and
a thinking one look the same until then. Both waits end when the turn is cancelled.

**A blocked request is not retried,** and the message says it was not and what to do. The
same request would be blocked again, and a retry costs a full input.

**A reply the provider's filter stopped is treated the same way.** Every provider that has one
reports it as a finish reason of its own (a refusal, a safety stop, `content_filter`), which rig
reads as one thing. Stopped before the reply said or did anything, it ends the turn as blocked,
unrecorded and not retried. Stopped partway, what it said is kept and a notice says it was
stopped. A tool call in it is not run, since its arguments may be unfinished: it is answered
`not run`, the model is told, and three such replies in a row end the turn as blocked. A summary
the filter stopped is blocked too, which sends the reduction to the main model and then to a
fade. Analysing malware puts text in front of a filter that the same model would not see
elsewhere, so this is a case to expect rather than an error to be surprised by.

**A reduction can be cancelled.** A summarizer that never answers used to hold the turn with
no way out. Nothing is written until the summary is in, so cancelling leaves the log as it was.

**The summarizer has a window too.**
- The range is sent as entries, and is fitted to a quarter less than the window before it is
  sent: tool output is cut shorter in steps, then the oldest entries are left out (what an
  earlier reduction kept goes last), and what was left out is said.
- If the model still finds it too long, it is asked again at 70%, then 50%, then 35% of the
  size that was rejected, each strictly smaller than the last. After that it gives up and says
  the text was cut to a third and did not fit.
- Its reply may be at most half the window; it is told the window.
- A busy summarizer is tried twice more. One that fails is replaced once by the main model if
  that is a different model, and the error names both when both fail.

**Two messages of the person's in a row are one,** when the conversation is handed to the
provider. Some providers refuse adjacent turns of one role, and the rest read them as one.
Nothing needs it yet; the cut that keeps your last message in front of a stand-in will.

## Why

pi collects the provider phrases and, on a length stop below its own limit, compacts and
tries again once; it fails the tool calls of a truncated reply instead of running them.
opencode's steps of 70, 50 and 35% are for a compaction that is itself rejected. goose removes
more and more tool output from a summary request that overflows, and compacts reactively at
most twice. cline retries a cut-off turn three times, and gives a different terminal message
for "nothing to compact", "still too big after compacting" and "compaction is off". hermes
sorts errors into kinds that each carry their recovery, and asks Ollama for the window it
really serves. What they share is that every recovery is bounded and none is silent.

## Consequences

- `Error` gained `Overflow`, `Blocked` and `Transient`. `Error::is_model` says whether an error
  came from the model or the connection.
- `LlmRequest` gained `num_ctx`; `SummaryRequest` gained `window`, `input_cap` and `parts`;
  `LlmSummarizer` is built with `LlmSummarizer::new` and takes a fallback. `AgentConfig`
  gained `idle_timeout`, `transient_retries` and `retry_delay`.
- The notices are events (`Event::Notice`): a frontend that shows them lets a person see that a
  turn is retrying and why, which is the difference between a slow turn and a stuck one.
- A summarizer that falls back to the main model does so without saying so. It costs a second
  call only when the first has failed.

## Not done

- A reply that *succeeds* with more input than the window (a provider that silently accepts an
  overflow) is not detected as one; the calibration from reported usage catches it on the next
  request instead.
- The summarizer is assumed to have the main model's window. A summarizer setting that names
  a model with another window would need a window of its own.

## Considered and rejected

- **Ask a cut-off model to be more concise** (cline). It adds a message that stays in the log
  for good, and it answers the wrong question when the limit was the room, not the verbosity.
  A retry with more room, after a reduction if the room was the problem, answers it.
- **A synthetic "continue" after a cut-off text.** [ADR-0037](0037-context-reduction-is-one-budget-manager.md)
  removed these for the same reason: they pile up in the sessions that need them least.
- **Refuse a model with a small window** (hermes does, below 64k). The point is that a small
  window should work, even if less comfortably.
- **A regular expression for each provider's wording.** Phrases on the lowercased message are
  as exact as the messages are, are readable, and a wrong one is a line to delete.
- **Retry a blocked request unchanged.** It is blocked again, and a retry costs a full input.
