---
title: Creating an Overview
---
**Goal:** Synthesise findings so far into a coherent situational picture.

**Entry criteria:** Triage, plus Memory and Network where they applied.

**Activities:**
- Consolidate what is known (access vector, capability, scope, impact, affected platforms); identify gaps and contradictions.
- Frame working hypotheses with explicit confidence; produce a high-level attack narrative and a system/artifact map.
- Recommend the next analytical steps, including whether more evidence is needed (`acquisition` again) or a specimen needs its own analysis (`malware-analysis`).

**Decision points:** Sufficient to proceed to the timeline and root cause, or are gaps large enough to loop back?

**Iteration:** Maintain as a living document updated throughout the investigation.

**Pitfalls:** Presenting hypotheses as facts; anchoring on the first theory; omitting alternative explanations; silently dropping platforms that were in scope.

**Deliverables:** Overview/situation report, hypothesis list with confidence, gap analysis. Feeds Timeline, Root cause and Reporting.

Call `reactor_phase_complete` once the overview states what is known, what is not, and what to do next.
