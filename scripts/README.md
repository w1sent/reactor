# scripts/

Repo-management scripts. Not implemented yet.

## `install.py` (planned)

The second half of installing REactor. `pi install git:…/reactor` gets the
assets; this gets everything that is not a pi resource.

1. Symlink `bin/reactor` onto `PATH` (`--cli-dest`, default
   `~/.local/bin/reactor`).
2. Seed `~/.pi/reactor/tools.toml` and `toolsets.toml` from the shipped copies —
   **only if absent**. Never clobber; if they exist and differ, report it and
   point at `reactor diff-config`
   ([ADR-0004](../docs/adr/0004-config-updates-via-plain-diff.md)).
3. Create `~/.pi/reactor/state.json` if absent.
4. Fetch configured upstream skills into `~/.pi/reactor/skills/<tool>/`, pinned
   to the ref in the catalogue, recording source/ref/fetch-time
   ([ADR-0008](../docs/adr/0008-aggregate-upstream-skills.md)).
5. Report. Warn **only** where a configured skill could not be fetched — a tool
   with no configured skill is the normal case and gets no warning.

`--link` and `--copy` modes mirroring the plugins repo installer.

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
