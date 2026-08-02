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
