# A call chooses how much of its output it sees, and the cut says how to read the rest

> **Amended by [ADR-0058](0058-reduction-is-a-ladder-and-the-budget-follows-the-window.md) (built):**
> unset, the default of `toolOutput.maxInlineBytes` follows the window — a twentieth of it
> (a fifth of its token count, in bytes), between 2 KiB and 32 KiB — and a limit the person set is theirs whatever the window. The 32 KiB
> below is what a large window gets.

[ADR-0041](0041-the-agent-loop-owns-the-message-list.md) bounded every tool result to a
fixed 32 KiB, its head and its tail, with a marker naming the entry that holds the whole.
Two things were missing. The agent could not say beforehand that it wanted more, or less:
a 60 KiB listing it needed whole took a cut and then a `history_read` for the middle, and a
command it only ran to see the exit status still cost 32 KiB of context. And the marker gave
the entry and its size in bytes, while `history_read` pages by line, so the agent had to
guess where the middle began.

## Decisions

**The default limit is a setting.** `toolOutput.maxInlineBytes` in `settings.json` (shown in
the GUI's Tools settings, in KiB) is the limit a call gets when it does not ask for one. It
defaults to 32 KiB and is kept between 1 KiB and 1 MiB. It is global only: unlike the
context budget ([ADR-0038](0038-settings-resolve-global-then-session.md)), a session has no
layer of its own for it, because the agent can already choose per call.

**A built-in tool takes `max_output_bytes`.** It replaces the default for that one call,
within the same bounds. Head and tail keep their shares, a quarter and three quarters.
`bash` holds the head and tail of a spilled output at that size, so a larger limit is not
cut short by its buffers. The parameter is advertised only on the tools whose output can run
long (`bash`, `read`, `history_index`, `history_read`, `history_search`, `graph_query`,
`docs_search`), because every advertised parameter costs context on every request. A tool
that answers in a line has no use for it. The loop honours it on any built-in tool all the
same.

**An MCP tool's output is cut at the default.** Its parameters are its server's, and an
argument the server did not declare cannot be added to its schema or taken out of its
arguments without guessing. What the agent does with an MCP output that was cut is the
next decision.

**The marker says the output was truncated and gives the `history_read` call for the
rest, and nothing else:**
`… [output truncated; the rest: history_read {"id": 42, "offset": 410, "limit": 4713}] …`.
The call names the lines that are not shown, counted the way `history_read` counts them,
and can be made as written. The range may overlap the shown head or tail by a line, but it
never leaves a gap. The agent can also page the rest in smaller pieces from the same
offset, which is the same choice a built-in call makes with `max_output_bytes`.

## Consequences

- Seven tool definitions grew by one small parameter. Two loop tests whose context windows
  were tuned to within a hundred tokens of the old definitions got a little more room.
- A careless large limit fills the context sooner. The budget manager handles it as it
  handles any large result ([ADR-0037](0037-context-reduction-is-one-budget-manager.md)),
  and 1 MiB is the ceiling.
- `history_read` is itself a tool whose output is cut. A page bigger than the limit is cut
  again, and its own marker points at the new entry; a larger `max_output_bytes` or a
  smaller `limit` avoids that.

## Considered and rejected

- **No cut, with the agent choosing every time.** A tool's output size is not known before
  it runs, and one forgotten limit on a `strings` dump would fill the window. A default
  that applies when the agent says nothing keeps the safe case the easy one.
- **Wrapping MCP tools' schemas with the parameter.** The server could reject the unknown
  argument, and stripping it before the call means REactor edits arguments the model meant
  for someone else. Paging through `history_read` gets the same result without touching
  the server's contract.
- **Advertising the parameter on every tool.** It is noise on `write`, `edit`,
  `update_steps` and the graph writes, whose results are a line. It is also what pushed the
  tightly tuned tests over their windows: a measurable cost for no use.
- **A head/tail split chosen per call.** A second parameter for a rare need. Asking for a
  smaller limit and paging the part that matters covers it.
- **A marker that also gives sizes in bytes and lines, and names `history_search`.** The
  call already holds the entry and the line range, and `history_search` is described with
  the tool itself. Every cut output would repeat that text.
