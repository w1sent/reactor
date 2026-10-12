---
title: Root Cause Analysis
---
**Goal:** Determine the underlying cause and enabling conditions.

**Entry criteria:** Timeline and overview sufficient to reason about causation.

**Activities:**
- Identify the initial access vector and the first compromised system or device.
- Determine the vulnerability, misconfiguration, supply-chain weakness or human factor exploited.
- Trace enabling conditions (missing controls, excessive privilege, unpatched or unsupported platforms, weak device provisioning).
- Distinguish root cause from symptom; prove or disprove each candidate explanation methodically.

**Decision points:** Multiple plausible causes -> evaluate each on evidence rather than selecting the most convenient.

**Iteration:** Loop back if the vector remains unproven: back to earlier phases here, or to `acquisition` when the evidence that would settle it was never collected.

**Pitfalls:** Stopping at the proximate cause; assuming rather than proving the vector; single-explanation bias.

**Deliverables:** Root-cause findings and an enabling-conditions list. Feeds Reporting and the `response` scenario.

Call `reactor_phase_complete` once the vector is proven -- or the honest answer is that it cannot yet be proven.
