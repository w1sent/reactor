---
title: Package
---
**Goal:** Deliver the change so that someone else can apply it, verify it and undo it.

**Entry criteria:** A patch that meets its specification and passes the baseline comparison.

**Activities:**
- Produce the deliverable: the patched build, a patch file (a delta against the exact original), or a script that applies the change and checks what it is applied to.
- Fix up the container and trust chain: checksums, signatures or re-signing, manifests and update metadata, so the result is accepted where it will run.
- Record hashes of the original and of the result, and the exact versions the patch applies to; make it refuse, loudly, on anything else.
- Document what changed and why, how to apply it, how to verify it and how to roll back, with the known limitations.
- Keep the tests and the baseline record with it so the result can be re-verified.

**Decision points:** Patched binary or patch file? Is distributing the patched build permitted, or only a delta?

**Pitfalls:** A patch that applies silently to the wrong build; no rollback; a re-signing step that was never tested; documentation that describes the intent rather than the actual bytes changed.

**Quality checks:** Applying the patch to the recorded original reproduces the recorded result, byte for byte; the original is untouched.

**Deliverables:** Patched build or patch file, hashes, version scope, change description, apply/verify/rollback instructions, test evidence.

Call `reactor_phase_complete` once the deliverable reproduces from the original and its scope and rollback are documented.
