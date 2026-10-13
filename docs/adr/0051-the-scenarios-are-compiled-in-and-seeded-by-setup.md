# The scenarios are compiled in and seeded by `reactor setup`, and their absence is said

A fresh install listed no scenarios. They are files under `prompts/scenarios/`, the GUI
found them only next to a checkout or in `~/.reactor/prompts/scenarios`, and nothing put
them there: `reactor setup` seeded the catalogue and nothing else. A binary installed with
`cargo install` has no checkout beside it, and the list was empty with no word about why.

## Decisions

**The scenarios are compiled into `reactor-core`, as `tools.toml` is.** A `build.rs` lists
`prompts/scenarios/` and generates the table, so a scenario added to the directory ships
with no second list to keep in step
([ADR-0039](0039-the-executables-contain-no-python.md): a bare binary must be able to
set itself up).

**`reactor setup` seeds them into `<config dir>/prompts/scenarios/`, file by file, and
never overwrites.** A file that is absent is written; one that matches is left; one that
differs is the user's and stays, named in a warning
([ADR-0004](0004-config-updates-via-plain-diff.md)). A file the user deleted comes back on
the next run, as a deleted `tools.toml` does. `--dry-run` writes nothing.
`REACTOR_PACKAGE_ROOT` still wins: the files of that checkout are the shipped ones.

**Where the GUI looks follows the config directory.** After `REACTOR_PACKAGE_ROOT` and a
checkout beside the binary, it reads `REACTOR_CONFIG_DIR` (else `~/.reactor`), which is
where `setup` wrote.

**Their absence is said.** `reactor doctor` reports `scenarios-missing` when the config
directory holds none (not when the catalogue is still the shipped one, which `not-installed`
already covers, nor when a checkout supplies them). The GUI's first session warns once
when it finds no scenarios directory at all.

## Consequences

- An edited scenario is never updated by a later release; `setup` lists the files that
  differ, and the user takes the new copy by deleting theirs and re-running it.
- The authored `skills/` directory goes through the same lookup and is not seeded by this
  decision.
- The library carries ~100 KB of Markdown.
