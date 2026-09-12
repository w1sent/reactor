# rolling-context

The fade: an alternative to pi's summarization compaction, for models with a
small context window and a compaction mechanism that fixates on old content.
Off by default; `/rolling on` opts the session in.

Instead of summarizing old messages — which tends to lose the thread on *why*
something was done, not just what — the fade leaves the session file untouched
and sends only the newest messages that fit a configurable budget. What was
dropped is recoverable via the history tools, and the session manifest
(goal + steps, from `goal-setting/`) is what survives on purpose.

Only pi's *proactive* threshold compaction is cancelled; manual `/compact` and
overflow recovery are left alone. Config in
`~/.pi/agent/pi-rolling-context.json`: `pct`, `reserve`.

Why it never summarizes, why the recovery path must stay reachable, and why
it measures and cuts exactly the way pi does:
[ADR-0019](../../docs/adr/0019-rolling-context-ships-here-general-purpose.md)
and [ADR-0020](../../docs/adr/0020-rolling-context-measures-and-cuts-like-pi-does.md).
