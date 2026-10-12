---
title: Evidence Acquisition & Preservation
toolset: forensics
---
**Goal:** Collect, in one pass and defensibly, every planned source that is actually available -- volatile memory, network captures, persistent storage, logs and cloud exports -- while preserving integrity (NIST SP 800-86 "Collection"; ISO/IEC 27037).

**Entry criteria:** An approved collection plan and scope statement.

**Activities:**
- Follow order of volatility across *all* source kinds: running state and memory first, then network captures and flow logs that age out or roll over, then persistent storage, then logs, cloud exports and archived or derived data.
- Collect what is available, whatever its kind. A source the plan named but that cannot be reached is recorded as unreachable with the reason; a source nobody planned but that turns out to exist is collected if it is in scope.
- Use read-only or write-blocked acquisition where the platform permits, and document the method and its side effects where it does not.
- Select the acquisition depth the platform supports (physical image, full-filesystem or logical extraction, chip-off or flash dump, agent-based collection, memory capture, packet capture, cloud export).
- Compute cryptographic hashes at acquisition and record them; capture volatile context (processes, connections, logged-on users, loaded modules).
- Document tool, version, operator, timestamps (UTC), source identity and method contemporaneously.
- Write `evidence/INVENTORY.md` in the case: one entry per source with its kind (memory, disk, network, logs, device, specimen), path relative to the case, SHA-256, tool and version, UTC time, status (acquired, partial, unreachable, not applicable) and the reason for anything not acquired. Evidence files stay where they were put and are referenced by path and hash.

**Decision points:** If acquisition may be interrupted (battery, remote wipe, tamper response), prioritise the highest-value data first. If the device cannot be imaged without alteration, record the justification. Decide, from the inventory, which scenario comes next: `investigation` for a system, device or incident; `malware-analysis` when a specimen was isolated; `reverse-engineering` when only a program's internals are in question.

**Pitfalls:** Powering off a live compromised system and losing memory-resident keys and fileless code; letting a capture window roll over while disk was being imaged; triggering device wipe or re-encryption; missing hashes; broken custody; an inventory that lists only what succeeded.

**Quality checks:** Post-acquisition hash verification; auditable and repeatable process; complete custody entries; the inventory accounts for every source in the plan.

**Deliverables:** Verified images, memory dumps, captures and extractions with hashes and custody records, and `evidence/INVENTORY.md`. This scenario ends here: scenario state is cleared when it finishes, so the inventory is what the next scenario reads.

Call `reactor_phase_complete` once acquisition is verified -- hashes match, custody entries complete and the inventory written -- and say which scenario to start next.
