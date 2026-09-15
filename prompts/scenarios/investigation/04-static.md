---
title: Stage D — Static Analysis
toolset: native
---
**Goal:** Extract everything possible from a specimen without executing it.

**Entry criteria:** A suspect artifact is isolated in the lab.

**Activities:**
- Identify format, architecture, target platform and toolchain; compute hashes and format-appropriate fuzzy or import hashes.
- Extract and decode strings; parse the container structure (headers, sections/segments, imports/exports and symbols, resources, manifests and permissions, signing and certificate data).
- Assess entropy and detect packing or encryption; run existing detection rules; note anti-analysis indicators.
- Hypothesise capability from imported APIs, requested permissions or referenced syscalls.

**Decision points:** Packed or obfuscated -> route to Stage F. Already classifiable to a known family? Script, document or bytecode payload -> deobfuscation path.

**Iteration:** Re-run static analysis on each unpacked or decoded layer.

**Pitfalls:** Trusting strings and imports from a packed artifact; over-reading capability from imports alone; missing payloads embedded in resources, overlays or asset directories.

**Deliverables:** Static analysis notes, extracted strings and structure data, candidate IOCs, packing status. Feeds Stages E and F.

Call `reactor_phase_complete` once static properties are catalogued and capability hypotheses are on record.
