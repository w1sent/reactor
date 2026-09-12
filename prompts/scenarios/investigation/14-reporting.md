---
title: Stage N — Reporting / Publication
---
**Goal:** Produce clear, defensible deliverables for technical and executive audiences (NIST SP 800-86 "Reporting").

**Entry criteria:** Analysis sufficient to answer the scoping questions.

**Activities:**
- Structure the report (executive summary -> scope and method -> findings -> timeline -> ATT&CK mapping -> IOC and detection appendices -> recommendations).
- Write a plain-language executive summary covering impact and required action; present technical detail with enough method to be reproduced.
- Apply ICD 203 estimative language, keeping likelihood and analyst confidence separate; give alternative explanations due consideration; include visualisations.
- Compile appendices in machine-readable form.

**Decision points:** Audience-specific versions; classification and handling markings; body vs. appendix placement.

**Iteration:** Peer/QA review cycle.

**Pitfalls:** Overstating certainty; platform jargon in the executive summary; unsourced claims; conflating likelihood with confidence.

**Deliverables:** Final report, executive brief, IOC and detection appendix. Feeds Stage O and the customer.

Call `reactor_step_complete` once the report answers the scoping questions and every claim carries its evidence.
