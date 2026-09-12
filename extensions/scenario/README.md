# scenario

Multi-step analysis workflows, advanced by the agent itself. A scenario is a
directory of Markdown briefings under `prompts/scenarios/<id>/`; the agent
calls `reactor_step_complete(summary)` when a step's work is done, and the
tool's *return value is the next step's briefing* — what to do now, what not
to start yet, which tools just became relevant. State survives `/reload` and
resumed sessions.

The shipped scenario is `investigation`: seventeen stages from scoping and
collection planning through triage, static, dynamic and deep analysis,
timeline, detection and reporting to lessons learned and analysis-derived
tooling.

```
/reactor-scenario list              what scenarios exist (autocomplete offers ids)
/reactor-scenario start <id>        begin one
/reactor-scenario status            where you are
/reactor-scenario next [summary]    manual advance, same state machine
/reactor-scenario stop              end it
```

A step may name a `toolset:` in its frontmatter; activating it as the agent
reaches the step is additive — the previous step's tools are never taken away.
`REACTOR_SCENARIOS_DIR` overrides where steps are read from.

Why the tool is always registered, why steps are read from files rather than
surfaced as pi prompt commands, and how to write your own scenario:
[ADR-0009](../../docs/adr/0009-scenarios-advance-by-tool-result.md) and
[ADR-0017](../../docs/adr/0017-scenario-steps-are-read-directly-not-pi-prompts.md).
