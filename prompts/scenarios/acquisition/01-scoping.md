---
title: Scoping & Collection Planning
toolset: forensics
---
**Goal:** Define the questions to answer, the scope, the legal authority and a prioritised evidence-collection plan before touching any evidence.

**Entry criteria:** An incident or request exists with a point of contact and a rough description.

**Activities:**
- Identify the questions to answer and the success criteria; determine the platforms, architectures and device types in scope.
- Enumerate candidate evidence sources -- endpoints, servers, mobile and embedded devices, volatile memory, network captures and flow logs, cloud and SaaS logs, backups -- without yet deciding which will be collected.
- Classify sources by order of volatility and by accessibility constraints (locked devices, encrypted storage, soldered flash, vendor-controlled cloud data, captures that roll over).
- Determine legal authority, jurisdiction, privacy constraints and data-handling rules; select acquisition methods and validate tools per ISO/IEC 27041.
- Define storage, naming and chain-of-custody procedures; assign roles.
- Write the scope statement and the collection plan into the case as files. Acquisition, the next phase, works from the plan; the `investigation` scenario later answers to the scope statement.

**Decision points:** Live vs. dead acquisition? Full image vs. targeted/logical collection? Which sources are technically reachable at all? In-scope vs. out-of-scope systems?

**Iteration:** Revisit scope as new affected systems surface during analysis; a widened scope means running this scenario again.

**Pitfalls:** Over-collecting; under-scoping and missing patient zero; ignoring volatile data; assuming a desktop-style acquisition path applies to a mobile or embedded target; no legal sign-off.

**Deliverables:** Collection plan, scope statement, volatility-ordered evidence source list, chain-of-custody template. Feeds Acquisition.

Call `reactor_phase_complete` once the collection plan and scope statement exist.
