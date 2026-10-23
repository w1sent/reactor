# A scenario moved by hand waits for the next prompt, and that prompt is the summary

> **Amended by [ADR-0052](0052-the-window-is-a-rail-a-drawer-and-a-workspace.md):** the
> stepper is no longer in the Context panel, which is gone. It is numbers down the right edge
> of the session's transcript pane, so that each open session shows its own; a click on a
> number opens that phase's card and a second click goes there. Where this says "the panel",
> read that card and the Scenario chip under the prompt. Everything decided below about a move
> by hand is unchanged.
>
> **Amended by [ADR-0065](0065-a-scenario-is-general-starts-from-a-question-and-can-be-written-for-the-case.md):**
> phases can be left out in the start dialog. They are passed over as `(skipped)` as a jump
> over them would be, and going to one by hand takes it back in.

[ADR-0009](0009-scenarios-advance-by-tool-result.md) has the agent advance a scenario with
`reactor_phase_complete`, and gives the person a manual `next`. The GUI now shows the phases
as a stepper in the Context panel, and the person can go to any phase. This records how a
move by hand differs from the agent's.

## Decisions

**The agent's way is unchanged.** `reactor_phase_complete(summary)` is the only tool, it
moves to the next phase, and its result is the briefing. The stepper follows the stored
state, whoever changed it.

**A move by hand starts no turn.** `/reactor-scenario goto <n>`, `next` and a click on a
phase set the current phase and return. What the agent was doing is over — or was
interrupted — and the briefing is already in force: it is the Scenario block of the system
prompt, rebuilt every request ([ADR-0030](0030-gated-tools-advertise-by-state.md)). The
person's next prompt starts the phase. Starting a scenario is not a move by hand and still
opens with its first briefing as a turn.

**The next prompt is the summary of the phase that was left.** Going forwards leaves the
left phase's summary pending (`pendingSummary` in the stored state); the notice and the
panel ask for "a line or two" on what it concluded, and `Agent::prompt` records that
prompt, whitespace-collapsed and cut to 600 characters, as the summary before the turn runs.
A summary given with the command (`goto 4 found the packer`, `next …`) is used at once and
nothing is pending. Phases jumped over are `(skipped)`. Going back drops the summaries of
the phases that will be done again.

**Moving by hand is for between turns.** The stepper is disabled while the agent works, and
the GUI refuses `goto`/`next` then. Abort is allowed at any time.

**A phase whose toolset has missing tools still starts.** The toolset is enabled as before
(advisory, [ADR-0007](0007-deactivation-is-soft.md)); the registry lists only tools that are
present, so the agent is never told about the missing ones. The panel tells the person.

## Consequences

- `next` changed: it no longer sends the briefing as a turn. The golden replays of the
  scenario port were updated for it; the extension they were taken from is gone
  ([ADR-0043](0043-the-pi-flavor-is-removed-from-the-tree.md)).
- Stored state gains `pendingSummary`, omitted when empty: old sessions load unchanged.
- Authoring scenarios in the GUI is deferred (`TODO.md`).
