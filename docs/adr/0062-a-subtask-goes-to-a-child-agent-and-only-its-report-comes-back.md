# A subtask goes to a child agent, and only its report comes back

> **Extended by [ADR-0063](0063-children-run-in-parallel-in-the-background-and-are-linked-to-their-parent.md):**
> children run in parallel, can be started in the background and waited for, and are linked to
> their parent. Cost stays per session, as decided there.

Reverse-engineering work is output-heavy: a per-sample enumeration, a query across binaries, a
batch of carving jobs. Done in the parent's turn it spends the parent's window on bulk output the
parent needs one paragraph of. This adds one tool that gives such a task to a child, which spends
its own window on it.

## Decisions

**One tool, `agent`, with a `task` and optionally the names of the `tools` the child gets.** The
child runs its own loop (`Agent::prompt`) with its own message list
([ADR-0041](0041-the-agent-loop-owns-the-message-list.md)), its own budget and its own reductions.
It does not see the parent's conversation: the task is everything it knows, which the tool's rule
says in one line ([ADR-0056](0056-a-tool-says-what-it-does-and-the-rules-are-one-line-each.md)).
Only its last message returns, as the parent's tool result, with a footer naming the child's
session, its rounds and calls and its tokens (input, of which cached, and output).

**The child is a session of the same case.** It is created like any session
(`Store::create_next`), so its whole transcript is in `analysis.db` and can be opened, read with
the history tools and searched later, and its writes go through the same single writer thread
([ADR-0046](0046-the-analysis-database-is-sqlite-inside-the-case.md)). Its log begins with an `origin`
state entry naming the parent session and the task. The case's goal and guidelines are the case's,
not the session's, so the child works toward the same ones; scenario, steps and the rest of a
session's state start empty.

**What the child has is what it is told it has.** A child is built from the parent's model,
summarizer, configuration and tools, minus `agent`: children do not start children. With
`tools` it is given only those; a name that is not a tool is left out and the result says so.
This is advertisement, not a wall: the child still has `bash` unless it is left out, and `bash`
reaches anything its user can ([ADR-0007](0007-deactivation-is-soft.md), [ADR-0033](0033-reactor-is-a-rust-project-on-rig.md)). There
is no approval step, because there is no approval model; the parent's call is the approval, as
every other tool call of the parent is. A child's system prompt is made by the same code as any
([ADR-0006](0006-registry-injected-into-system-prompt.md)).

**The call waits for the child (unless it is started in the background, ADR-0063), and the parent's cancel is the child's.** While it runs, the
parent's tool card shows a line per child tool call and per notice. Interrupting the parent
cancels the child, which keeps what it did in its log. A child that fails (the model is down, it
was stopped) is an error *result*: the parent decides what to do, and the turn goes on.

**The tool exists only where the agent was built with it** (`Agent::enable_children`). The window
does; a test rig or a headless caller that does not is not offered a tool it cannot run.

## Not done

- **Cost is attributable, not summed, and meant to be.** Each child's tokens are in its own log and
  in the footer of the result. The parent's counters do not include them.
- **Findings do not enter the analysis graph by themselves.** The report is for the analyst; the
  graph's tools and pull are how structured findings get in ([ADR-0047](0047-the-analysis-graph-is-the-case-s-model.md)).
- Running several together, a wait, and a link to the parent are in ADR-0063.

## Considered and rejected

- **A hub or daemon that owns sessions** (cline's hub, pi-durable). The window hosts the agent in
  its process ([ADR-0042](0042-the-gui-hosts-the-agent-in-process.md)); a child is a task inside it.
- **codex's message board between agents.** A governance layer between peers. The shape taken is
  its single tool, report-only, and an isolated loop, without the board.
- **Children that start children** (cline allows it). A tree of agents each with a window and a
  provider bill, under a model that cannot see the tree. Not in v1.
- **Another model per child.** The child follows the parent's model; a `model` argument is a
  line when a case needs it.
