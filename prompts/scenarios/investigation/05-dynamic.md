---
title: Stage E — Dynamic / Behavioural Analysis
toolset: dynamic
---
**Goal:** Observe runtime behaviour safely to reveal what static analysis cannot.

**Entry criteria:** Isolation confirmed by Infrastructure and a clean-state snapshot or baseline taken.

**Activities:**
- Baseline the clean environment; execute the specimen in a platform-appropriate instrumented environment (virtual machine, container, emulator, instrumented device or hardware bench).
- Monitor process, file, configuration, permission and network activity with platform-native or injected instrumentation.
- Redirect and capture network callbacks with simulated services; capture dropped artifacts and memory-resident state.
- Iterate by supplying expected inputs, arguments, environment conditions or simulated server responses to unlock further behaviour; characterise C2 patterns and timing.

**Decision points:** Does the specimen detect the analysis environment, require specific hardware, or refuse to run? (Anti-analysis handling is Stage F.) Is code-level insight needed to progress?

**Iteration:** Revert to clean state and re-run under modified conditions until behaviour is characterised.

**Pitfalls:** Allowing real network egress; concluding "benign" from a single evasive run; contaminating the host or the wider device lab; ignoring behaviour that only triggers on real hardware or with real peripherals.

**Deliverables:** Behavioural report, captured traffic, dropped artifacts, runtime IOCs, C2 details. Feeds Stages F, I, J.

Call `reactor_phase_complete` once behaviour is characterised -- or the refusal to run is itself documented.
