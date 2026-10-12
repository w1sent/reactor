---
title: Triage
toolset: triage
---
**Goal:** Rapidly separate signal from noise, prioritise systems and artifacts, and decide where deep analysis is warranted.

**Entry criteria:** The `acquisition` scenario has finished: `evidence/INVENTORY.md` and the scope statement exist in the case. Evidence the person brought directly is fine -- record it in the inventory first.

**Activities:**
- Read the inventory and note which kinds of evidence exist; that decides whether the Memory and Network phases apply.
- Run rapid collection and parse the platform's key execution, persistence and account artifacts.
- Hash suspect files and check reputation and known-bad sets.
- Extract static properties of suspect artifacts (format, strings, imports or symbols, packing indicators, entropy, signing state).
- Take a first look at whatever volatile or network evidence the inventory lists.
- Form initial hypotheses; rank systems and devices by likelihood of compromise and evidentiary value.

**Decision points:** Did a specimen turn up that needs analysing in its own right? Record it and hand it to the `malware-analysis` scenario -- it is a separate scenario, run before or after this one. Which systems warrant going back to `acquisition` for full collection? Is immediate escalation or containment needed?

**Iteration:** Re-triage additional systems using indicators discovered in early passes.

**Pitfalls:** Deep-diving before triage; trusting a single automated verdict; lacking a platform baseline so legitimate security agents and normal OS behaviour look malicious.

**Deliverables:** Triage report, prioritised system/artifact list, initial IOCs and hypotheses. Feeds Memory, Network and Overview.

Call `reactor_phase_complete` once systems are ranked and initial hypotheses are on record.
