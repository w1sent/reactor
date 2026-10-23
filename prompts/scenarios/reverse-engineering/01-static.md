---
title: Static Analysis
toolset: native
description: Answer specific questions about a program's internals: static mapping, deep reverse engineering, and a report.
---
**Goal:** Map the program's structure without executing it, so that reverse engineering goes where the questions are.

**Applies when:** A program, library or specimen whose internals are in question, and the specific questions: what algorithm, what protocol, what configuration, why it behaves as it does.

**Orient:** Start from the goal and what the case already holds: its files, the graph, and what earlier scenarios concluded (listed under this briefing when there is any). Say which of the inputs this phase works from are there, state the assumption you make for any that are not, and go on. Ask only when the goal cannot be answered without what is missing.

**Activities:**
- Identify format, architecture, target platform and toolchain; compute hashes.
- Extract and decode strings; parse the container structure (headers, sections/segments, imports/exports and symbols, resources, manifests, signing data).
- Assess entropy and detect packing, protection or obfuscation; note anti-analysis and integrity checks.
- Locate the code that bears on the questions: entry points, exported functions, string and import cross-references.

**Decision points:** Packed, protected or obfuscated -> go on to Deep reverse engineering with unpacking as its first job. Script, document or bytecode layer -> deobfuscation path.

**Iteration:** Re-run static analysis on each unpacked or decoded layer.

**Pitfalls:** Trusting strings and imports from a packed artifact; analysing the loader rather than the payload; missing payloads embedded in resources, overlays or asset directories.

**Deliverables:** Structure map, candidate functions of interest, packing and protection status. Feeds Deep reverse engineering.

Call `reactor_phase_complete` once the structure is mapped and the code of interest is located.
