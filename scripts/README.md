# scripts/

Repo-management scripts.

## `install.py`

The second half of installing REactor. `pi install git:…/reactor` gets the
assets; this gets everything that is not a pi resource.

```bash
python3 scripts/install.py                 # symlink the CLI, seed config, fetch skills
python3 scripts/install.py --copy          # copy the CLI instead of symlinking
python3 scripts/install.py --cli-dest PATH # somewhere other than ~/.local/bin/reactor
python3 scripts/install.py --no-skills     # skip the network step
python3 scripts/install.py --dry-run       # say what would happen and stop
```

What it does:

1. Symlink (or copy) `bin/reactor` onto `PATH`. Warns if the destination
   directory is not actually on `PATH`.
2. Seed `~/.pi/reactor/tools.toml` and `toolsets.toml` from the shipped copies —
   **only if absent**. Never clobbers; if they exist and differ, it says so and
   points at `reactor diff-config`
   ([ADR-0004](../docs/adr/0004-config-updates-via-plain-diff.md)).
3. Create `~/.pi/reactor/state.json` if absent.
4. Fetch configured upstream skills into `~/.pi/reactor/skills/<tool>/`, pinned
   to the ref in the catalogue, recording source, ref, resolved commit and fetch
   time in `.reactor-skill.json`
   ([ADR-0008](../docs/adr/0008-aggregate-upstream-skills.md)). This step is
   `reactor skills fetch` in a subprocess rather than a second implementation.
5. Report. Warn **only** where a configured skill could not be fetched — a tool
   with no configured skill is the normal case and gets no warning.

Idempotent; re-running is the supported way to update.

## `verify-recipes.py`

Checks every `[tool.*.install]` recipe in the catalogue against its package
manager's real index, and exits non-zero if any recipe names a package that does
not exist.

```bash
scripts/verify-recipes.py                # the shipped tools.toml
scripts/verify-recipes.py ~/.pi/reactor/tools.toml
```

A wrong recipe is worse than an absent one, because `reactor install` will run
it — and recipes are the one part of the catalogue that cannot be checked by
reading it. So they are checked against PyPI, crates.io, the npm registry,
formulae.brew.sh, packages.debian.org, Launchpad, Fedora's mdapi, the AUR RPC,
and the local pacman sync database.

Keys that name no declared manager are notes and are skipped: they are never
executed, so there is nothing to verify
([ADR-0010](../docs/adr/0010-install-recipes-keyed-by-package-manager.md)).
A declared manager with no checker in this script is itself reported, so adding
a manager cannot silently opt out of verification.

Deliberately **not** part of `tests/`: the test suite is offline and runs in
about a second, and this is neither. Run it when you touch an install table.

It checks that the package **exists**, not that it provides the binary the
tool's `detect` looks for. Those names diverge often — Debian's `aapt` ships
`aapt2`, Arch's `android-tools` ships `adb`, `wireshark-cli` ships `tshark` — so
a recipe can name a real package that installs the wrong thing. A clean run is
not a substitute for someone who knows the tool.

This is authoring-time verification, which is a different thing from the
runtime package-availability probing ADR-0010 rejects. The cost that made
probing wrong — a network round trip per candidate per tool, on a user's
machine, to refine a recommendation they are about to read — is not a cost here,
because it is paid once by whoever edits the catalogue.

## Why installation is two steps

pi never builds anything, never initialises submodules, and runs
`git clean -fdx` inside the installed package on every update
([ADR-0002](../docs/adr/0002-package-ships-assets-tools-are-sibling-repos.md)).
So the CLI symlink, the seeded config and the fetched skills all have to happen
outside pi's package tree, by something pi does not manage.

A `postinstall` hook in `package.json` *would* fire — pi does not pass
`--ignore-scripts` on package installs — and was rejected: it would run on every
install and every update, and would put network fetches and filesystem writes in
front of pi's startup.
