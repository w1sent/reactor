# The window is a rail, a drawer and a workspace, and a session's state sits in its own pane

The window had ten dockable panels and one right-hand dock that held six of them as tabs.
The tab strip did not fit them, every panel header carried its own row of icons, and what
belongs to a session was spread over three places: three switches above the prompt, a
Context panel with two tabs, and a status bar. With several transcripts open, all of that
showed the active session's state and none of the others'.

This records how the window is arranged now. It amends where three earlier decisions put
things ([ADR-0044](0044-filesystem-panels-read-through-a-vfs.md),
[ADR-0048](0048-mcp-tools-are-catalogue-entries-the-model-gets-as-definitions.md),
[ADR-0049](0049-a-scenario-moved-by-hand-waits-for-the-next-prompt.md)); what they decided
about behaviour stands.

## Decisions

**A rail holds the panels one picks from; the rest is the workspace.** The rail is a strip
of icons at the window's left edge: Sessions, Toolbox, Services, Files. One of them at a
time is shown in a drawer beside it, whose edge is dragged to make it wider or narrower.
Transcripts, the graph, the documents, the console and the viewers a file opens in are tabs
of the workspace, which is gpui-kit's dock unchanged: tabs
are dragged, split and grouped with its own drag and drop, and no docking of our own is
built. A rail panel is the same view wherever it is shown, so it can be made a tab of the
workspace when it is wanted in view for longer, and taken back; it is in one place at a
time. The console is a workspace tab only.

**The Toolbox is one tool list, and toolsets are tags above it.** Tools and Toolsets were
two panels. A toolset is not a group of the list: a tool can be in several, because a
toolset is a tag list that intersects ([ADR-0013](0013-toolset-tags-intersect.md)). So each
tool is listed once, and the toolsets are tags: a click toggles one, an active one has a
dot, and resting the pointer on one shows its numbers and brings its tools forward. A tool's
row says why it is on — through which active toolsets, or pinned by the person — which is
the `override` the catalogue already reports
([ADR-0011](0011-selector-edits-overrides-not-outcomes.md)).

**What belongs to a session is drawn in that session's transcript pane.** Top to bottom:

- the usage bar, whose details open in place and push the transcript down;
- the goal line, while the manifest is on: the goal, the editor and the Frame as icon
  buttons, and, opened in place, the guidelines and the steps the agent keeps;
- the transcript, with the running scenario's steps as numbers down its right edge;
- the prompt, with the model and one button that sends and, while the agent works,
  interrupts;
- the chips.

Each pane shows its own session's, whichever session is the active one, so two sessions
side by side each say where they stand. A click in a pane acts on that pane's session.

**The per-session switches are chips, and what is off is behind one of them.** Other
sessions, case graph, documents, reporting, goal, scenario and identity are each a chip
while they are on. What is off is listed by an *Activate* chip and nowhere else. A plain
chip turns off on a click; one with settings (reporting, goal, scenario, identity) opens
them, and they end in a *Turn off* button, which for a scenario is *Stop scenario*. The
context budget is a chip too and is always there, first, without the dot of a thing that
is on: a mode always applies, so it cannot be turned off.

**The scenario's stepper is numbers, and a step is read before it is gone to.** A click on
a number opens that step's card: its title, what it concluded or which toolset it asks for,
and which of those tools are missing. A second click, or a click with Ctrl held, goes
there. Everything [ADR-0049](0049-a-scenario-moved-by-hand-waits-for-the-next-prompt.md)
decided about a move by hand is unchanged: it starts no turn, the next prompt is the
summary of the step that was left, and the stepper is disabled while the agent works.

**The Context panel and the status bar are gone.** Their parts have the homes above. The
embedding progress, which the status bar showed as two numbers, is a ring on the rail while
documents are being embedded, with the numbers on a click.

**Panels this does not name are not redrawn.** The graph, the console, the file explorer,
the session tree and the services list are moved, not changed; the documents panel stays a
workspace tab, where its passages have the room to be read.

## Why

**The right dock was a drawer already, used as a dock.** Six panels shared one strip that
showed one at a time. A rail says that honestly, gives each a fixed place to click, and
leaves the dock to the things that are arranged rather than picked from.

**State that is per session has to be shown per session.** The window could already hold
several transcripts, but everything about a session other than its conversation followed
the active one. Side by side that reads as the wrong session's goal and budget on the pane
one is looking at.

**A switch that is off does not need a place.** Seven switches drawn all the time are
seven things to read before the prompt. Shown only while on, the row under the prompt is
the list of what this session draws on, and the one chip that lists the rest answers "what
could I turn on".

**Grouping tools by toolset would have listed most tools several times.** Tags keep one
row per tool and still show membership, on demand.

## Consequences

- `PanelKind::Context`, `PanelKind::Tools` and `PanelKind::Toolsets` are gone; there is
  `PanelKind::Toolbox`, and `/panel tools` and `/panel toolsets` mean it.
- The four layout presets place workspace panels only. Their side docks are empty; a rail
  panel a preset names (the Toolbox in Catalogue) is made a tab.
- A session's plan, scenario, switches and identity are kept with the session when it is
  not the active one, as its context view already was.
- gpui-kit's rule that a dock holding one lone tab group cannot have its tabs dragged out
  still applies (`layout.rs` has the detail).
- Not built: hiding a rail icon, moving the rail's panels to the right edge, saving a
  layout under a name, and tearing a tab out into its own window.

## Considered and rejected

**Keeping the Context panel and moving only the bar.** The plan tab's parts are used at
different rates: the scenario's steps every few minutes, the goal rarely, reporting almost
never. One panel made the frequent one as far away as the rare ones.

**Toolsets as collapsible groups of the tool list.** A tool in three toolsets would be
three rows, and toggling one of them would raise the question of what the other two show.

**A switch on each toolset beside a click that filters.** A click on a toolset row toggles
it today, and a second control for the same thing next to it was one too many. The pointer
resting on a tag shows what a filter would have.

**A docking mechanism of our own, with drop zones and labels.** gpui-kit's dock already
moves, splits and groups tabs, and a second mechanism would have to agree with it.

**A horizontal stepper in the goal line.** It took the width a goal needs, and a scenario
with more than a handful of steps did not fit a narrow pane.
