# reporting's enforcement is a filesystem probe, not the heuristic ADR-0007 rejected

`extensions/reporting/` can, at its strictest level, actually revert an
agent's last turn and re-demand documentation. That looks like it contradicts
[ADR-0007](0007-deactivation-is-soft.md) -- "REactor never enforces anything"
-- so this records why it's a different case, not an exception carved out of
the same one.

## Why this isn't the case ADR-0007 rejected

ADR-0007 rejected one specific mechanism: deciding whether a *tool* is in use
by pattern-matching a bash command line, then blocking it. That's a heuristic
in the precise sense that matters -- it can only ever guess, because a bash
command line is unstructured text and the real question ("did this invoke
`frida`") has no reliable answer from the string alone. An absolute path, an
alias, or `python -c "import frida"` all defeat it. Shipping that as
enforcement produces false confidence: a mechanism the user believes holds,
that doesn't.

`reporting/`'s question is different in kind: "does the reporting folder's
content on disk differ from the last time it was checked." That has a
mechanical, exact answer -- `readdirSync` + `statSync`, size and mtime per
file, diffed against the last snapshot. There is no guessing step. It is the
same "ask the machine directly" stance `status/` already takes for whether a
service is up, pointed at a folder instead of a process, and it catches every
way the folder can change -- the `write`/`edit` tools, a `bash` redirect,
`git checkout`, a hand edit in another window -- because none of them are
special-cased. Nothing here parses a command line.

## Why it's still opt-in and off by default

The exactness of the detector doesn't make forcing documentation on every
session a good default -- it's a workflow preference, not a machine fact
REactor should assert on anyone's behalf. `reactor-reporting` ships disabled
(`/report on`, or level 0 by default even once on) for the same reason
`rolling-context/` ships disabled: a general-purpose behavior change belongs
to the person running the session, not to the package.

## The revert mechanism (level 2)

Reverting needs `navigateTree`, which pi deliberately does not expose to
ordinary event handlers (`ExtensionContext`) -- only to `registerCommand`
handlers (`ExtensionCommandContext`). Two facts, verified against pi
0.84.2's shipped `dist/` (also recorded in `docs/pi-api-notes.md`), bridge
that gap:

1. `pi.sendUserMessage(text, { expandPromptTemplates: true })` reaches
   `AgentSession.prompt()`, which checks whether `text` starts with `/`
   *before* it looks at whether the agent is streaming, and if so hands off
   to `_tryExecuteExtensionCommand(text)` -- built with
   `this._extensionRunner.createCommandContext()`, the same
   `ExtensionCommandContext` a real `/foo` invocation gets. That's how
   `agent_settled` (a plain event) reaches a command handler that has
   `navigateTree`.
2. `navigateTree` throws if the agent is still streaming, and (unlike
   `reload`/`newSession`/`fork`/`switchSession`) does **not** invalidate the
   calling `ctx` -- it moves the leaf pointer within the same
   `SessionManager` rather than swapping sessions. So the revert triggers
   from `agent_settled` (fired once `_isAgentRunActive` is already false),
   never from `turn_end` (fired mid-loop, while a multi-tool-call turn is
   often still actively streaming) -- calling `navigateTree` there would
   throw.

`/reactor-report-enforce` is the command this dispatches to. It is
registered like any other command (so it shows up in `getCommands()`) but
its description says plainly that it isn't meant to be typed by hand -- it
depends on module state (`basePrompt`, the pending-revert flag) that only
makes sense mid-sequence.

## The `maxReverts` fallback

A model that never complies would otherwise revert forever. After
`maxReverts` consecutive reverts (default 3), `reporting/` stops reverting
and falls back to level-1-style nagging (which was already running
underneath, since level 2 is level 1 plus reverts) and notifies the user
once. The session is never left silently stuck, and it is never left
spinning.

## Considered and rejected

- **Watch `tool_call` for the built-in `edit`/`write` tools' typed `path`
  input.** Exact for those two tools, but blind to anything written via
  `bash` -- and a report is exactly the kind of file someone might append to
  with a shell redirect. Reverting to a folder probe removes the blind spot
  entirely instead of documenting around it, and is simpler code besides:
  one snapshot-diff function, not two tool-specific narrowings.
- **Trigger the revert from `turn_end`.** Fires once per LLM round trip
  inside the loop, which looks like the natural "a turn just finished"
  point -- but the agent loop is very often still streaming when it fires,
  and `navigateTree` throws in that state. `agent_settled` is the first
  point pi itself guarantees `isStreaming` is false.
- **Block further tool calls instead of reverting** (`ToolCallEventResult{
  block: true }`). Considered as the safer, no-self-dispatch alternative;
  rejected because it doesn't match what was asked for -- it stops forward
  progress but never undoes the ignored turn or re-poses the original
  prompt, which is what "revert the last step and run the last prompt
  again" means.
