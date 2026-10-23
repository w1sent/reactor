---
title: Reporting / Publication
---
**Goal:** Produce clear, defensible deliverables for technical and executive audiences (NIST SP 800-86 "Reporting").

**Applies when:** The analysis can answer the goal's questions, or it is clear which it cannot.

**Orient:** Build on what the earlier phases concluded. If what this phase works from is not there, say so, state the assumption you make and go on with what there is.

**Activities:**
- Structure the report (executive summary -> scope and method -> findings -> timeline -> ATT&CK mapping -> IOC appendix -> recommendations).
- Write a plain-language executive summary covering impact and required action; present technical detail with enough method to be reproduced.
- Apply ICD 203 estimative language, keeping likelihood and analyst confidence separate; give alternative explanations due consideration; include visualisations.
- Compile the IOC appendix in machine-readable form, from the indicators gathered across the phases. Detection rules for a specimen belong to `malware-analysis`; reference them where they exist.

**Decision points:** Audience-specific versions; classification and handling markings; body vs. appendix placement.

**Iteration:** Peer/QA review cycle.

**Pitfalls:** Overstating certainty; platform jargon in the executive summary; unsourced claims; conflating likelihood with confidence.

**Deliverables:** Final report, executive brief, IOC appendix. Feeds the `response` scenario and the customer.

Call `reactor_phase_complete` once the report answers the goal's questions and every claim carries its evidence.
