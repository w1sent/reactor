# A running child is followed in a tab and stopped on its own, and a panic ends a turn instead of hanging it

Child agents looked as if they never finished, and nothing could be done about one while it ran.
Two things were behind it. One was a bug: the `#` reference scan in `references::find` stepped by
byte and sliced the text before the current byte, so a task with a multi-byte character such as
`—` panicked the agent's thread (`end byte index … is not a char boundary`). A panic in a turn
meant the turn never reported back: the window stayed "working", a parent waited on a child that
was gone, and `wait` waited for ever. The other was that a child had no handle: it could only be
ended by interrupting the parent, and what it was doing could be seen only as a line on the
parent's tool card, if it was a foreground child at all.

## Decisions

**The scan checks the byte before it looks behind it.** `find` asks whether byte `i` is `#` (one
byte, so on a character boundary) and only then what precedes it. Text with `—`, `“”` or CJK around
`#` has a test.

**A panic ends the turn with an error.** `Agent::prompt` and `prompt_pending` run the turn under
`catch_unwind`. A panic becomes `internal error: <message>. The turn was ended and the log is
intact; this is a bug in REactor.`, the calls of the last reply that have no result are answered
`not run: an internal error ended the turn` so the log still replays, and the agent takes the next
prompt. A child's panic is therefore an error result for its parent, and a background child's is a
report in the `wait` queue, not silence.

**Every running child is listed and can be stopped on its own.** The agent keeps the children it
is running, foreground or background, each with a cancel token derived from its parent's, so
interrupting the parent still stops them all and stopping one leaves the parent and its siblings
alone. A stopped child ends with what it had: its parent gets a result saying it was cancelled,
with what it had said last and where its session is. `Agent::running_children` and
`Agent::stop_children(session | all)` are the API; `/agents` lists them and
`/agents stop [all | <session>]` stops them.

**A running child is followed by clicking it.** In the session list a running child shows as
working, like any session that is. Clicking Open or double-clicking it opens a transcript tab
that is read from the case as the child writes it (about every half second, only when its log
has grown), so what it reads and runs can be watched while it works. The tab takes no message:
the child has nobody to read one, and a note says so. Interrupt in that tab stops that child,
and so does a **Stop** button in the session panel for the picked child. When the child has
finished the tab gives way, and opening it again opens the session itself, which can be
continued like any. Closing the window stops the children with the turns.

**A child has a round limit.** `children.maxRounds` (60, zero for none) caps the model round
trips of one child, within the agent's own limit. At the limit, or on any other failure, the
child's result says why and carries what it had said last, so a stopped child is a partial
report and not nothing.

## Not done

- **The tab polls.** A child's events are not carried to the window; the tab reads its log. A
  second of delay is the cost of not giving a child a second owner in the window.
- **A tab that follows a child has no tree, context bar or chips.** They belong to an agent the
  tab does not have.

## Considered and rejected

- **Opening the child as a full session while it runs.** Two agents would write one log, from two
  copies of it in memory. The live child owns its session until it ends.
- **Streaming the child's every event to the parent's card.** A background child has no card, and
  several together would interleave in one.
