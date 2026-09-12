# context-editor

Hand-curating what the model sees. Two doors to the same edit:

- `/context-editor` — a landscape overlay: every context-visible entry as a
  row, toggle visibility, then apply.
- `/context-editor manual` — the same entries serialized into a text file,
  opened in `$VISUAL`/`$EDITOR`/`nano`; delete a block to keep it hidden,
  rewrite one to change what it says.

Either way, the edit ends by asking which mechanism to use: **fork** (default)
— replay the kept messages into a new session — or **filter** — keep this
session and hide the marked entries going forward. Tool calls and their
results are always handled as pairs.

Why there is no in-place rewrite, and why the two mechanisms are the only
real ones: [ADR-0021](../../docs/adr/0021-context-editor-forks-or-filters-never-rewrites.md).
