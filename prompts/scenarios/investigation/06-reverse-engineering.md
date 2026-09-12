---
title: Stage F — Deep Reverse Engineering / Unpacking
toolset: debugging
---
**Goal:** Recover the true code, algorithms, protocols and cryptography through unpacking, deobfuscation, disassembly, decompilation and emulation.

**Entry criteria:** Static or dynamic analysis indicates packing, obfuscation or unanswered questions requiring code-level insight.

**Activities:**
- Identify the packer, protector or obfuscator; unpack -- automatically where possible, otherwise by running to the original entry point under a debugger or emulator and dumping, then repairing the dumped image (import tables, relocations, section alignment).
- Deobfuscate strings and control flow, including bytecode and script layers; disassemble, decompile and where useful emulate core routines.
- Recover network protocol structure and cryptographic algorithms (cipher, mode, key and IV derivation and handling); extract configuration.
- Document recovered algorithms as language-neutral pseudocode the Software Engineer can implement.

**Decision points:** Is manual unpacking required? Is the recovered logic complete enough to support a tool (Stage Q)? Are additional samples needed for a robust family rule?

**Iteration:** Interleave static and dynamic techniques; dump at successive unpacking layers.

**Pitfalls:** Analysing the loader rather than the payload; incorrect image reconstruction; anti-debug, anti-emulation and integrity checks defeating naive runs; mistaking custom or modified crypto for a standard primitive.

**Quality checks:** Unpacked code validated (symbols resolve, strings decode, behaviour matches Stage E); recovered algorithms reproduce observed runtime data.

**Deliverables:** Unpacked artifact, recovered algorithm and protocol specifications, extracted configuration, code-level capability notes, detection-rule seed material. Feeds Stages J, K, Q.

Call `reactor_step_complete` once the recovered logic answers the questions that triggered this stage.
