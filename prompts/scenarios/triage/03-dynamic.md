---
title: Dynamic analysis
toolset: dynamic
---
Run it, under a debugger or instrumentation, somewhere it cannot do real
damage. Confirm or correct the static theory against what actually happens:
network calls, files touched, processes spawned.

Call `reactor_step_complete` once you have observed real behaviour that
confirms, corrects, or extends the static theory.
