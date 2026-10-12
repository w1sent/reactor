---
title: Scoping & Specification
---
**Goal:** Say exactly what the change is, what must not change, and whether it is feasible, before reverse engineering for it.

**Entry criteria:** An oriented program with a recorded baseline.

**Activities:**
- Confirm the purpose and the authority to modify this program -- ownership, licence terms or authorisation -- and write it down. Stop here if it cannot be established.
- Specify the change as behaviour: what the program does now, what it should do, the inputs under which each applies.
- State what must stay the same, drawn from the baseline, and the success criteria and how each is measured.
- Fix constraints: minimal difference or free rewrite, which builds and versions are in scope, whether signatures or integrity checks have to keep validating, who will apply the patch and how.
- Make a coarse feasibility call from the profile: is the change likely to be a few instructions, a new routine or a redesign? What is known and what is a guess?

**Decision points:** Feasible, infeasible, or feasible only with more orientation? Binary patch, loader or runtime hook, or rebuild from source? (The exact approach is settled in Patch & test, once the change site is known.)

**Iteration:** If reverse engineering shows the specification was wrong or unreachable, return here and change it deliberately; do not drift.

**Pitfalls:** Specifying implementation instead of behaviour; no stated invariants, so regressions cannot be defined; scope that grows during the work.

**Deliverables:** Written specification with success criteria, invariants and constraints, plus the feasibility assessment. Feeds Targeted reverse engineering.

Call `reactor_phase_complete` once the specification and feasibility call are written and the authority to proceed is recorded.
