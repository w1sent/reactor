# The REactor executables contain no Python

Nothing that ships as `reactor`, `reactor-gui`, or anything that will ship after
them may need a Python interpreter to run, install, or update. Python remains
welcome in the repository where it is *tooling* (scripts that authors run) or
*content* (scripts inside a skill, which the agent runs on the analysed machine).

This is the rule that finishes [ADR-0034](0034-reactor-cli-becomes-a-rust-library-with-a-binary.md):
porting `bin/reactor` to Rust would have been half a decision if the thing that
puts it on the machine were still `python3 scripts/install.py`.

## What follows

- **`scripts/install.py` is replaced by `reactor setup`.** Seeding
  `~/.reactor/`, fetching upstream skills and writing shell completions are the
  binary's own subcommand. Building and placing the binary is `cargo install`.
  Same semantics as before — seeding never clobbers, `diff-config` is how you
  see drift ([ADR-0004](0004-config-updates-via-plain-diff.md)), re-running is
  the update path — minus the interpreter.
- **The shipped catalogue is compiled in.** The old CLI found `tools.toml` by
  walking up from its own symlink; a static binary has no checkout to walk to.
  `tools.toml` and `toolsets.toml` are embedded, so `reactor setup` works from a
  bare binary and every command falls back to the compiled-in copy — and says so
  in `doctor` — until it has run.
  `REACTOR_PACKAGE_ROOT` points at a checkout instead, for development.
- **`--format json` loses one field.** `doctor`'s `platform.python` reported the
  *CLI's own* interpreter, which no longer exists. It is replaced by
  `platform.reactor`, the version. Nothing read it (checked in the extensions and
  in the GUI's client). Every other byte is unchanged, and a differential test
  against the Python original holds that line until the original is deleted.
- **Part of the subject matter is still Python, and stays a question put to the
  machine.** `python_module` detection, `install --create-venv` and
  `install 'decompile-python[all]'` exist *because some RE tools are Python
  libraries*. They ask the interpreter `[probe].python` names, or the platform's
  package manager. Absent an interpreter, detection answers `unknown` — never
  `absent`, which is the same rule as any probe that could not be run. This is
  not REactor using Python; it is REactor reporting on it.
- **Skills may contain Python.** `skills/decompile-python/scripts/*.py` and
  `skills/bindiff/scripts/*.py` run where the agent runs, on the analysed
  machine, and are content.
- **Repository tooling may be Python.** `scripts/verify-recipes.py` checks
  install recipes against package indexes at authoring time; it is run by a
  person editing the catalogue, is not installed, and is not required by users.
  `scripts/parity.py` and the Python original it diffs against are migration
  scaffolding, deleted with `bin/reactor`.

## Considered and rejected

- **Keep `install.py`; it only runs once.** Rejected: "install REactor" would
  still begin with "have Python 3.11+", which is the floor
  [ADR-0034](0034-reactor-cli-becomes-a-rust-library-with-a-binary.md) removed
  from the binary. An installer's requirements are the product's requirements.
- **Rewrite the installer as a `cargo xtask`.** Rejected: an xtask needs a
  checkout and a toolchain, and `reactor setup` needs neither once the binary
  exists. Seeding config is a runtime concern (it must be re-runnable, and it
  reads the *installed* binary's shipped copy), so it belongs in the runtime.
- **Detect Python modules without Python — walk `site-packages`.** Rejected:
  which directories count, and what `find_spec` would say about namespace
  packages, editable installs and `.pth` files, is exactly what the interpreter
  knows and a reimplementation gets wrong. Asking is cheap (one subprocess for
  all modules, cached) and honest about being unable to.
- **Keep `platform.python` in `doctor`, by probing `[probe].python --version`.**
  Rejected: it would preserve a byte for a field nobody reads by adding a
  subprocess to the slowest command. The version of the thing reporting is more
  useful in a bug report than the version of an unrelated interpreter.
