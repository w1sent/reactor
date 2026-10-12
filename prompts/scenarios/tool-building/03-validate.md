---
title: Validate Against Ground Truth
---
**Goal:** Show, with evidence, that the tool produces the right output and know exactly where it does not.

**Entry criteria:** A working implementation and the corpus with ground truth.

**Activities:**
- Run against every available sample and compare to independently derived expected output.
- For extractors, compare with manually extracted configuration. For deobfuscators and unpackers, verify the output executes or analyses consistently with dynamic observations. For parsers, verify against a reference implementation or manual parsing. For decryption and recovery tools, verify against known plaintext/ciphertext pairs and format signatures.
- Record the success rate and the exact variant scope covered, and examine every failure.
- Feed hostile, malformed, truncated and out-of-scope input and confirm the tool fails safely.

**Decision points:** Is the failure a bug in the tool, a gap in the specification, or something earlier analysis got wrong? A tool that fails on inputs the specification did not anticipate is an analytical finding: take it back to `reverse-engineering`.

**Pitfalls:** Validating only on the sample that inspired the tool; checking the tool against its own output; counting "ran without error" as success.

**Quality checks:** Validated against multiple independent ground-truth cases; deterministic and reproducible.

**Deliverables:** Validation results with success rate and variant scope, and the list of failures with their causes. Feeds Harden & package.

Call `reactor_phase_complete` once the tool passes every ground-truth case it claims to cover and the failures are explained.
