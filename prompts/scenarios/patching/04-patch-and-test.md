---
title: Patch & Test
toolset: patching
---
**Goal:** Make the smallest change that meets the specification, and show that it does, and that nothing else changed.

**Applies when:** Change-site notes and guards from Targeted reverse engineering.

**Orient:** Build on what the earlier phases concluded. If what this phase works from is not there, say so, state the assumption you make and go on with what there is.

**Activities:**
- Choose the approach from what was found: in-place byte change, a code cave or added section, a loader or runtime hook, or a rebuild from source. Prefer the one with the least footprint and the easiest rollback; record why.
- Write the new code and assemble it for the exact target architecture and mode; disassemble the result and check it decodes as intended.
- Apply the change to a **copy**, handling the container: relocations, alignment, size and checksum fields, import tables, and anything that must be re-validated.
- Test the specified behaviour; test the baseline -- the same inputs and conditions as Orientation -- and compare. Where the changed routine can be isolated, exercise it alone in an emulator, including edge cases.
- Diff the original against the patched file or its disassembly and confirm the difference is only what was intended.
- Deal with the guards found earlier: satisfy them legitimately, or record that a check was deliberately left to fail.

**Decision points:** Does the approach hold up, or does testing show a different one is needed? Is a failure a bug in the patch, or a sign that the specification or the understanding of the code was wrong?

**Iteration:** Patch, test and revert to the clean copy until the specification is met. Loop back to Targeted reverse engineering when behaviour surprises you.

**Pitfalls:** Testing only the changed path; comparing against remembered rather than recorded behaviour; patching the working copy so the original is lost; endianness, instruction-set mode or alignment mistakes that only show on some inputs.

**Quality checks:** The specified behaviour is met; the baseline is unchanged elsewhere; the diff against the original is explained line by line; the patch applies cleanly to the original and fails safely on a different build.

**Deliverables:** Patched build or patch, test results against the baseline, the diff and its explanation. Feeds Package.

Call `reactor_phase_complete` once the specified behaviour is met, the baseline is unchanged elsewhere and the diff is explained.
