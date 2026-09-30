# reactor-cli

The `reactor` command: a thin binary over [`reactor-core`](../reactor-core/), the
library that owns the catalogue, the probes, the cache, activation, install
recipes and upstream skills. Both are Rust, and **nothing in either needs Python**
([ADR-0039](../../docs/adr/0039-the-executables-contain-no-python.md)): a single
static binary, no interpreter to match, no `tomllib` floor.

```bash
cargo install --git https://github.com/w1sent/reactor reactor-cli    # puts `reactor` in ~/.cargo/bin
cargo install --path crates/reactor-cli    # ...or from a checkout you already have
reactor setup                              # seed ~/.reactor/, fetch skills, completions
reactor doctor                             # what is present, what is missing
```

`reactor setup` works from a bare binary: the shipped `tools.toml` and
`toolsets.toml` are compiled in, so there is no checkout to find. Before `setup`
has run, every command reads the compiled-in copy and `doctor` says so.

[ADR-0034](../../docs/adr/0034-reactor-cli-becomes-a-rust-library-with-a-binary.md)
records why this is a library with a binary over it (the GUI links the library
instead of spawning this), and
[ADR-0005](../../docs/adr/0005-reactor-cli-stdlib-python.md) why it is one
command at all — a decision that stands.

## Surface

```
reactor doctor                    what is present, what is not, how to get it
  --check-skills                  also compare fetched skills against their remote ref
reactor registry                  the block injected into the agent's system prompt
reactor services                  what is running, of the tools that declare a probe
reactor tools list                catalogue listing  [--tag T] [--active] [--present|--missing]
reactor tools show <id>           one entry in full, including install candidates
reactor tools enable|disable <id> activation state (a hint; never blocks — ADR-0007)
reactor tools reset <id>          drop the override; go back to what the toolsets say
reactor toolsets list|show <id>   named groups
reactor toolsets enable|disable   activate a group
reactor state                     what is active right now, and from which file
reactor skills list               configured upstream skills and whether they are here
reactor skills show <id>          print a skill for review before trusting it
reactor skills fetch [<id>...]    fetch configured upstream skills
reactor install <id>...           opt-in install  [--method M] [--dry-run] [--yes] [--create-venv]
  reactor install all             every catalogued tool
  reactor install 'decompile-python[all]'
                                   every python3.x the platform's own package manager
                                   offers (not all's business -- see below)
reactor refresh                   drop the probe cache and re-probe
reactor completion <shell>        print a completion script (bash, zsh, fish -- ADR-0015)
reactor diff-config               diff(1) shipped vs installed config  [--file tools|toolsets]
reactor overwrite-config          force-replace installed config (backs up first)
reactor setup                     seed ~/.reactor/, fetch upstream skills, install completions
  --no-skills  --no-completions  --dry-run
```

`reactor __complete tools|toolsets` also exists — bare, unprobed ids that the
completion scripts above shell back into. It is left out of `--help`
deliberately (ADR-0015): it is not a documented interface, only something the
scripts that ship alongside it depend on.

`--format json` is supported on every subcommand and is **not optional**: it is
the interface scripts and the GUI's CLI fallback rely on, so its shape is part of REactor's
contract and changes to it are breaking. Human-readable output carries no such
guarantee. `crates/reactor-cli/tests/cli.rs` pins the shape, and its golden files pin the bytes.

`registry`, `state`, `toolsets` and `skills fetch` were added during
implementation and are not in ADR-0005's original sketch. `registry` is the
notable one: rendering the block here rather than in the extension keeps the
determinism the prompt cache depends on testable in a single language, and
leaves the extension a pure transport.

## Constraints

- Reads `~/.reactor/`, never the shipped copies
  ([ADR-0003](../docs/adr/0003-tools-toml-single-source-of-truth.md)) — except
  `diff-config` and `overwrite-config`, whose whole job is to compare the two.
  If the installed catalogue is absent it falls back to the shipped copy and
  says so, so `reactor doctor` works before `reactor setup` has run.
- Probes have timeouts and a cached-value fallback. A hanging probe would
  stall an agent turn.
