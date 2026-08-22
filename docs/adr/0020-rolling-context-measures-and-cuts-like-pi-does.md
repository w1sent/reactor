# rolling-context measures tokens and cuts turns the same way pi's own compaction does

`extensions/rolling-context/`'s fade (the `context` handler) used to keep its
own bookkeeping: a hand-rolled chars/4 estimate over a *separately serialized*
copy of the session branch (built for the `history_index`/`_search`/`_read`
tools), and a plain positional slice of `event.messages` sized by how many of
those serialized entries fit. Three real bugs came from that, all with the
same root cause — the array being budgeted and the array actually being cut
were never the same array, counted in the same unit:

1. **Visual overflow.** The budget walk measured *serialized text* built by
   `serializeConversation`, not the `AgentMessage[]` actually sent. pi's own
   `getContextUsage()` (the footer, and the threshold check) measures real
   provider-reported usage for the actual request. The two estimates disagree,
   so the fade's idea of "under budget" and pi's idea of "over 100%" could
   both be true at once for the same turn.
2. **Backend rejections.** `content.length` (session branch entries) and
   `messages.length` (`event.messages`) are not the same count — compaction,
   branch-summary, and custom entries widen or narrow one without the other.
   A positional slice sized by one and applied to the other can land the cut
   inside a `toolCall`/`toolResult` pair, which every provider rejects.
   Compounding it: `session_before_compact` cancelled **every** non-manual
   reason, including `"overflow"` — pi's own last-resort recovery *after* a
   request was already rejected for exactly this. Cancelling that left a
   session that hit it with no way back.
3. **No hard ceiling.** The fade always kept the newest message whole,
   however large, with nothing stopping that from itself exceeding the real
   window once a reserve larger than expected (or a single huge tool result —
   exactly what this extension exists for) made the "soft" budget meaningless.

## Decision

Stop keeping a separate accounting. Use pi's own exported primitives
(`@earendil-works/pi-coding-agent`: `estimateTokens`, and the same
cut-point rules `findCutPoint` encodes) directly against `event.messages`,
the one array that actually matters:

- **Measure with `estimateTokens(message)`** — pi's own per-message
  chars/4 estimator, the same one `getContextUsage()` and `shouldCompact()`
  use. Whatever the fade thinks fits is now the same currency pi is counting
  in, closing bug 1 by construction rather than by tuning.
- **Cut with the same boundary rule pi's compaction uses**: walk
  `event.messages` newest → oldest, never let the kept window start on a
  `toolResult` (mirroring `isCutPointMessage`/`findCutPoint`, retargeted from
  session entries to live messages — see `findSafeCut` in the source). A
  `toolResult` can only ever follow its call, never lead a request, so this
  can no longer land mid-pair. Closes bug 2's structural half.
- **`session_before_compact` now only cancels `"threshold"`.** `"manual"`
  was already let through; `"overflow"` joins it. The fade preempting pi's
  own summarization compaction early is the whole point of this extension;
  vetoing pi's *recovery* after a request already failed was never that —
  it was accidental collateral from treating every non-manual reason alike.
  Closes bug 2's remaining half.
- **A hard ceiling, separate from the soft one.** `pct * (window - reserve)`
  is where the fade *aims*, for headroom on the next turn. `window - reserve`
  is where it may never cross — `enforceHardBudget` drops the oldest of the
  already-boundary-safe kept messages first and, if even the single newest
  turn alone is bigger than that ceiling, archives (truncates) its content
  with a pointer back to `history_search`/`history_read` rather than send it
  whole. The newest message is still never dropped outright — only shrunk —
  so the "always keep the current turn" contract survives, just bounded now.
  Closes bug 3.
- **The standing guidance now says to use the history tools sparingly.**
  Watching real transcripts showed the model reaching for
  `history_search`/`history_read` reflexively — plausible, since they were
  presented as the on-ramp to anything faded, with no cost signalled. Each
  call spends part of the same budget the fade exists to protect, so the
  system-prompt text now frames them as a recovery path for one concrete
  missing detail, not a browsing habit, and points repeat use back at
  `update_steps` (put it in the manifest instead of re-fetching it).

## Consequences

- The fade's own bookkeeping (`buildSerialized`/`serializeEntry` over
  `sm.getBranch()`) still exists, but now only for what it was always
  actually good at: the line-addressed `history_index`/`_search`/`_read`
  tools and the manifest's "visible from line N" pointer. That pointer is
  explicitly cosmetic now — computed by proportion against a differently
  counted array — since correctness no longer depends on it lining up.
- Two new knobs' worth of behavior, no new config surface: the hard ceiling
  is `window - reserve` using the *existing* `reserve` config key, and the
  soft budget is the existing `pct` against that same ceiling instead of
  against the raw window.
- `tests/extensions/rolling-context.test.mjs` gained three regression tests
  aimed at the bugs directly (never orphaning a tool result; archiving a
  turn bigger than the hard ceiling instead of overflowing; `"overflow"`
  passing through `session_before_compact` uncancelled), and
  `tests/extensions/harness.mjs` needed no changes — the fix stayed inside
  what the existing fake host already modeled.

## Considered and rejected

- **Tune the chars/4 constant instead of switching estimators.** Rejected:
  the bug was never that chars/4 is a bad approximation (pi's own
  `estimateTokens` is the same heuristic) — it is that it was applied to a
  *different string* than what got sent. No constant fixes a unit mismatch.
- **Keep the positional slice, but sanity-check `content.length ===
  messages.length` first and fall back to "keep everything" on mismatch.**
  Rejected: that fallback is exactly the overflow bug, just detected instead
  of prevented — and the mismatch is the *common* case (a manifest or
  branch-summary entry any turn), not an edge case worth a bailout path.
- **Cancel `session_before_compact` only for `"threshold"`, leave `"manual"`
  as the sole other exception, and require the extension to explicitly
  re-derive whether an `"overflow"` is trustworthy.** Rejected as needless
  caution: `"overflow"` only ever fires after pi itself has already decided
  a request was rejected for size — there is nothing left for this extension
  to second-guess by that point, and pi-api-notes.md's own advice ("closest
  equivalent" reasoning throughout this package) is to trust what pi's
  runtime has already established rather than re-litigate it.
