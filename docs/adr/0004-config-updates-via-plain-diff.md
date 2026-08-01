# Config updates are a plain `diff`, merged by hand

`tools.toml` and `toolsets.toml` are seeded into `~/.pi/reactor/` at install time
and are the user's files thereafter. When the shipped copies change, REactor
tells the user they differ and shows them a diff. It does not merge.

Two subcommands, both in v1:

- `reactor diff-config` — runs `diff(1)` between the shipped copy and the
  installed copy and prints the output. Nothing more.
- `reactor overwrite-config` — force-replaces the installed copy with the
  shipped one.

Install and `reactor doctor` check whether the files differ and point at
`diff-config` when they do. The user fixes their file by hand.

## Why nothing cleverer

A three-way merge was designed and discarded. It would have stashed the shipped
copy at seed time as a baseline, letting `diff-config` classify each difference
as upstream-only (safe to apply), user-only (preserve) or a genuine conflict,
and letting a `--apply` flag take the safe ones automatically.

That is strictly more capable, and it is the wrong trade here. It adds a hidden
baseline file, a classification pass, a merge implementation, a re-basing step
after apply, and a conflict-presentation format — a meaningful amount of code
and a meaningful amount of behaviour to get subtly wrong — to save the user from
running `vimdiff` a handful of times a year on a file they wrote.

`diff` is already installed, already understood, and already integrates with
whatever merge tool the user prefers. The realistic edit volume is low: a
distro-specific install recipe, a locally-built tool, an occasional probe tweak.
Hand-merging that is a minute of work.

The cost is accepted honestly: with no baseline, a 2-way diff cannot distinguish
the user's edits from upstream's changes, so the diff shows both mixed together
and grows noisier the more the user has customised. That is exactly why `--apply`
does not and cannot exist here — applying a change you cannot classify is not
safe — and why `overwrite-config` is a blunt force-replace rather than a
selective one.

## Consequences

- No baseline file, no hidden state, nothing to keep in sync.
- `diff-config` is a subprocess call and an exit code. It inherits the user's
  `$DIFF`-style preferences only insofar as `diff` itself does; a `--tool` flag
  to point at `vimdiff` or `delta` is an obvious later convenience, not v1.
- `overwrite-config` destroys local edits by design. It takes a timestamped
  backup first so that is recoverable, and it is never invoked automatically.
- Seeding never clobbers: if the file exists, install leaves it alone and
  reports the difference instead.

## Considered and rejected

- **Three-way merge with a recorded baseline** — rejected as over-built for the
  edit volume; see above.
- **An overlay model** — shipped catalogue stays authoritative and always
  current, user file holds only additions and overrides, merged at read time.
  This dissolves the problem rather than managing it: upstream changes flow
  automatically forever and user overrides always win, so neither subcommand
  would be needed. Rejected for now because it makes the full catalogue less
  discoverable — it is no longer one file you can open and read — and because
  the intent is for the installed catalogue to be the thing the user works with.
  Recorded here because it remains the better answer if hand-merging becomes
  annoying in practice.
- **`.pacnew`-style side files** (`tools.toml.new` dropped beside the live file,
  with `doctor` warning it is pending) — rejected; the same manual merge, but
  the pending files accumulate and get ignored.
