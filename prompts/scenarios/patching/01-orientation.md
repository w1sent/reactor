---
title: Orientation
toolset: native
description: Change a program's behaviour at binary level safely: orient, specify, reverse the change site, patch and test, package.
---
**Goal:** Understand the program well enough to change it safely, and record how it behaves *before* it is changed.

**Applies when:** The program to be changed is in hand, with a reason for changing it.

**Orient:** Start from the goal and what the case already holds: its files, the graph, and what earlier scenarios concluded (listed under this briefing when there is any). Say which of the inputs this phase works from are there, state the assumption you make for any that are not, and go on. Ask only when the goal cannot be answered without what is missing.

**Activities:**
- Work on a copy; the original stays read-only and is hashed. Record its hashes, format, architecture, target platform, toolchain and version.
- Map how it is built and run: entry points, libraries and dependencies, configuration, packing or protection, signing and integrity checks that a modification would trip.
- Find how it is distributed and updated, since a patch has to survive or avoid that path.
- **Capture a baseline:** run the unmodified program under the conditions that matter and record its observable behaviour -- outputs, files, network traffic, exit codes, timing where relevant. This is the oracle the patch is tested against; without it you can only show that the change works, not that nothing else broke.

**Decision points:** Is the program packed or protected so that patching needs unpacking first (the `reverse-engineering` scenario)? Is source available, making a rebuild better than a binary patch? Can it be run here at all?

**Pitfalls:** Patching the only copy; skipping the baseline; missing a self-integrity check or update mechanism that will undo or reject the change; assuming the build on the bench is the build in the field.

**Deliverables:** Program profile (identity, hashes, structure, protections), baseline behaviour record. Feeds Scoping & specification.

Call `reactor_phase_complete` once the program is profiled and its baseline behaviour is recorded.
