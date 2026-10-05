# Observed problems
- Inspect how well the current architecture covers cache optimization: which surfaces rebuild the system prompt and when, what each transition costs against the cached prefix, and whether any of it churns per turn. Look at this later.

# Deferred
- Scenario authoring in the GUI: scenarios are directories of `NN-name.md` files with `title:` and `toolset:` frontmatter (`prompts/scenarios/`), editable only as files. An editor in the style of the identity editor — add, edit, reorder and delete phases, pick the toolset from the existing ones — would make them a thing a user writes. See ADR-0009 and ADR-0049.

