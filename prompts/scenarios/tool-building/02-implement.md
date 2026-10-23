---
title: Implement
---
**Goal:** Build the tool defensively, from the specification.

**Applies when:** A specification, a success criterion and a validation corpus with ground truth.

**Orient:** Build on what the earlier phases concluded. If what this phase works from is not there, say so, state the assumption you make and go on with what there is.

**Activities:**
- Implement against the specification, not against the one sample that prompted it.
- Treat all input as hostile: malformed and truncated input must not crash the tool or execute anything.
- Never contact live malicious infrastructure outside an isolation environment provided for the purpose.
- Keep the implementation portable and isolate target-specific parts behind clear interfaces.
- Make the tool non-destructive by default: never modify original evidence -- read it, operate on copies and write to new output.

**Decision points:** Where the specification is silent or ambiguous, ask or return to Specify & assess rather than invent behaviour. Which parts need to be configurable, and which are better fixed?

**Iteration:** Build in slices that can each be run against ground truth as they appear.

**Pitfalls:** Hardcoding one platform's assumptions; parsing that trusts length and offset fields from the input; behaviour that depends on the analyst's machine; copying code from the specimen that should be reimplemented.

**Deliverables:** Working implementation with its own tests. Feeds Validate.

Call `reactor_phase_complete` once the implementation covers the specification and runs on the corpus.
