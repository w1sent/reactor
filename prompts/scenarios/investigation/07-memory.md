---
title: Stage G — Memory / Runtime State Analysis
toolset: memory
---
**Goal:** Extract evidence from volatile memory that never reaches persistent storage -- injected code, memory-only payloads, hidden processes, in-memory configuration, credentials and keys.

**Entry criteria:** A memory image or live runtime access exists.

**Activities:**
- Establish process and task context; hunt injected, hollowed or unbacked executable regions.
- Enumerate network and handle/descriptor artifacts; inspect loaded modules and hooks; scan memory with detection rules.
- Dump suspicious regions and processes for follow-on reverse engineering.
- Recover in-memory configuration, credentials and encryption keys present while the process is or was running.

**Decision points:** Symbol or profile mismatch must be resolved before trusting output. Suspicious region found -> dump and route to Stage F. Key material present -> route to Stage Q.

**Pitfalls:** Incorrect symbols producing nonsense; mistaking legitimate security-agent injection for malicious activity; assuming volatile state survived a reboot or power loss.

**Quality checks:** Cross-technique validation (multiple enumeration methods compared); dumped artifacts hashed; findings corroborated against persistent and network evidence.

**Deliverables:** Memory findings, dumped processes and regions, in-memory IOCs and recovered secrets. Feeds Stages F, I, J, Q.

Call `reactor_phase_complete` once volatile evidence is extracted and cross-checked.
