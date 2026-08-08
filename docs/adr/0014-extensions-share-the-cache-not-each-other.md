# Extensions share probe results through the cache, never through each other

Every extension shells out to `reactor` on its own. `cache.json` and its TTLs
are the entire sharing mechanism: no extension imports another, publishes to
another, or reads another's state, and none of them knows how many others exist.

`reactor` grows a `services` subcommand so that a status view has a query shaped
like the question it is asking, rather than mining `doctor` for the one field it
wants.

Concurrent writers to `cache.json` are left racing, deliberately — with one fix,
below, that stops a race from publishing a torn file.

## Why

**A shared TypeScript module cannot hold shared state, so the obvious design is
not available.** pi loads each extension through its own `createJiti(...,
{ moduleCache: false })` call, so a module imported by two extensions is
*instantiated twice*. A `probeCache.ts` imported by both would compile, run, and
produce two independent caches that agree at first and drift apart — the worst
possible failure, because it looks like it works. This is a fact about pi's
loader, not a preference, and it rules out the design that would otherwise be
first choice.

**The cross-extension channel that does exist buys coupling.** `pi.events` is
real, and one extension could own probing and publish results to the other. It
would work. It also means a load-order dependency, a failure in the producer
silently emptying the consumer's display, and a second implementation of "when
is it time to probe" living in TypeScript — which is the thing
[ADR-0005](0005-reactor-cli-stdlib-python.md) exists to prevent.

**The cache is already the shared store, and it is already correct for this.**
It is keyed on a stamp of `tools.toml`, so editing the catalogue invalidates it;
entries carry timestamps and are read through TTLs, so two readers cannot
disagree for longer than `service_ttl` (30 s) or `detect_ttl` (300 s). Two
extensions reading it a millisecond apart get the same answer because they are
reading the same file, not because anything coordinated them.

**The cost of not sharing is process startup, not probing.** Measured warm, on
the shipped catalogue: `reactor state` — which loads config and probes nothing —
is 70 ms, `reactor registry --cached` is 72 ms, and `reactor registry` with the
TTL machinery live is 111 ms. So the second extension's per-turn call costs
about 70 ms of Python interpreter start plus whatever probing the TTLs actually
call for, which on most turns is none. Against a turn containing an LLM round
trip, that is not a number worth designing around —
[ADR-0006](0006-registry-injected-into-system-prompt.md) reached the same
conclusion about the first 79 ms.

**Last-writer-wins is safe here because writers load, then update.** Each
process reads `cache.json`, replaces only the keys it probed, and writes the
whole document back. So a lost update loses *freshness*, never correctness: the
loser's entries revert to what was on disk, and the next call past the TTL
re-probes them. There is no state where the cache holds an answer nobody
measured.

## Consequences

- **The temp file needed a unique name.** `_write_json` wrote every document
  through a fixed `<name>.tmp` before renaming. The rename is atomic, so a
  reader never saw a partial file — but two processes writing `cache.json.tmp`
  at once could interleave their writes and then rename the mixture into place,
  publishing a torn file that was never anyone's state. The temp name now
  carries the pid. This was the only genuine corruption path, and it existed
  before any of this and applies to `state.json` too.
- **A corrupt cache is not an error.** `Cache.__init__` catches `OSError` and
  `ValueError` and falls back to an empty document, so the worst outcome of any
  race is a re-probe.
- **Status is turn-scoped, not polled.** No timer, no background probe. The
  footer reflects the machine as of the last turn — which is the moment the
  agent acts on it, so this is the right semantics rather than a concession. A
  device plugged in mid-turn shows up on the next one, or immediately via the
  refresh command.
- **Two extensions spawn `reactor` per turn instead of one.** If a third arrives
  this stops being free and the answer changes — most likely to one extension
  fetching a combined payload and the others rendering it, which is a different
  ADR and needs the coupling argument re-run.
- **`reactor services` is a new stable JSON surface**, so it is pinned by test
  like the rest of `--format json`.

## Considered and rejected

- **A shared module imported by both extensions.** Rejected because it does not
  work — see above. Worth recording precisely because it is the design anyone
  would reach for first, and because its failure is silent.
- **One extension owns probing; the other subscribes via `pi.events`.** The
  serious alternative. It removes the duplicate spawn and gives a single place
  where probe scheduling lives. Rejected on coupling: two extensions that must
  load in an order, a producer whose failure empties a consumer, and a
  scheduling policy in TypeScript that the CLI already implements in TTLs. The
  saving is 70 ms a turn.
- **`status/` reads `cache.json` directly.** Tempting: it is right there, it is
  JSON, and it is exactly the data. Rejected because `cache.json` is an internal
  format with no contract — bucket names, the `stamp` key, the TTL semantics —
  and a second reader in a second language turns it into one
  ([ADR-0005](0005-reactor-cli-stdlib-python.md)). The JSON *commands* are the
  contract; the files behind them are not.
- **File locking around the cache.** `fcntl.flock` would serialise writers and
  make lost updates impossible. Rejected twice over: it is POSIX-only in the
  stdlib, and the thing it prevents is harmless here, because load-then-update
  means a lost write costs a re-probe rather than a wrong answer. Complexity for
  a failure mode that is already benign.
- **A long-lived `reactor` daemon extensions talk to.** Removes the startup cost
  entirely and makes genuinely live status possible. Rejected as far too much
  machinery — a lifecycle, a socket, a protocol, and an orphan-process problem —
  bought for 70 ms and a footer that updates between turns instead of during
  them.
- **Folding the status line into `tool-registry`.** One extension, one spawn, no
  question to settle. Rejected because the two do unrelated jobs: one shapes
  what the *model* is told and must be byte-stable across turns
  ([ADR-0006](0006-registry-injected-into-system-prompt.md)), the other shows a
  *human* volatile machine state. Merging them would put a changing widget in
  the same file as the thing whose whole contract is not changing.
