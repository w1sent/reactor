# REactor owns its session store format

The Rust harness defines its own on-disk session format rather than reading or
writing pi's. Append-only JSONL plus an index; branches as parent pointers on
entries; a context reduction recorded as an entry describing the range it
covers, with the original entries left in the log.

## Why

**Compatibility with a format we do not own is a standing obligation.**
Reading pi's session files means tracking pi's changes to them forever, for a
handoff [ADR-0035](0035-portable-surface-is-machine-facts.md) already
established is not wanted. `docs/pi-api-notes.md` exists because pi's internals
had to be verified version by version; extending that discipline to a
persisted format is the same cost with no end date.

**The store has to hold things pi's does not.** Which range a summary
replaced, so it can be shown and undone. Branch structure the GUI's tree
renders directly. What a context reduction discarded, so the history tools can
still reach it. Designing those into a format we control is straightforward;
retrofitting them onto one we do not is not.

**Originals must survive reduction, or the history tools go blind.** The fade
already depends on this — "the session file itself is untouched, and faded
history is recoverable" ([ADR-0019](0019-rolling-context-ships-here-general-purpose.md)).
[ADR-0037](0037-context-reduction-is-one-budget-manager.md) extends the same
requirement to summarization, which is the stronger case: everything older
than the last compaction is exactly the range `history_search` exists to
reach.

## Consequences

- **No session moves between pi and reactor-gui, in either direction.** A
  session started in one is finished in that one. This is a real loss against
  today, where a GUI session is a normal pi session file resumable from the
  terminal (`gui/SPEC.md` §1), and it is the price of the two reasons above.
- **[ADR-0021](0021-context-editor-forks-or-filters-never-rewrites.md)'s
  constraint is lifted.** "Never rewrites history" was imposed by pi owning
  the session file. With originals retained and reductions recorded as
  entries, restoring a summarized range is a supported operation rather than a
  rewrite — the log is still append-only.
- **Append-only is the crash story.** A killed process loses at most the entry
  being written; there is no rebuild step and no lock file in the common path.
- **The index is derived and disposable.** It exists for `history_index` and
  the GUI's tree; it is rebuilt from the log if missing or stale, so its format
  can change without migrating anything.
- **[ADR-0018](0018-pi-itself-is-a-catalogue-entry-non-interactive-only.md) is
  unaffected.** pi as a non-interactive catalogue entry has nothing to do with
  session files.

## Considered and rejected

- **Read and write pi's session format.** Rejected per the reasoning above: a
  permanent obligation in exchange for a handoff that is out of scope.
- **SQLite.** Rejected for now, not on principle. It would give the index for
  free and make range queries trivial. It also makes the store opaque to
  `grep`, `jq` and `tail -f` — which is how this project debugs everything
  else — and adds a dependency and a migration story before there is evidence
  the query load needs one. Revisit if the derived index becomes the expensive
  part.
- **One file per entry on disk.** Rejected: thousands of small files per
  session, and ordering becomes a filename convention.
- **Keep a rewritable transcript and drop originals on compaction.** Rejected:
  it breaks the history tools over precisely the range they exist for, and
  makes the GUI's undo impossible.
