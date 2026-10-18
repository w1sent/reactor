# Children run in parallel, can be started in the background and waited for, and are linked to their parent

[ADR-0062](0062-a-subtask-goes-to-a-child-agent-and-only-its-report-comes-back.md) gave the agent a
child and left four things for later: running several at once, waiting for one started earlier, a
link from a child's session to its parent's, and a way to see that link. This does them. Token cost
stays per session: a child's tokens are in its own log and in the footer of its report, and nothing
sums them into the parent's, by decision.

## Decisions

**A child's session points at its parent's.** `session.parent_session` (migration 8, nullable, a
reference to `session`, indexed) is set when the child's session is made (`Store::create_child`).
It is empty for a session a person began. `sessions::list` returns it, `set_parent` refuses a
session as its own parent or a session that is not there. The `origin` entry in the child's log
stays: it holds the task.

**The session list is a hierarchy.** A child's row sits directly under its parent's, one step in
and with a line down its left edge, tagged "agent of #3"; roots keep the list's order (newest
first) and so do children among themselves. A child whose parent is not in the list (an empty
session the list leaves out) is a root, and a ring in the links cannot hang the listing. Nothing
else about a row changes: it opens, closes and shows its state like any session.

**Children asked for in one reply run together.** When a reply holds several `agent` calls, they
run at once, up to `children.maxParallel` (4) at a time, and their results are entered in the order
the calls were made, like every other call's. The other calls of that reply run after them, in
order. A child that fails is still only an error result. `children.parallel` (settings.json, on by
default) turns it off, and then they run one after another as before; `/children` shows the state
and `/children parallel [on|off]`, `/children serial` and `/children max <n>` change it. It is a
global setting, read for every reply. Its cost is the cost of the work: several provider calls in
flight, which a rate limit can refuse (ADR-0057 retries those).

**A child can be started in the background and waited for.** `agent` with `background: true`
returns at once with the child's session. `wait` blocks until a child started that way finishes
and returns its report: the one named by `session`, or the next to finish, or, when none is
running, says so. It ends when the turn is interrupted. A child that finished before `wait` was
called is held until it is. Background children share the limit of `maxParallel` and are cancelled
with the turn (an interrupt); they have no tool card to report progress to, so their progress is
their session, which the list shows. A report not waited for is not lost: the session has it.

**A child has neither tool.** `agent` and `wait` are the parent's; a child does not start children
([ADR-0062](0062-a-subtask-goes-to-a-child-agent-and-only-its-report-comes-back.md)).

## Not done

- **A report is not delivered when the child settles.** The parent has to call `wait`. Delivery on
  settle would need an entry that arrives while the turn is not asking, which is what the inbox
  ([ADR-0061](0061-typed-input-is-a-fact-in-the-log-before-it-is-sent.md)) is for; it can ride that
  when it is wanted.
- **The startup session picker is still flat.** It lists sessions before a case is open.

## Considered and rejected

- **Parallel by running every tool call of a reply at once.** Tool calls have effects on the
  filesystem and the shell in an order the model wrote; children are separate sessions with their
  own shells. Only they are run together.
- **A session-level parallel switch.** The cost and the rate limit are properties of the provider
  and the machine, not of a session ([ADR-0038](0038-settings-resolve-global-then-session.md)).
- **Summing a child's tokens into the parent.** Not wanted: cost is per session.
