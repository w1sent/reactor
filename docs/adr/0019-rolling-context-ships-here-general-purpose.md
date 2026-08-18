# rolling-context ships here too, general-purpose and independently switched

`extensions/rolling-context/` is an existing, working pi extension — an
alternative to pi's own summarization-based compaction, for models with a
small context window and compaction that fixates on old content. It keeps a
tiny always-present manifest (goal + agent-maintained steps) at the front of
every prompt and fades everything else out of the *next* prompt once it no
longer fits a configurable budget; the session file itself is untouched, and
faded history is recoverable through three tools (`history_index`,
`history_search`, `history_read`). Brought in from a standalone draft
(`pi-rolling-context.ts`, previously distributed as a single file copied to
`~/.pi/agent/extensions/`), behaviour unchanged, adapted only in file
location and header style.

It is **off by default** and switched independently of everything else here —
`/rolling [on|off]`, not `/reactor-toolbox`. Its commands, tools, and config
file keep their original names: `/goal`, `/guidelines`, `/frame`,
`update_steps`, `history_index`/`_search`/`_read`,
`~/.pi/agent/pi-rolling-context.json`.

## Why

**Nothing in ADR-0002 restricts this package to RE-specific extensions.** Its
line is about *build steps*, not *subject matter*: "the REactor pi package
contributes only things that need no build step: extensions (`.ts`, loaded
directly by pi's `jiti`), skills, prompt templates and themes, plus a
stdlib-only Python CLI." A pure-TypeScript extension with no `package.json` of
its own is exactly what that line already describes, regardless of what it
does once loaded.

**RE sessions are an unusually good fit for what this does, even though the
mechanism has nothing RE-specific in it.** Disassembly listings, `strings`
dumps, and packet captures are exactly the kind of content that fills a
context window fast and is expensive to keep verbatim — the situation this
extension is built for. That is a reason to want it *alongside* REactor, not a
reason it has to become an REactor concern; it stays a general-purpose
extension that happens to travel well with this one.

**One `pi install` for everything a person wants installed, rather than a
second sibling package for one file.** The sibling-repo split in
[ADR-0002](0002-package-ships-assets-tools-are-sibling-repos.md) exists
because a *compiled* tool has its own release cycle pi cannot build for —
version independently, ship independently. None of that applies to a
build-step-free extension: there is no compile step to keep separate, no
release cadence of its own worth the ceremony of a second repository for a
single `.ts` file.

**Not renamed under a `reactor-` prefix.** `/reactor`, `/reactor-tools`,
`/reactor-status`, and `/reactor-scenario` share a prefix because they are
facets of *one* feature — the toolbox and its scenarios — not because every
command this package ships is required to carry it. `/rolling`, `/goal`,
`/guidelines`, and `/frame` are their own vocabulary, already documented and
already in use; renaming them would break a real person's muscle memory and
any config they already have (`~/.pi/agent/pi-rolling-context.json`) for a
purely cosmetic uniformity this repo has never actually enforced.

**Not folded into `reactor.json`.** [ADR-0016](0016-extension-toggles-live-in-their-own-pi-side-file.md)
picked "each extension owns a small file named for itself" specifically
because that is pi's own documented convention for exactly this need — and
`pi-rolling-context.json` independently arrived at the identical pattern
before the two ever met. Combining them into one file would couple two
extensions that share nothing, for no benefit beyond one fewer file on disk.

## Consequences

- **Directory layout follows this repo's convention, not the source's.**
  `extensions/rolling-context/index.ts`, matching `tool-registry/`,
  `selector/`, `status/`, and `scenario/` — not the flat `<name>.ts` the
  standalone draft shipped as. The config file keeps its original name
  regardless: it is persisted, user-facing state, not a repo-layout choice.
- **Tested to the same standard as the other four**
  ([ADR-0012](0012-extensions-tested-through-pi-s-own-loader.md)): pi's real
  loader, a faked host. This is the first extension needing prior
  *conversation* history in the fake session (not just custom entries), so
  `tests/extensions/harness.mjs`'s `makeContext` grew `sessionManager.getBranch()`,
  an optional `model`, and `getSystemPrompt()`.
- **`scripts/check-in-pi.mjs`'s default set gains `/rolling`, `/goal`,
  `/frame`** alongside the existing commands, for the same reason those are
  there: a real pi process, not a mock, is the check that needs no
  simulation.
- **Two independently-configured, independently-toggled subsystems now ship
  in one package.** A person who wants the toolbox but not rolling context,
  or vice versa, gets exactly that — `toolbox: false` in `reactor.json` says
  nothing about `/rolling`, and `/rolling off` (the default) says nothing
  about the toolbox.

## Considered and rejected

- **Leave it a separate sibling package the user installs independently.**
  Rejected: it is not a compiled tool with its own release cycle — the one
  condition [ADR-0002](0002-package-ships-assets-tools-are-sibling-repos.md)
  requires for that split — so there is nothing a second repository buys
  here that this one does not already provide for free.
- **Prefix its commands and tools `reactor-*` / `reactor_*` for uniformity.**
  Rejected per the naming point above: no other extension in this package
  has ever been required to share a prefix with the toolbox, and doing it
  here would break real, already-documented muscle memory for a consistency
  rule that does not otherwise exist.
- **Merge `pi-rolling-context.json` into `reactor.json`.** Rejected: the two
  extensions share no state and no reason to read each other's
  configuration; one file per extension is the pattern
  [ADR-0016](0016-extension-toggles-live-in-their-own-pi-side-file.md)
  already chose and this only reaffirms it.
