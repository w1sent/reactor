---
title: Triage
toolset: triage
description: Investigate an incident on systems or devices: triage, memory, network, overview, timeline, root cause and report.
---
**Goal:** Rapidly separate signal from noise, prioritise systems and artifacts, and decide where deep analysis is warranted.

**Applies when:** A system, device or incident is to be investigated, and evidence about it is in the case -- however it got there.

**Orient:** Start from the goal and what the case already holds: its files, the graph, and what earlier scenarios concluded (listed under this briefing when there is any). Say which of the inputs this phase works from are there, state the assumption you make for any that are not, and go on. Ask only when the goal cannot be answered without what is missing.

**Activities:**
- Survey the evidence in the case (an inventory if there is one, else the files themselves) and note which kinds exist; that decides whether the Memory and Network phases apply.
- Run rapid collection and parse the platform's key execution, persistence and account artifacts.
- Hash suspect files and check reputation and known-bad sets.
- Extract static properties of suspect artifacts (format, strings, imports or symbols, packing indicators, entropy, signing state).
- Take a first look at whatever volatile or network evidence there is.
- Form initial hypotheses; rank systems and devices by likelihood of compromise and evidentiary value.

**Decision points:** Did a specimen turn up that needs analysing in its own right? Record it and hand it to the `malware-analysis` scenario -- it is a separate scenario, run before or after this one. Which systems warrant going back to `acquisition` for full collection? Is immediate escalation or containment needed?

**Iteration:** Re-triage additional systems using indicators discovered in early passes.

**Pitfalls:** Deep-diving before triage; trusting a single automated verdict; lacking a platform baseline so legitimate security agents and normal OS behaviour look malicious.

**Deliverables:** Triage report, prioritised system/artifact list, initial IOCs and hypotheses. Feeds Memory, Network and Overview.

Call `reactor_phase_complete` once systems are ranked and initial hypotheses are on record.
