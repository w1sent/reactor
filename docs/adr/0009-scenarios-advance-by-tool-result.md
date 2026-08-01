# Scenarios advance by tool result, not by marker or heuristic

A scenario is a multi-step analysis workflow — triage, then static, then dynamic,
then report. The agent advances by calling a registered pi tool:

```
reactor_step_complete(summary: "triage done: ARM64 ELF, stripped,
                                UPX-packed, 3 network syscalls")
```

and the tool's **return content is the next step's briefing**:

```
## Step 2/4: Static analysis
Unpack first, then map the network paths you found.
Newly relevant: bn, angr.
Do not start dynamic analysis yet.
```

Scenario state — which step, what each completed step reported — rides in the
tool result's `details` field, which pi persists in the session without ever
sending it to the model, and is restored on session reload via `appendEntry` and
`ctx.sessionManager.getEntries()`.

## Why a tool call

Tool calls are the one channel a model uses reliably and structurally. The
alternatives all degrade: a sentinel string in prose is a regex over natural
language and fires on "I will emit `<<STEP-DONE>>` when I am finished"; hooking
`agent_settled` conflates "stopped talking" with "finished the step", and the
agent settles constantly, including when it pauses to ask a question mid-step.

Making the *result* carry the briefing is what makes this cheap. The obvious
design — call a tool, then have the extension inject a message with the next
step — costs an extra message in context and an extra turn boundary. Returning
the briefing as the tool's own content costs nothing beyond the tool result that
was already going to exist, and it arrives exactly where the model is already
looking.

The `summary` parameter is not decoration. It forces the model to state what it
concluded before it is allowed to move on, and that statement is what the next
briefing can refer back to.

## Why scenarios exist

An eager model finishes triage and immediately starts patching. It is not wrong
to be capable of that, but it skips the evidence-gathering that would have made
the patch correct, and it burns context on a subproblem nobody asked about yet.
A briefing that says "do not start dynamic analysis yet" costs one tool result
and redirects it.

## Consequences

- Scenarios steer; they do not restrict. Nothing stops an agent from ignoring a
  briefing and running `frida` in step 1 — consistent with
  [ADR-0007](0007-deactivation-is-soft.md), and accepted.
- The user can always advance manually. A slash command that forces the next
  step exists alongside the tool, because the human is the better judge of
  whether a step is genuinely finished.
- Steps can change what is *advertised*: a briefing names the tools that just
  became relevant, and a scenario may activate a toolset as it advances. It
  never deactivates as enforcement — only to reduce noise.
- Scenario definitions are prompt templates, discovered through
  `resources_discover` like everything else, so they can be gated and reloaded
  the same way.
- Deferred past v1 (see `TODO.md`), because the registry has to be in real use
  before the right step decomposition is knowable.

## Considered and rejected

- **A sentinel string in the reply**, matched in `message_end` — rejected.
  Harness-portable, which is worth nothing here
  ([ADR-0001](0001-pi-is-the-only-target-harness.md)), and a weak contract:
  markers leak into user-visible output and fire when merely discussed.
- **Advancing on `agent_settled`** — rejected; fires on every pause, including
  mid-step clarifying questions, and cannot distinguish them.
- **User-driven only, no tool** — rejected as the sole mechanism, kept as the
  manual override. It has zero false advances and puts judgement with the human,
  but it makes every step boundary a synchronous wait on the user, which defeats
  running a scenario unattended.
- **A step state machine enforced by blocking tool calls** — rejected; see
  [ADR-0007](0007-deactivation-is-soft.md).
