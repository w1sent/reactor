# context-editor forks a new session or filters the current one; it never rewrites history

`extensions/context-editor/` gives a person two ways to decide what the agent
sees, on top of (and independent of) whatever `rolling-context/`'s automatic
fade is doing: a *landscape* view (`/context-editor`) listing every
context-visible entry with a toggle, and a *manual* view
(`/context-editor manual`) that dumps the same entries to a text file and
opens `$VISUAL`/`$EDITOR`/`nano` on it — same resolution order pi's own
external-editor prompt uses.

Both end by asking how the edit takes effect, and the two answers are not
symmetric, because pi's `SessionManager` is not symmetric: it is an
append-only tree whose own doc comment says entries "cannot be modified or
deleted." There is no operation this extension (or any extension —
`ReadonlySessionManager` is what `ctx.sessionManager` actually is) can call to
rewrite or remove an entry in place. Two real mechanisms exist to build on
that constraint, not around it:

## Decision

- **Fork (default)** — `ctx.newSession({ setup })` builds a genuinely new
  session, replaying only the kept entries (and, for entries whose content is
  plain text, the edited body) in order via `sm.appendMessage()`. This is a
  real, clean "new branch with the new context": nothing about the current
  session is touched, and the new one contains exactly what was chosen,
  nothing else. It is the only path that can honor a *text* edit from the
  manual view, since replaying is the only way this package can ever produce
  a message with different content — there is no in-place edit to fall back
  to.
- **Current branch** — since nothing can be deleted, this applies the edit as
  a **standing filter** instead: hidden entry ids persist in the session as a
  `custom` entry (`pi.appendEntry`, restored on `session_start` — the exact
  pattern `rolling-context/` and `scenario/` already use for their own state)
  and a `context` handler drops them from `buildContextEntries()` before
  building the outgoing messages, every future turn. The session file itself
  is unchanged — the same "nothing is lost, only unsent" guarantee
  `rolling-context/`'s fade makes, just driven by an explicit choice instead
  of a budget. Because there is no in-place rewrite, current-branch mode can
  hide an entry but cannot reword it — a text edit applied this way is
  refused with a message pointing at fork instead of silently discarded or
  silently ignored.

**Hiding closes over tool-call/tool-result pairs.** A `toolResult` sent
without the assistant message that made the call (or vice versa) is a
malformed request every provider rejects — the same class of bug ADR-0020
fixed in `rolling-context/`'s automatic fade. `closeHidden` extends whatever
the person toggled so a call and its result are always hidden or shown
together, in both the fork replay and the current-branch filter.

**The manual view's file format keeps the *previous* current-branch hidden
state distinguishable from "untouched."** A row already hidden is written as
an explicit marker (`[HIDDEN -- delete this block to keep it hidden, or
replace this line with real content to reveal it]`), not its real body.
Without that, leaving an already-hidden block alone in the file — the natural
thing to do with content someone already decided to hide — would be
indistinguishable from never having touched it, and the block would silently
come back the moment the file round-trips. The marker turns "still hidden"
and "now visible with this content" into two different, visible states of
the same block instead of one ambiguous one.

## Consequences

- **Two mechanisms, one code path.** Both views resolve to the same `Edit`
  shape (`hiddenIds`, `textEdits`) and the same `applyEdit`, so "landscape
  toggle" and "manual delete" are two ways of producing identical input to
  one apply function, not two apply implementations to keep in sync.
- **The manual view has to suspend pi's own TUI before the external editor
  can get any keystrokes.** pi's TUI holds the terminal in raw mode for its
  own input handling; spawning `vi`/`nano`/etc. with `stdio: "inherit"`
  while that is still active means both are reading the same stdin at once,
  and the editor never sees a keypress -- the bug this shipped with
  initially. `ctx.ui.custom` is the only way an extension gets the `tui`
  handle needed to suspend that (`tui.stop()` before spawning,
  `tui.start()` after), the same shape pi's own
  `examples/extensions/interactive-shell.ts` uses to hand a shelled-out
  interactive command the terminal — confirmed against that example rather
  than assumed, after the naive `spawn(..., {stdio:"inherit"})` version
  reached a real terminal and silently ate every keystroke.
- **`ctx.newSession()` invalidates `ctx` (and the closed-over `pi`) the
  moment it resolves**, same as `ctx.reload()`
  (`docs/pi-api-notes.md`) — the fork branch's `ctx.ui.notify()` call has to
  happen *before* `newSession`, not after; there is nothing to confirm
  afterward anyway, since the new session is now the active one. Caught by
  `tests/extensions/context-editor.test.mjs`, which is why `harness.mjs`'s
  `newSession` fake sets `guard.stale = true` the same way its `reload` fake
  already did — a real ordering bug, reproduced by the same class of test
  that caught `/reactor-toolbox off`'s original one (ADR-0016-era fix, noted
  in `docs/pi-api-notes.md`).
- **Compaction and branch-summary entries are dropped on fork, not
  replayed.** `sm.appendMessage()` does not accept them (`SessionManager`
  gives those their own `appendCompaction`/`branchWithSummary` methods), and
  a stale compaction pointer would be meaningless in a session rebuilt from a
  hand-picked entry set anyway. Rare in practice: `rolling-context/`
  recommends leaving pi's own threshold compaction off (ADR-0020), and this
  extension's own current-branch filter never produces one.
- **`harness.mjs` gained `ui.select` and `newSession` fakes**, deterministic
  rather than interactive: `select` answers from a queue
  (`selectAnswers`, empty = "dismissed"), `newSession` runs `setup` against a
  minimal recorder exposing only `appendMessage` and records what was
  appended for a test to assert on. Neither models pi's real tree/compaction
  machinery — `buildContextEntries` in the fake is just `getBranch()` again —
  which stays true to ADR-0012's split: this extension's own logic is what
  the mock verifies, pi's own session internals are what
  `scripts/check-in-pi.mjs`'s real process is for.

## Considered and rejected

- **A literal in-place rewrite for "current branch" mode**, hunting for some
  `SessionManager` escape hatch that mutates history. Rejected on the facts,
  not preference: `ReadonlySessionManager` is what extensions actually get,
  and even the full `SessionManager` a `newSession`/`fork` `setup` callback
  receives has no delete or edit method — only `appendXXX` — because the
  session file is append-only by design. There is nothing to find.
- **Silently dropping text edits in current-branch mode instead of
  refusing them with a message.** Rejected: a person who typed a correction
  and had it silently discarded would reasonably conclude the tool is
  broken, not that they picked the one apply mode that cannot express a
  reword. Saying so once, at the moment it matters, costs one notify call.
- **Let a current-branch apply merge new hidden ids into the old set instead
  of replacing it wholesale.** Rejected: the landscape overlay always opens
  seeded with the current hidden set (already-hidden rows show hidden), and
  the manual view's marker scheme does the same for the text file — so the
  toggle state the person finishes with **is** the complete desired set
  already, including anything they chose to un-hide. Merging on top of that
  would make un-hiding impossible without reaching for something more manual
  than the "toggle it back" a person would reasonably expect to work.
