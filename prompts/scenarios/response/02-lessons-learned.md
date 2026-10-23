---
title: Lessons Learned
---
**Goal:** Improve future response (NIST SP 800-61 Post-Incident Activity).

**Applies when:** Incident substantially resolved.

**Orient:** Build on what the earlier phases concluded. If what this phase works from is not there, say so, state the assumption you make and go on with what there is.

**Activities:**
- Hold a review; assess what happened, what worked and what did not.
- Update detections, playbooks and the IR plan; feed new indicators and rules into defences.
- Identify control, visibility and tooling gaps, including platforms where the team lacked acquisition or analysis capability; capture training needs.

**Pitfalls:** Blame culture suppressing candour; lessons captured but never actioned.

**Deliverables:** Lessons-learned report, updated playbooks and detections, capability gap list. Feeds back into `acquisition` for the next incident.

Call `reactor_phase_complete` once the review is written and its actions are listed.
