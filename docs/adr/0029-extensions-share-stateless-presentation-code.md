# Extensions share stateless presentation code; state stays in the cache

`extensions/lib/` holds code any REactor extension may import — today, the
statusbar vocabulary (whether the anchor block is on the line, the separator a
block leads with, the styling of an error) and the frame that sets every
popup apart from the session output. [ADR-0014](0014-extensions-share-the-cache-not-each-other.md)
ruled out shared modules between extensions; this ADR scopes that rule. What
extensions still never share is **state** — probe results flow through
`reactor` and `cache.json`, and anything a module holds at module scope stays
single-owner. What they may now share is **code that holds nothing**: pure
functions over the arguments they are given.

## Why

**ADR-0014's prohibition was about state, and the lib has none.** pi loads each
extension through its own `createJiti(..., { moduleCache: false })` call, so a
module imported by N extensions is instantiated N times. For a probe cache
that is fatal — two caches that agree at first and drift apart, the failure
ADR-0014 exists to prevent. For pure functions it is a non-event: the same
code run twice, answering from its arguments, with nothing to drift. The
vocabulary is exactly that: `lead()` reads a file and colours a dot,
`errorBlock()` styles a message, neither remembers anything between calls.

**The duplication was the drift.** The statusbar's vocabulary was carried as
hand-maintained copies in six extensions, and keeping them in step was manual:
every pass over the footer re-edited six files in lockstep, and a copy that
missed an edit shipped the inconsistency the copies exist to prevent. One
stateless source of truth removes that failure mode by construction — a fix to
the lib lands in every importer on the next load, which is the opposite of the
divergence ADR-0014 warns about, and the reason that ADR's rejection ("a
shared module imported by both extensions ... does not work") does not reach
here: it was written about a probe cache, whose entire value is the state it
holds.

**Compliance becomes an import.** The statusbar grammar — anchor, separator,
glyph, colours — is otherwise an instruction to copy a pattern from a
neighbouring file and hope. With the lib, a new extension that wants a footer
block imports `lead()` and the grammar comes with it; what remains
extension-specific is the words, which only the extension knows.

## Consequences

- **The lib must stay stateless.** No module-level `let`, no caches, no
  counters, no event subscriptions — the moment it holds state, ADR-0014's
  failure mode arrives through the front door, with N independent copies that
  agree at first. The header says so; review keeps it so. State that extensions
  share still lives in `reactor.json` (preferences) and `cache.json` (probe
  results), read fresh at call time.
- **The lib is instantiated once per importer.** Nothing coordinates the
  copies and none needs coordinating; the cost is a few kilobytes of
  TypeScript compiled per extension at load, which is nothing next to the
  Python interpreter each extension already starts per turn.
- **The lib lives where discovery skips it.** `extensions/lib/` contains no
  `index.ts` and no package manifest, and pi loads an extension subdirectory
  only through one of those entry points — so the library is never loaded as
  an extension. It is reached by relative import from the extensions that use
  it.
- **ADR-0014's data rule is untouched.** No extension imports another, reads
  another's state, or learns how many others exist. Probe results still flow
  through the CLI and the cache; the lib carries presentation, never facts.
- **The grammar is pinned across the family** by
  `tests/extensions/statusbar.test.mjs`, which loads every extension and
  checks each footer block against the same rules — so a block that stops
  following the grammar fails a test instead of quietly reading wrong.

## Considered and rejected

- **Amending ADR-0014 in place.** Rejected: an ADR is a record of the
  decision as it was taken, and ADR-0014's record is correct for the question
  it answered — who owns probe results. This ADR answers a different question
  — who owns presentation — and points back at it where the reasoning is
  borrowed.
- **A shared *stateful* module: a footer registry extensions write into.** The
  exact failure ADR-0014 rejects, now with six writers and a read that cannot
  tell a fresh write from a stale one. The cache remains the only shared
  store.
- **One extension owning the whole footer.** It would have to reimplement
  pi's footer — pwd, token totals, context percentage, model, thinking level —
  to fix a separator, which is the second-implementation cost
  [ADR-0012](0012-extensions-tested-through-pi-s-own-loader.md) exists to
  warn about, paid in full.
- **Leaving the vocabulary duplicated, policed by tests alone.** The grammar
  test catches a copy that drifts, but only after the drift, and only for the
  six extensions in its table; a new extension starts from a copy, not an
  import. The lib is the cheaper habit, and the test keeps it honest.