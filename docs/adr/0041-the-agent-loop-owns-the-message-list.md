# The agent loop owns the message list, and reduction replaces whole ranges

`reactor-agent` is the loop [ADR-0033](0033-reactor-is-a-rust-project-on-rig.md)
promised: it holds the session, builds every request itself, and calls the model
through rig's `CompletionModel::stream` — **not** through rig's own agent loop. This
ADR records the choices the build settled that the earlier ADRs left open. Where it
refines [ADR-0036](0036-reactor-owns-its-session-store-format.md) or
[ADR-0037](0037-context-reduction-is-one-budget-manager.md) it says so.

## Decisions

**rig is the provider layer, and the loop is ours.** rig's agent driver owns its
message list, which is the thing this project is leaving pi to stop fighting over.
The seam is one small trait, `Llm { complete(request, on_delta) -> Reply }`, with a
rig-backed implementation (`RigLlm`) and a scripted one for tests. Our messages and
blocks are converted to rig's at that seam and nowhere else, so what is on disk never
depends on a rig release, and reasoning signatures and tool-call ids round-trip.

**A reduction is one stand-in message replacing a range, aligned on tool-group
boundaries.** ADR-0037 describes dropping mechanical entries and summarizing
conceptual ones; done *in place* that leaves an assistant tool call without its result
(or the reverse), which every provider rejects. So the reduction covers a contiguous
range that starts and ends between whole tool groups, and one message stands in for
all of it: the summary (where the mode summarizes) followed by the stub list (where it
drops). The mapping to the ADR's table is exact — `fade` writes stubs only, `compact`
a summary only, `auto` a summary plus stubs for the mechanical entries — and the
summarizer still sees every entry, tool output included, before any bytes leave the
context. The stand-in is *projected*, never stored as a message: the log gains one
`reduction` entry and loses nothing, and a `restore` entry undoes it.

**Two thresholds, not one.** The fade had a single `pct` (0.9 of the window minus
reserve). Owning both strategies needs a target *below* the trigger, or a reduction
that lands just under it fires again on the next tool result. So: reduce when the
context passes `pct` (default 0.9) of `window − reserve`, down to `keep` (default
0.5) of it. This settles ADR-0037's open "keep-window size" with a starting point,
not a conclusion; whether it should adapt is still to be learned from use. Token
counts are estimated (four characters to a token) and *calibrated* from the provider's
reported usage, because hex and code tokenize much denser than prose.

**The keep window never contains an orphaned result, and never excludes the current
instruction.** The plan starts the kept suffix at a user or assistant entry, and no
later than the last user message: a giant tool result inside the current turn cannot
take the task that caused it out of view.

**Truncation and reduction meet at one address.** A tool result is bounded to
`MAX_INLINE_BYTES` (32 KiB) as its head (8 KiB) and its tail (24 KiB) — the tail is
larger because errors, prompts and final lines live there — with a marker naming the
entry that holds all of it. The whole output is kept as a blob in the session
(spilling to disk past 4 MiB so memory stays bounded) and is reached with
`history_read` / `history_search`, the same `#id` a fade stub carries.

**`bash` is a persistent shell.** Named sessions keep `cd`, exports and variables
between calls, because RE is stateful in a way a fresh `bash -c` is not. Output
streams as it arrives. Stdin is `/dev/null` for each command so one that reads it
cannot swallow the script after it. A timeout, a cancelled turn, or `exit` kills the
shell's process group and drops the session, and the result says what was lost —
there is no way to interrupt a child out of a shell still reading its stdin, so the
honest options are to wait or to restart.

**The loop's invariants are rules, not edge cases.** The budget is checked before
every request and after every tool result. A failed reduction stops the turn
immediately (the context did not shrink). Reductions that do not get the context to
fit are counted, and the turn stops after a budget of them. And every tool call in a
recorded reply gets a recorded result — even on cancellation or a failed reduction
mid-batch — so a session can always be resumed and its history always replays.

**Generated blocks are rebuilt every request, in a fixed order** — identity, the
tool registry, skills, manifest, reporting, the current scenario phase — from
`reactor-context` and the catalogue. They are not in the log and so cannot be
reclaimed by a reduction; an unchanged session sends identical bytes and the provider's
prompt cache holds. A scenario's phase is such a block now, with the summaries of the
phases already completed, where the pi extension sent a one-time message that the fade
would eventually have eaten.

**No permission or approval layer** — stated in ADR-0033 and unchanged: REactor is run
inside a VM or container, and nothing here pretends otherwise.

## What is not done

- **The gate is not met.** "A real RE session runs end to end" needs a real model and
  a person; nothing here has spoken to a provider. Everything up to the wire is tested
  against a scripted model. `examples/repl.rs` — a terminal REPL over the same `Agent` the
  GUI will drive — lets someone with an API key run it. It is a development example, not a
  frontend: REactor has no TUI (ADR-0033), and it is never built or installed by default.
- **Budget knobs are not settings yet.** `mode`, `pct`, `keep`, `reserve` and the
  summarizer model are constructed in code; resolving them through
  [ADR-0038](0038-settings-resolve-global-then-session.md) is phase 5's, with the UI
  that shows their scope.
- **Scenarios and authored skills are read from directories.** A bare binary has
  neither; whether to compile them in (as `tools.toml` is) is open.
- **Long-lived background work** beyond a command's timeout is the model's to manage
  with `&` and polling; a job table is not built.
