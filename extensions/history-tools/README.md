# history-tools

Line-addressed recovery over the session history: `history_index` orients,
`history_search` finds, `history_read` reads. The session file is serialized
once per call into a stable line-indexed text; every answer carries line
numbers so the next call can be addressed against them.

On by default in **any** session — useful under pi's own compaction, under the
fade, and with no compaction at all, which is why they are their own extension
([ADR-0024](../../docs/adr/0024-rolling-context-splits-into-goal-setting-history-tools-and-the-fade.md)).

`/history-tools [on|off]` is the user's lever for a model that calls them too
often; a disabled tool answers with the command that re-enables it, and the
choice rides its own per-session entry so it survives a reload.

Config in `~/.pi/agent/pi-history-tools.json`: `readBudget` (paging cap for
`history_read`), `searchHitLimit`, `contextLines`.
