# A scenario is general, starts from a question, and can be written for the case

> Amends [ADR-0017](0017-scenario-steps-are-read-directly-not-pi-prompts.md) (a third
> frontmatter field, a lookup over three places, and scenarios the agent writes),
> [ADR-0051](0051-the-scenarios-are-compiled-in-and-seeded-by-setup.md) (the config directory
> is one of three places, not the only one) and
> [ADR-0049](0049-a-scenario-moved-by-hand-waits-for-the-next-prompt.md) (phases can be left
> out before the start). `CONTEXT.md`'s "scenarios do not carry state into the next" no longer
> holds: what each phase concluded stays in the case's graph.

Three things went wrong in use. A scenario assumed another had run before it: `investigation`
opened on `evidence/INVENTORY.md`, which only `acquisition` writes, so most of the time the person
first had to explain the case in a prompt of their own and then start the scenario. The briefings
named files and a layout instead of saying how to do the stage, so they read as instructions for one
case's folder. And a case with a narrow target (one host, one question) fitted none of the shipped
scenarios: the person triaged it with the agent, agreed on a plan, and had no way to make that plan
a scenario.

## Decisions

**A scenario starts from a question, and the question is the session's goal.** Starting one opens a
dialog with the scenario's description, a question field prefilled with the current goal, and the
phases as checkboxes. What is in the field when Start is pressed becomes the manifest's goal
(authored by the analyst); an empty field leaves the goal as it is. The goal survives every
reduction already and is in every request, so the scenario needs no store of its own for it, and
every phase sees it. `/reactor-scenario start <id> [skip=2,4] [question…]` does the same from the
prompt; everything after the id and the optional `skip=` is the question, line breaks kept.

**Phases left out are skipped, not removed.** The unticked phases are kept in the stored state
(`skip`, omitted when empty). The scenario starts on the first phase that is not skipped, and
moving on, by the agent or by hand, passes over them recording `(skipped)`, so the stepper still
shows every phase in its place and the numbers stay those of the files. Going to a skipped phase by
hand takes it out of `skip`: the person changed their mind.

**A briefing says how to do the stage, not where things are.** Every shipped phase now has the same
parts: *Goal*, *Applies when* (the kind of evidence or question that makes it relevant), *Orient*,
then the activities, decision points, pitfalls and *Done when*. *Orient* replaces "Entry criteria:
the X scenario has finished": look at the goal, the case's files and the conclusions earlier
scenarios left in the graph; state the assumptions made; go on; and ask only when something the
question cannot be answered without is missing. No phase names a file the case must have.

**A scenario has a description.** `description:` in the frontmatter of the first phase file (in
any other it is ignored). The chooser and the start dialog show it, and the agent reads it in the
list when it suggests one. Still no manifest file: the phases are the scenario.

**Scenarios are looked up in three places, the case first.** `<case>/.reactor/scenarios/`, then
`<config dir>/prompts/scenarios/`, then the checkout's `prompts/scenarios/` when there is one
(`REACTOR_PACKAGE_ROOT` or beside the binary). The first that has the id wins, so a case can tailor
a global scenario under the same name. The case's are in `.reactor/`, so they move with the case
when an unsaved state is saved and when a case is copied. The format is the same everywhere.

**The agent writes scenarios; the person starts them.** `scenario_save` writes one (`id`, `scope`
case or global, `description`, the phases) as `NN-slug.md` files. It does not start it. The
transcript shows the saved scenario as a card with *Start here*, *Start in a new session* and
*Edit*; the first two open the start dialog. The intended flow is a short triage in a session, a
discussion of what the investigation should look like, a saved scenario, and a start. A new
session in the same case begins with an empty context, so the question in the start dialog has to
carry what the triage found; the dialog says so.

**Scenario editing is a switch, like Documents.** `scenarioEditing` in the memory settings
(on unless a layer turns it off) decides whether `scenario_list`, `scenario_save` and
`scenario_amend` are offered; the session's *Scenario editing* chip turns it off and *Activate*
turns it on again, as for *Other sessions*, *Case graph* and *Documents*. It is for a session that
should only run scenarios, and saves their definitions in every request. Running a scenario,
`reactor_phase_complete` and the editor window do not depend on it, and a call made after it was
turned off is told why.

**A running scenario can be amended, from the current phase on.** `scenario_amend` replaces the
phases *after* the current one. The phases done and the one in progress cannot change, so the
stored index and summaries stay right without copying the phases into the state. The amendment is
written as a case scenario under the same id (all phases, the earlier ones copied as they were),
so it changes this case and not the global or shipped one. Its rule of use is that the person
agreed to the change first; it is a rule, not a gate, like the rest of a scenario
([ADR-0007](0007-deactivation-is-soft.md)).

**What a phase concluded stays in the case.** Each phase summary, from `reactor_phase_complete`
or from the prompt after a move by hand, is also written to the graph as an `analysis.note` tagged
`scenario`, with the scenario, phase number and title in its properties, when the case graph is on
for the session. A scenario that starts afterwards lists the most recent of them under its first
briefing, which is what *Orient* reads first. `(skipped)` and `(advanced manually)` are not written.

**A case scenario is promoted to global by the agent.** *Promote to global* (in the editor, or
`/reactor-scenario promote <id>`) sends the agent a prompt to rewrite the scenario without the
case's host names, paths, accounts, indicators and other specifics, and save it with scope global.
The case copy stays.

**The editor is a window.** Like the identity editor: the id, the scope, the description, and the
phases, each with a title, a toolset picked from the existing ones (or none) and a body; add,
remove and move phases. Saving writes the same files the tool does, through the same function. A
shipped or global scenario opened with scope case is saved as a case copy; a shipped one saved
global becomes the user's file, as an edit always did
([ADR-0004](0004-config-updates-via-plain-diff.md)).

**`reactor doctor` checks the scenarios the user wrote.** For the global and the current case's
scenarios that differ from the shipped ones: a phase with no title or no body, a `toolset:` that
is not in `toolsets.toml`, numbers that repeat or have gaps, a file that is not `NN-name.md`, a
directory with no phases, and a scenario with no description. Each is a `scenario-invalid`
problem naming the file. A scenario that is broken still loads; doctor says why it reads oddly.

## Consequences

- Reading and writing the format moves to `reactor-core::scenarios` (`Library`, `parse_phase`,
  `save`, `lint`), so doctor can use it. `reactor-context::scenario` takes a `Library` instead
  of a directory.
- The stored state gains `skip`; old sessions load unchanged.
- A scenario renamed or removed while it runs is handled as before: the stepper shows nothing and
  `stop` clears it.
- The phase summaries in the graph are notes. They can be found with `graph_find` (tag
  `scenario`) and revised or reverted like any other annotation.

## Considered and rejected

- **Snapshotting the phases into the state at start.** That would make editing a running scenario
  harmless, but it would make `scenario_amend` and the editor change a copy the stepper does not
  read. Allowing amendment only after the current phase keeps the index valid without it.
- **A separate brief stored with the scenario.** The goal is already that, already survives
  reductions and is already shown. A second "question" block would say the same thing twice.
- **Referencing shipped phases from an authored scenario** (`from: investigation/05-timeline`).
  A copy is self-contained and can be tailored; a reference is one more field and one more lookup,
  and silently changes when a release does.
- **Letting the agent start the scenario it wrote.** Starting one changes the goal and the system
  prompt for the rest of the session; that is the person's call.
