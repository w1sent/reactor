---
title: Creating a Timeline
toolset: forensics
---
**Goal:** Build an accurate, source-attributed chronology of events.

**Applies when:** There are timestamped artifacts to put in order -- nearly always.

**Orient:** Build on what the earlier phases concluded. If what this phase works from is not there, say so, state the assumption you make and go on with what there is.

**Activities:**
- Normalise all timestamps to a single reference (UTC), recording each source's original time zone, epoch format, resolution and any clock skew.
- Generate a combined timeline from every available source -- filesystem metadata, OS state stores, system and application logs, execution evidence, network telemetry, memory findings, cloud and device logs; filter to relevant windows.
- Correlate events across sources; map each significant event to its ATT&CK technique to produce a TTP-ordered progression.
- Annotate every entry with its source artifact and a confidence level.

**Decision points:** Fast triage timeline or full combined timeline? Which time windows warrant depth?

**Pitfalls:** Mixing time zones or epoch formats across platforms; trusting easily forged timestamps without corroboration; timeline noise burying signal; failing to check for timestamp manipulation.

**Quality checks:** Each entry cites its source; anti-forensic timestamp manipulation explicitly checked; key events corroborated by two or more independent sources.

**Deliverables:** Documented timeline and ATT&CK-mapped event sequence. Feeds Root cause and Reporting.

Call `reactor_phase_complete` once the timeline is source-attributed and ATT&CK-mapped.
