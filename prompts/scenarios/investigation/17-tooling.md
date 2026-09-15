---
title: Stage Q — Creating Analysis-Derived Tooling
---
**Goal:** Convert understanding gained in earlier stages into validated, reusable software: configuration and secret extractors, deobfuscators and unpackers, protocol and traffic decoders, artifact and format parsers, emulation or instrumentation harnesses, automated triage and classification tools, and data-recovery or decryption utilities.

**Entry criteria:** Reverse engineering (Stage F), memory analysis (Stage G) or artifact analysis has produced a sufficiently complete specification of the logic to be reimplemented, plus samples to validate against.

**Inputs:** Recovered algorithms and pseudocode, format and protocol specifications, extracted key or configuration material, a corpus of samples or artifacts, and ground truth (known plaintext, known-good parsed output, expected configurations, reference decoded data).

**Activities:**
1. **Specify.** The analyst role writes a precise, platform-neutral specification of what the tool must do: the input format and its variants, the transformation or extraction logic, edge cases, and the expected output. Define the success criterion and how it will be measured.
2. **Assess feasibility and value.** Confirm the logic is complete enough to reimplement, that the effort is justified by reuse or scale, and that no existing tool already does it. For recovery and decryption tools specifically, confirm the recovery is actually possible rather than assumed.
3. **Implement.** Build the tool defensively: hostile, malformed and truncated input must not crash or execute anything; never contact live malicious infrastructure outside Infrastructure-provided isolation; keep the implementation portable and isolate target-specific parts behind clear interfaces.
4. **Validate against ground truth.** Run against every available sample and compare to independently derived expected output. For extractors, compare with manually extracted configuration. For deobfuscators and unpackers, verify the output executes or analyses consistently with dynamic observations. For parsers, verify against a reference implementation or manual parsing. For decryption and recovery tools, verify against known plaintext/ciphertext pairs and format signatures. Record the success rate and the exact variant scope covered.
5. **Harden and scope.** Handle variants, versions and platform differences explicitly; fail loudly and safely when input falls outside the validated scope rather than producing silently wrong output.
6. **Package and document.** Ship with usage documentation, the validated variant and version scope, known limitations, the measured success rate, test suite, and reproducible build instructions.

**Decision points:** One-off script or maintained tool? Standalone utility or plugin to an existing framework? Is coverage sufficient, or are more samples needed to generalise? Does the tool need to be safe for non-expert operators?

**Iteration:** Refine against new variants; re-measure the success rate; loop back to Stage F when the tool fails on inputs the specification did not anticipate -- tool failure is itself an analytical finding.

**Pitfalls:** Building from an incomplete or misunderstood specification; validating only on the sample that inspired the tool; silently producing wrong output on out-of-scope input; hardcoding one platform's assumptions; releasing attacker-usable capability without considering the consequences.

**Quality checks:** Validated against multiple independent ground-truth cases; deterministic and reproducible; non-destructive by default (never modify original evidence -- operate on copies and write to new output); scope and limitations stated honestly.

**Deliverables:** Tested tool, validation results, variant and limitation notes, usage documentation, safe-operation guidance.

Call `reactor_phase_complete` once the tool passes every ground-truth case and its documented scope matches its measured behaviour.
