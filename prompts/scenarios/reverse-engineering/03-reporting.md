---
title: Reporting
---
**Goal:** Record what was recovered so that someone else can verify it and build on it.

**Entry criteria:** The questions that started the work are answered, or the point where they could not be is understood.

**Activities:**
- State each question and its answer, with the evidence (addresses, functions, captured data) and how a reader reproduces it.
- Write the recovered algorithms, protocol structures and configuration formats as specifications: language-neutral pseudocode, field tables, worked examples with real data.
- Record the unpacked layers and how they were obtained, with hashes, so the work can be repeated.
- Separate what was proven from what was inferred, and list what remains unknown.
- Say what the specifications are ready for: a `tool-building` scenario (an extractor, decoder or decryptor), a `patching` scenario, or detection work in `malware-analysis`.

**Pitfalls:** Findings without the addresses to check them; specifications that leave out the edge cases that were hard; inferred behaviour presented as proven.

**Deliverables:** Findings report, specifications, unpacked artifacts with hashes.

Call `reactor_phase_complete` once every answer carries its evidence and the specifications are complete enough to implement from.
