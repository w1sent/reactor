---
title: Specify & Assess
---
**Goal:** Turn what earlier analysis understood into a precise specification for a tool, and decide whether building it is feasible and worth it.

**Entry criteria:** Reverse engineering, memory analysis or artifact analysis has produced a sufficiently complete description of the logic to be reimplemented, plus samples to validate against.

**Inputs:** Recovered algorithms and pseudocode, format and protocol specifications, extracted key or configuration material, a corpus of samples or artifacts, and ground truth (known plaintext, known-good parsed output, expected configurations, reference decoded data).

**Activities:**
1. **Specify.** Write a precise, platform-neutral specification of what the tool must do: the input format and its variants, the transformation or extraction logic, edge cases, and the expected output. Define the success criterion and how it will be measured.
2. **Assess feasibility and value.** Confirm the logic is complete enough to reimplement, that the effort is justified by reuse or scale, and that no existing tool already does it. For recovery and decryption tools specifically, confirm the recovery is actually possible rather than assumed.

**Decision points:** One-off script or maintained tool? Standalone utility or plugin to an existing framework? Is the specification complete, or does it send the work back to `reverse-engineering`? Does the tool need to be safe for non-expert operators?

**Pitfalls:** Building from an incomplete or misunderstood specification; no ground truth to validate against; effort spent on a one-off that a script would have served.

**Deliverables:** Specification, success criterion, the validation corpus and ground truth, and the feasibility decision. Feeds Implement.

Call `reactor_phase_complete` once the specification is written, ground truth is identified and the decision to build is recorded.
