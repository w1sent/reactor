# reactor-context is a port, verified against the frozen extensions — not a set of shims

`reactor-context` reimplements the logic of the four session-state extensions —
`goal-setting` (the manifest), `identity`, `reporting`, `scenario` — as Rust state
machines plus deterministic rendering. The pi extensions are **not** changed to
call it. They stay exactly as they are, frozen
([ADR-0035](0035-portable-surface-is-machine-facts.md)), and act as the
*specification*: a capture script drives each of them through pi's own loader and
records what they said and did, and the Rust must reproduce that byte for byte.

This replaces the wording of [MIGRATE.md](../../MIGRATE.md) phase 3, which had the
extensions "become shims over it" so that the portable core would run in
production through pi.

## Why

**Shims need a session-scoped surface that [ADR-0035](0035-portable-surface-is-machine-facts.md)
deliberately does not build.** The extensions are TypeScript inside pi's process;
the only ways a Rust crate can be reached from there are a subprocess (`reactor
context …`, a per-turn process that reads and writes a manifest, an identity and a
scenario phase — exactly the `--session` surface ADR-0035 rejects, with the
locking and ownership questions it names) or a WASM build (a new toolchain, an
npm-side build step and a loader, added to a package whose whole status is
"frozen"). Both add moving parts to the half that is meant to stop moving, to
prove something a test proves more cheaply.

**The thing the shims were meant to prove is that the port is faithful, and a
capture proves it directly.** "Runs in production through pi" was a proxy for
"produces the same blocks as the code it replaces". Recording the extensions'
behaviour and replaying it against the Rust asserts that property itself, over
every command branch, and it does so on every `cargo test` with nothing running.

**A frozen specification is a good specification.** The extensions will not
change, so the goldens will not either; `capture.mjs --check` fails if either
ever does, which is how a change to a frozen extension gets noticed.

## What the capture found

Two divergences a careful read of the TypeScript had missed, both caught the first
time the replay ran:

- The derive prompt contains `""` where the source reads `\"\"` — inside a JS
  single-quoted string the backslash is an escape, so it is not in the output.
- A CRLF scenario file loses every frontmatter field but the last, because JS `.`
  excludes `\r` and the closing delimiter's `\r?` swallows only the final line's.
  Quirks like that are kept: the point of the port is faithfulness, and a scenario
  author's file behaves the same under both harnesses.

## Consequences

- **`crates/reactor-context/tests/golden/*.json` are the contract.** They are
  regenerated only by `tests/extensions/golden/capture.mjs`, which needs pi and a
  Node with TypeScript support like the extension suite does, and are committed so
  that `cargo test` needs neither.
  *(Since [ADR-0043](0043-the-pi-flavor-is-removed-from-the-tree.md) the script and the
  extensions exist only in git history, at commit `014a9b8`; the goldens are now plain
  fixtures.)*
- **Effects are data.** A command returns what it did to the world — an entry to
  append, a toolset to enable, a settings file to write — for the caller to
  perform. That is what lets the crate stay ignorant of a session store that does
  not exist yet ([ADR-0036](0036-reactor-owns-its-session-store-format.md)).
- **The settings cascade is written once, global half only.**
  [`settings::resolve`](../../crates/reactor-context/src/settings.rs) is the single
  place "session override wins, absent inherits" lives
  ([ADR-0038](0038-settings-resolve-global-then-session.md)); until the store
  exists, each module's own per-session state *is* the override.
- **Some UI-shaped text is not ported.** The footer row, its colours and its width
  handling are presentation, and belong to whichever frontend renders it. Anything
  that reaches the *model* is ported and compared.
- **Not yet wired to anything.** Nothing calls `reactor-context` until the agent
  loop exists (phase 4). What phase 3 buys is that when it does, the blocks it
  injects are already known to be right.

## Considered and rejected

- **Shims through a `reactor context` subcommand.** Rejected: it is the
  session-scoped CLI surface ADR-0035 rules out, plus a process per turn.
- **Shims through WASM.** Rejected: a WASM toolchain and loader for the frozen
  half, to prove what a golden proves.
- **Port without capturing — read the TypeScript and match it by eye.** Rejected:
  the capture caught two mismatches in its first run that reading had not.
- **Also port the footer and the widgets.** Rejected: that is a frontend's job, and
  the GUI has its own.
