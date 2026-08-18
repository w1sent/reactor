---
title: Static analysis
toolset: native
---
Disassemble or decompile. Map the interesting functions: entry point,
suspicious imports, anything that looks like persistence, C2, or a packer
stub unpacking itself. Unpack first if step 1 found packing. Do not run the
sample yet -- form a theory from statics alone.

Call `reactor_step_complete` once you have a working theory of what this file
does, even a rough one.
