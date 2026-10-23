---
title: Scoping & Collection Planning
toolset: forensics
description: Plan and collect evidence defensibly: decide what to collect, from where and under what authority, then acquire and preserve it.
---
**Goal:** Define the questions to answer, the scope, the legal authority and a prioritised evidence-collection plan before touching any evidence.

**Applies when:** An incident or request needs evidence collected, and the sources, scope or authority are not settled yet. When evidence is already in the case, record what is there and plan only what is missing.

**Orient:** Start from the goal and what the case already holds: its files, the graph, and what earlier scenarios concluded (listed under this briefing when there is any). Say which of the inputs this phase works from are there, state the assumption you make for any that are not, and go on. Ask only when the goal cannot be answered without what is missing.

**Activities:**
- Identify the questions to answer and the success criteria; determine the platforms, architectures and device types in scope.
- Enumerate candidate evidence sources -- endpoints, servers, mobile and embedded devices, volatile memory, network captures and flow logs, cloud and SaaS logs, backups -- without yet deciding which will be collected.
- Classify sources by order of volatility and by accessibility constraints (locked devices, encrypted storage, soldered flash, vendor-controlled cloud data, captures that roll over).
- Determine legal authority, jurisdiction, privacy constraints and data-handling rules; select acquisition methods and validate tools per ISO/IEC 27041.
- Define storage, naming and chain-of-custody procedures; assign roles.
- Record the scope and the collection plan in the case, where the next phase and later scenarios will find them. Acquisition, the next phase, works from the plan.

**Decision points:** Live vs. dead acquisition? Full image vs. targeted/logical collection? Which sources are technically reachable at all? In-scope vs. out-of-scope systems?

**Iteration:** Revisit scope as new affected systems surface during analysis; a widened scope means running this scenario again.

**Pitfalls:** Over-collecting; under-scoping and missing patient zero; ignoring volatile data; assuming a desktop-style acquisition path applies to a mobile or embedded target; no legal sign-off.

**Deliverables:** Collection plan, scope statement, volatility-ordered evidence source list, chain-of-custody template. Feeds Acquisition.

Call `reactor_phase_complete` once the scope and the collection plan are recorded.
