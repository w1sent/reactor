---
title: Remediation & Containment Recommendations
description: Turn what an investigation established into containment, eradication and recovery steps, then lessons learned.
---
**Goal:** Provide actionable containment, eradication and recovery guidance (NIST SP 800-61).

**Applies when:** Root cause and scope are reasonably established -- by an `investigation` in this case, or by what the person tells you.

**Orient:** Start from the goal and what the case already holds: its files, the graph, and what earlier scenarios concluded (listed under this briefing when there is any). Say which of the inputs this phase works from are there, state the assumption you make for any that are not, and go on. Ask only when the goal cannot be answered without what is missing.

**Activities:**
- Recommend containment (isolation or quarantine, blocking C2, disabling accounts and tokens, credential and key rotation, revoking certificates or device enrolments).
- Recommend eradication (remove artifacts, rebuild or reflash from trusted known-good images, close the underlying vulnerability, address every persistence mechanism identified).
- Recommend recovery (restore from verified clean backups, staged return to service with heightened monitoring).
- Account for platform constraints -- some embedded and mobile devices cannot be reliably cleaned in place and must be reflashed or replaced.

**Decision points:** Immediate containment vs. monitored observation for intelligence; rebuild vs. clean in place; replace vs. reflash for constrained devices.

**Pitfalls:** Partial eradication leaving backdoors or firmware-level persistence; tipping off the adversary prematurely; restoring from compromised backups.

**Deliverables:** Prioritised remediation and containment plan with decision thresholds. Feeds Lessons learned.

Call `reactor_phase_complete` once the plan is prioritised and its decision thresholds are explicit.
