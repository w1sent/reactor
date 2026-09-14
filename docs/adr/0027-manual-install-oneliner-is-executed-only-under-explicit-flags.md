# The manual one-liner is executable, but only under explicit flags

`reactor install` has so far had exactly one kind of command it will run: a
recipe keyed by a package manager whose binary it has verified is present
([ADR-0010](0010-install-recipes-keyed-by-package-manager.md)). Every other
value in an `[tool.*.install]` table — `manual`, a bare URL — is a note, shown
but never executed. That rule stays. What changes is that a second kind of
non-manager value joins the table, one that is *meant* to be run, and two
explicit flags are the only things that can run it:

```toml
[tool.bindiff.install]
manual = "build from source -- git clone ... (needs CMake 3.14+, Ninja, ...)"
manual-install-oneliner = 'git clone https://github.com/google/bindiff && cd bindiff && cmake -S . -B build -G Ninja ... && cmake --install build --prefix "$HOME/.local"'
```

- `--auto-install-manual` — the oneliner is the **fallback**: used only when
  no manager recipe is runnable on this machine, which is exactly the case the
  `manual` note has always covered, promoted from "read this" to "do this".
- `--force-install-manual` — the oneliner **overrides** manager ranking, for
  the machine owner who wants the upstream build even though a distro package
  exists.

Both are per-invocation, opt-in, and never implied. Without either flag the
behaviour is byte-for-byte what it was before: managers ranked by `prefer`,
notes shown, nothing else run.

## Why a separate key and not the note itself

The `manual` note is prose *by design* — "install the binja-mcp plugin, then
run its scripts/install.py", "https://joern.io/ — download and add to PATH".
Executing it would mean either a heuristic that guesses which sentences are
shell (wrong half the time, and guessing at what to execute is exactly what an
installer must not do), or rewriting every note into a command and losing the
human-facing text. A separate key keeps both: `manual` remains what a human
reads, `manual-install-oneliner` is what the machine runs, and a tool that has
no runnable manual path simply has no oneliner — the flags then say so and
skip, rather than guessing.

## Why the flags and not a manager key

Making `manual` a pseudo-manager (`[platform].prefer` would rank it, `reactor
install` would select it by default) would put unverified, upstream-written
shell on the same footing as a distro-vetted package — the exact inversion
ADR-0010 exists to prevent, where `reactor install` can only ever run a command
whose manager it has verified. Explicit flags keep the default path safe and
make the unsafe one loud in both the command line and the JSON payload
(`"method": "manual"` on every plan/ran entry).

## What execution does

The oneliner runs under `sh -c` in a scratch directory,
`~/.pi/reactor/manual/<tool-id>/`, recreated per run so `git clone && ...`
chains are re-runnable. After a successful run the tool's detect binary is
looked up on `PATH`; if still absent, the scratch directory is searched and the
binary symlinked into `~/.local/bin` — the user-local PATH directory, created
on demand — with a hint when that directory is not itself on `PATH`. A oneliner
that installs into a PATH directory itself (as the influx one does) makes the
promotion step a no-op.

The scratch directory is an install location, not a temp dir: the symlink
points into it, so deleting `~/.pi/reactor/manual/` dangles the link. That is
the accepted cost of symlinking rather than copying — scripts like `mac_apt.py`
depend on sibling files in their clone and break when copied alone.

## Consequences

- `tools.toml` gains one schema path: a non-manager install key that is
  conditionally executable. `scripts/verify-recipes.py` still skips it — it
  verifies recipes against manager indexes, and the oneliner has no manager.
- Every plan/ran entry carries `method: "manual"`, so JSON consumers can tell
  an upstream build from a package-manager install without parsing commands.
- The one-liners carry their own maintenance burden: where the upstream has no
  unversioned download URL (influx-cli's portal pins versions), the oneliner
  resolves the version itself via the upstream API and so stays current.
- One-liners are written for the user-local prefix (`~/.local/bin`) wherever
  the upstream install supports a prefix — a manual build that sudo-installs
  into `/usr/local` is what the `manual` note is for.