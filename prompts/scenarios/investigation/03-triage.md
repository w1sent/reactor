---
title: Stage C — Triage
toolset: triage
---
**Goal:** Rapidly separate signal from noise, prioritise systems and artifacts, and decide where deep analysis is warranted.

**Entry criteria:** Acquired or live-accessible evidence exists.

**Activities:**
- Run rapid collection and parse the platform's key execution, persistence and account artifacts.
- Hash suspect files and check reputation and known-bad sets.
- Extract static properties of suspect artifacts (format, strings, imports or symbols, packing indicators, entropy, signing state).
- Take a first pass over volatile memory (process inventory, injected regions, network connections).
- Form initial hypotheses; rank systems and devices by likelihood of compromise and evidentiary value.

**Decision points:** Malware incident (Cyber-Forensics / Reverse Engineer territory) or general system investigation (Forensics territory)? Which systems get full acquisition? Is immediate escalation or containment needed?

**Iteration:** Re-triage additional systems using indicators discovered in early passes.

**Pitfalls:** Deep-diving before triage; trusting a single automated verdict; lacking a platform baseline so legitimate security agents and normal OS behaviour look malicious.

**Deliverables:** Triage report, prioritised system/artifact list, initial IOCs and hypotheses. Feeds Stages D--I.

Call `reactor_phase_complete` once systems are ranked and initial hypotheses are on record.
