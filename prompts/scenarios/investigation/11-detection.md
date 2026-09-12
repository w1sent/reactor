---
title: Stage K — IOC & Detection Signature Development
toolset: malware
---
**Goal:** Convert findings into reusable, tested detection.

**Entry criteria:** Confirmed malicious artifacts or behaviours exist.

**Activities:**
- Catalogue atomic indicators (hashes, domains, addresses, mutexes/locks, configuration keys, file paths, package identifiers, certificates).
- Author file and memory detection rules built from several distinctive patterns and derived from multiple samples.
- Build behavioural and network detections in the formats the defending estate actually consumes; tag rules with metadata and ATT&CK IDs.
- Test against the full sample set (all must match) and against a representative benign corpus for each platform (none should match); tune for runtime performance.

**Decision points:** Broad hunting rule vs. precise alerting rule? Atomic indicator vs. behavioural detection?

**Iteration:** Refine against false positives and negatives; retrohunt historical corpora and feed matches back into analysis.

**Pitfalls:** Overly generic patterns; rules derived from one sample; deploying untested rules; benign corpora that do not represent the target platform.

**Deliverables:** Machine-readable IOC set and tested detection rules with metadata and test results. Feeds Stages L, N and defensive operations.

Call `reactor_step_complete` once every rule passes the sample set and the benign corpus.
