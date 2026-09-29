# The `reactor` CLI becomes a Rust library with a binary over it

`bin/reactor` — 90 KB of stdlib-only Python — is reimplemented as
`reactor-core`, a Rust library, with `reactor-cli` as a thin binary over it.
`reactor-gui` links the library directly instead of spawning the CLI for every
panel refresh.

This supersedes the language half of
[ADR-0005](0005-reactor-cli-stdlib-python.md). Everything else that ADR
decided — one CLI, `--format json` as the machine-readable contract,
extensions never parsing `tools.toml` themselves — holds unchanged and is in
fact strengthened.

## Why

**The GUI already treats the CLI as its source of truth, and pays a process
per panel to do it.** `gui/SPEC.md` records the rule ("single source of truth:
the CLI") and the cost: `reactor … --format json` spawned for the catalogue,
the toolsets and the services, on every refresh. Linking the library keeps the
rule and deletes the cost. The seam narrows; it does not move.

**Python's guarantee was "no third-party imports"; Rust's is stronger.**
[ADR-0005](0005-reactor-cli-stdlib-python.md) chose stdlib-only Python so that
installing REactor never meant installing a dependency tree, and so the CLI
kept working when a virtualenv did not. A single static binary satisfies that
requirement more completely — no interpreter version to match, no `tomllib`
floor at 3.11, nothing on the machine to be wrong.

**Two implementations of the catalogue would be worse than either.** Once the
GUI and the agent loop are Rust, keeping the catalogue in Python means every
catalogue fact crosses a process boundary and a JSON schema forever, or gets
duplicated. The first is the cost above; the second is the drift
[ADR-0005](0005-reactor-cli-stdlib-python.md) exists to prevent.

## Consequences

- **`reactor-core` depends on nothing above it.** No rig, no gpui, no agent
  concepts. Files and subprocesses only. This is what keeps it testable
  without a model and consumable by a harness that is not ours.
- **The 90 Python tests port as the acceptance gate.** Parity with the Python
  CLI is defined as that suite passing against the Rust binary, not as a
  reading of the source. Until it does, nothing downstream moves.
- **`--format json` output is frozen at the byte level during the port.** The
  four maintained pi extensions ([ADR-0033](0033-reactor-is-a-rust-project-on-rig.md))
  consume it, and [ADR-0006](0006-registry-injected-into-system-prompt.md)'s
  prompt-cache stability depends on the registry block rendering identically.
  The byte-stability test moves from Python to Rust with the code it guards.
- **The GUI keeps a CLI-subprocess path as a debug fallback.** If the linked
  library and the binary ever disagree, that is a bug in the library boundary,
  and being able to run both is how it gets found.
- **Shell completion stays generated, not hand-written**
  ([ADR-0015](0015-shell-completion-generated-not-hand-written.md)) — the
  generator moves, the decision does not.
- **`scripts/install.py` shrinks.** A built binary replaces the symlinked
  script; seeding `~/.reactor/` and fetching upstream skills stay.

## Considered and rejected

- **Keep the Python CLI; have Rust shell out to it.** Rejected: that is the
  status quo, and its cost is a process per panel plus an interpreter
  dependency in a project that otherwise has none.
- **Keep the Python CLI; bind it with PyO3.** Rejected: it embeds a Python
  runtime in the GUI to avoid rewriting 90 KB of straightforward file-and-
  subprocess code, and makes the build harder than the rewrite.
- **Rust library for the GUI, Python CLI for everyone else.** Rejected
  outright: two implementations of the catalogue, which is the one thing
  [ADR-0005](0005-reactor-cli-stdlib-python.md) was written to prevent.
- **Split `reactor-core` into a separate repository.** Rejected for now, for
  the reason [ADR-0031](0031-reactor-gui-lives-in-the-repo-and-installs-with-it.md)
  kept the GUI here: the catalogue and its consumers change together, and a
  second release cycle buys nothing while they do.
