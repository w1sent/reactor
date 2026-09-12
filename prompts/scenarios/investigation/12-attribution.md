---
title: Stage L — Attribution / TTP Mapping
---
**Goal:** Characterise adversary behaviour in a common framework and, where evidence supports it, associate it with known activity -- defensively.

**Entry criteria:** A body of observed behaviours and TTPs exists.

**Activities:**
- Map all observed behaviours to the appropriate ATT&CK matrix for each affected platform class; build a combined technique layer.
- Compare TTPs, tooling, infrastructure and code characteristics against documented campaigns and groups; assess the strength of each overlap.
- Express any association with explicit confidence and caveats.

**Decision points:** Enough for a named-actor assessment, or only a behavioural profile? What alternative explanations exist?

**Pitfalls:** Over-attribution from weak overlaps; false-flag susceptibility; conflating shared tooling or commodity builders with authorship.

**Deliverables:** ATT&CK technique layer, TTP profile, attribution assessment with confidence. Feeds Stages M, N.

Call `reactor_step_complete` once the technique layer and the confidence-caveated assessment exist.
