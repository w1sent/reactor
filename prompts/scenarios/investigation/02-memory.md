---
title: Memory / Runtime State Analysis
toolset: memory
---
**Skippable.** Read `evidence/INVENTORY.md` first. If it lists no memory image and no live runtime access, this phase does not apply: call `reactor_phase_complete` at once with a summary that begins `skipped:` and says what the inventory lacks, and do nothing else.

**Goal:** Extract evidence from volatile memory that never reaches persistent storage -- injected code, memory-only payloads, hidden processes, in-memory configuration, credentials and keys.

**Entry criteria:** A memory image or live runtime access is in the inventory.

**Activities:**
- Establish process and task context; hunt injected, hollowed or unbacked executable regions.
- Enumerate network and handle/descriptor artifacts; inspect loaded modules and hooks; scan memory with detection rules.
- Dump suspicious regions and processes for follow-on reverse engineering.
- Recover in-memory configuration, credentials and encryption keys present while the process is or was running.

**Decision points:** Symbol or profile mismatch must be resolved before trusting output. Suspicious region found -> dump it and note it for the `reverse-engineering` scenario. Key material present -> record it; it is the input a `tool-building` scenario would turn into a decryptor.

**Pitfalls:** Incorrect symbols producing nonsense; mistaking legitimate security-agent injection for malicious activity; assuming volatile state survived a reboot or power loss.

**Quality checks:** Cross-technique validation (multiple enumeration methods compared); dumped artifacts hashed; findings corroborated against persistent and network evidence.

**Deliverables:** Memory findings, dumped processes and regions, in-memory IOCs and recovered secrets. Feeds Overview and Timeline.

Call `reactor_phase_complete` once volatile evidence is extracted and cross-checked -- or with a `skipped:` summary.
