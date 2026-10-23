---
title: Targeted Reverse Engineering
toolset: debugging
---
**Goal:** Locate and understand the code that produces the behaviour to be changed -- no more of the program than that.

**Applies when:** A specification naming the behaviour to change.

**Orient:** Build on what the earlier phases concluded. If what this phase works from is not there, say so, state the assumption you make and go on with what there is.

**Activities:**
- Find the change site: trace from the observable behaviour (a string, a message, a check, an output) to the code that decides it, using static cross-references and a debugger or emulator on the running program.
- Understand it well enough to change it: inputs and outputs, callers, data and control flow, side effects, shared state, and what else depends on it.
- Find the guards: integrity checks, checksums, signature validation, anti-tamper and self-update code that would notice a modification.
- Note the space available for a change (padding, unused code, alignment) and the calling convention and constraints the replacement must meet.
- Confirm the understanding against the baseline: predict what the program does for a given input, then check.

**Decision points:** Packed or protected -> unpack first, through `reverse-engineering`. Behaviour spread over many sites, or the code is not understood well enough to change safely? Return to Scoping & specification with that finding rather than guess.

**Iteration:** Interleave static and dynamic work; every claim about the code is tested on the running program.

**Pitfalls:** Reversing the whole program instead of the change site; patching the first matching string rather than the decision that matters; missing a second copy of the logic; ignoring the integrity checks until the patch is already built.

**Deliverables:** Change-site notes (addresses, functions, calling conventions, constraints), a list of guards, and the evidence that the logic is understood. Feeds Patch & test.

Call `reactor_phase_complete` once the change site and its guards are understood well enough to design the patch.