- A probe that times out is reported as *unknown*, not as absent.
- `--format json` never writes anything to stdout but the payload. Confirmation
  prompts, installer output and progress all move elsewhere in that mode.
- `install` runs a command only when its package manager was verified present.
  Free-text install keys (`manual`, a URL) are shown and never executed
  ([ADR-0010](../docs/adr/0010-install-recipes-keyed-by-package-manager.md)).
- `install 'decompile-python[all]'` is a single special-cased pseudo-target,
  not a catalogue id: it installs every `python3.x` package the platform's
  *own* package manager (never an AUR helper) currently offers, because the
  `decompile-python` skill needs to run a `.pyc` through the interpreter
  version that produced it. Never swept in by `install all`. The bracket is
  deliberate -- same shape as `pip install pkg[extra]` -- and needs quoting
  in most shells for the same reason that does.
- A `pip` recipe that fails with pip's own "externally-managed-environment"
  message (PEP 668) gets a `hint` in its `install` result pointing at
  `--create-venv`, which creates `~/.reactor/venv` (once; later calls reuse
  it) and redirects `pip` recipes into it instead of the system interpreter.
  It never touches `uv`/`pipx` recipes, which already manage their own
  isolated environment. The venv's location is always printed at the end of a
  run made with the flag.
- Nothing time-derived may reach `render_registry`. That is enforced by a test,
  not by care.
- Activation edits are **minimal**: `enable`/`disable` store an override only
  where the active toolsets do not already produce that answer, so toggling a
  tool off and on again leaves `state.json` byte-identical
  ([ADR-0011](../docs/adr/0011-selector-edits-overrides-not-outcomes.md)). Each
  entry in `tools list` carries `override` (`"on"`, `"off"`, `null`) alongside
  `active` so a client can say which of the two it is looking at.
- **A tool that is not installed has no service state.** `services` reports it
  as `unknown`, never `down` — "down" is a claim that something exists and is
  not running, and it would send the agent looking for a thing to start.
- **Every JSON document is written through a per-process temp file**, then
  renamed. Two `reactor` processes can be writing `cache.json` at the same
  moment, because every extension shells out on its own and they share nothing
  else ([ADR-0014](../docs/adr/0014-extensions-share-the-cache-not-each-other.md)).
- A toolset's `tags` **intersect**: a tool is a member when it carries every tag
  listed, not any of them
  ([ADR-0013](../docs/adr/0013-toolset-tags-intersect.md)). Union between groups
  is what activating two toolsets already does. `tools` and `tags` still union
  with each other, and `doctor` reports any toolset that ends up selecting
  nothing.

## Environment

| Variable | Effect |
|---|---|
| `REACTOR_CONFIG_DIR` | Overrides `~/.reactor`. Used by the tests. Never triggers the legacy move below. |
| `REACTOR_PACKAGE_ROOT` | Read the shipped `tools.toml`/`toolsets.toml` from this checkout instead of the compiled-in copies. For development. |

**The state root moved** from `~/.pi/reactor/` to `~/.reactor/`
([MIGRATE.md](../../MIGRATE.md)). The first run with no `REACTOR_CONFIG_DIR` set
renames the old directory into place — once, and only if the new one does not
already exist — and says so on stderr. Nothing is ever merged or overwritten.

## What asks the machine's Python

`reactor` does not run on Python, but part of the *subject matter* is Python, and
that part has to be asked of a Python that is on the machine:

- `detect = { python_module = "…" }` runs `importlib.util.find_spec` in the
  interpreter `[probe].python` names — one subprocess for every such tool at once.
  With no interpreter the answer is `unknown`, never `absent`.
- `install --create-venv` runs `<[probe].python> -m venv`, and
  `install 'decompile-python[all]'` asks the platform's own package manager which
  `python3.x` packages it has.

Both are catalogued tools' business (the RE tools that happen to be Python
libraries), not the harness's.

## Tests

```bash
cargo test                       # core (unit, fixture, golden) + the binary, spawned
cargo test -p reactor-cli        # just the binary
```

The registry block's bytes and the `--format json` payloads are pinned by
golden files captured from the Python CLI this replaced
(`crates/reactor-core/tests/golden/`, `crates/reactor-cli/tests/golden/`).
