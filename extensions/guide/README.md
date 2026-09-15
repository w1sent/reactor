# guide

`/guide` opens a popup for the person at the keyboard: what REactor is, what
the footer's blocks mean, and the flows they will actually type in pi —
choosing tools, watching services, anchoring a session with a goal, running a
scenario, fading long sessions.

```
/guide                 open it
↑/↓ · PgUp/PgDn        scroll (the window follows the terminal height)
q / Esc                close
```

Outside a TUI there is nothing to pop over, so the command answers with a
one-line command sheet instead. The overlay carries no state, touches no CLI,
and is never seen by the agent — it is documentation that pops up where the
question arises. The body is one scrollable window, so it stays honest at any
terminal height.
## Keeping it true

The pages are data in `extensions/guide/index.ts` (`PAGES`, one per tool),
and the guide is part of surfacing a tool, not an afterthought: **adding an
extension or tool, or changing one whose commands or vocabulary are
user-facing, updates its guide page in the same change.** The tools index
(`/guide tools`) lists every tool by name; a stale entry is a code change
away, and `guide.test.mjs` pins the names.
