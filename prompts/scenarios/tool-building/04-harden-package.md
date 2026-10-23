---
title: Harden & Package
---
**Goal:** Make the tool safe to hand to someone else, with its scope and limits stated honestly.

**Applies when:** Validation results and an explained list of failures.

**Orient:** Build on what the earlier phases concluded. If what this phase works from is not there, say so, state the assumption you make and go on with what there is.

**Activities:**
- Handle variants, versions and platform differences explicitly; fail loudly and safely when input falls outside the validated scope rather than producing silently wrong output.
- Ship with usage documentation, the validated variant and version scope, known limitations, the measured success rate, the test suite, and reproducible build instructions.
- Write safe-operation guidance: where to run it, what it must not be pointed at, what it does to its input.
- Consider the consequences before releasing anything an attacker could use as is; decide who it is for.

**Decision points:** Maintained tool or one-off script? Is more coverage needed before release, or are more samples needed to generalise? Is it safe for non-expert operators?

**Iteration:** Refine against new variants and re-measure the success rate. Tool failure on unanticipated input goes back to `reverse-engineering`.

**Pitfalls:** Silently producing wrong output on out-of-scope input; a documented scope wider than the measured one; releasing an attacker-usable capability without considering the consequences.

**Quality checks:** Deterministic and reproducible; non-destructive by default; scope and limitations stated honestly.

**Deliverables:** Tested tool, validation results, variant and limitation notes, usage documentation, safe-operation guidance.

Call `reactor_phase_complete` once the tool passes every ground-truth case and its documented scope matches its measured behaviour.
