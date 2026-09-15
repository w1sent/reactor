---
title: Stage B — Evidence Acquisition & Preservation
toolset: forensics
---
**Goal:** Acquire relevant data defensibly while preserving integrity (NIST SP 800-86 "Collection"; ISO/IEC 27037).

**Entry criteria:** Approved collection plan.

**Activities:**
- Follow order of volatility -- capture volatile memory and running state before persistent storage, and persistent storage before archived or derived data.
- Use read-only or write-blocked acquisition where the platform permits, and document the method and its side effects where it does not.
- Select the acquisition depth the platform supports (physical image, full-filesystem or logical extraction, chip-off or flash dump, agent-based collection, cloud export).
- Compute cryptographic hashes at acquisition and record them; capture volatile context (processes, connections, logged-on users, loaded modules).
- Document tool, version, operator, timestamps, source identity and method contemporaneously.

**Decision points:** If acquisition may be interrupted (battery, remote wipe, tamper response), prioritise the highest-value data first. If the device cannot be imaged without alteration, record the justification.

**Pitfalls:** Powering off a live compromised system and losing memory-resident keys and fileless code; triggering device wipe or re-encryption; missing hashes; broken custody.

**Quality checks:** Post-acquisition hash verification; auditable and repeatable process; complete custody entries.

**Deliverables:** Verified images, extractions, memory dumps and captures with hashes and custody records. Feeds Stages C--H.

Call `reactor_phase_complete` once acquisition is verified -- hashes match, custody entries complete.
