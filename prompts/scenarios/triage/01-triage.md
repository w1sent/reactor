---
title: Triage
toolset: triage
---
Identify the file: format, architecture, endianness, obvious packing or
obfuscation, anything that jumps out from strings alone. Do not disassemble
yet -- this step is about knowing what you are looking at, not what it does.

Call `reactor_step_complete` once you can state the format, architecture, and
whether the file looks packed or otherwise disguised.
