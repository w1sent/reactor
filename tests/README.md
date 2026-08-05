# tests/

```bash
npm test                                       # both suites

python3 tests/test_reactor.py                  # the CLI, verbose
python3 tests/test_reactor.py TestRegistryDeterminism

node --test "tests/extensions/*.test.mjs"      # the extensions
node --test tests/extensions/selector.test.mjs
```

Two suites, no runner to install in either language. The CLI's is stdlib
`unittest` — the same reasoning that keeps the CLI stdlib-only
([ADR-0005](../docs/adr/0005-reactor-cli-stdlib-python.md)). The extensions' is
`node --test`, loading each extension through pi's own loader
([ADR-0012](../docs/adr/0012-extensions-tested-through-pi-s-own-loader.md)).

`bin/reactor` has no `.py` extension, so the Python suite imports it through
`SourceFileLoader`. Nothing anywhere touches `~/.pi/reactor/`: the Python
fixtures point the module's `CONFIG_DIR`/`PACKAGE_ROOT` globals at a temporary
directory, and the extension fixtures do the same through
`REACTOR_CONFIG_DIR` — which is also how the real CLI, spawned for real, ends up
reading the fixture catalogue.

## What is actually being defended

### `test_reactor.py` — the CLI

- **`TestRegistryDeterminism`** — the load-bearing one. Replacing the system
  prompt invalidates the provider's cached prefix, so the rendered block must be
  byte-identical across turns when nothing about the machine changed, and must
  change when something did ([ADR-0006](../docs/adr/0006-registry-injected-into-system-prompt.md)).
  Both directions are asserted, plus the absence of anything time-derived — a
  timestamp or a probe duration slipping into the renderer would be invisible in
  review and expensive forever. `TestJsonContract` repeats the same check across
  two separate processes, so the cache path is covered too.
- **`TestShippedConfig`** — runs the real `tools.toml` and `toolsets.toml`
  through the real loader: every `desc` inside its length budget, every install
  key either a declared package manager or `manual`, every `prefer` entry a
  manager that exists, no toolset selecting nothing, no toolset naming a tool or
  tag that does not exist. These are the mistakes that a hand-maintained
  catalogue actually accumulates.
- **`TestJsonContract`** — `--format json` is the extensions' only interface, so
  its shape is pinned by test rather than by convention. Also checks that an
  unknown tool produces a JSON error and no traceback, and that `install` never
  runs anything without confirmation.
- **`TestRecipeRanking`** — that an install key naming no known manager can
  never become a command REactor runs
  ([ADR-0010](../docs/adr/0010-install-recipes-keyed-by-package-manager.md)).
- **`TestOverrideEditing`** — that an activation edit is the *smallest* edit
  that produces the requested outcome, so toggling is not a way to accumulate
  pins ([ADR-0011](../docs/adr/0011-selector-edits-overrides-not-outcomes.md)).

### `extensions/` — the extensions

Both files drive the extension against the real CLI, so they fail when the JSON
contract moves underneath them rather than agreeing with a stale transcription
of it. `harness.mjs` holds the fixture catalogue, the `reactor` shim and the
fake host; neither test file stubs `reactor` itself.

- **`tool-registry.test.mjs`** — that the block is *appended* to the system
  prompt rather than replacing it, that absent and deactivated tools are not
  advertised, and that skills are withdrawn when their tool is. Then the failure
  modes, which are most of the extension: a failed probe keeps the last good
  block, a missing CLI is announced once rather than every turn, non-JSON output
  and error payloads become a status line instead of an exception.
- **`selector.test.mjs`** — that keystrokes produce the writes they claim to.
  The round trip is the one to keep: toggling a tool off and back on leaves
  `state.json` byte-identical, which is what makes the selector safe to browse
  in. Also the two display bugs that were real — the overlay asking for the full
  terminal width, and a version column wide enough that `10.1.1.8388` is never
  cut into a different version — plus the invariant behind them, that no
  rendered line exceeds the width it was given.

## Keeping the suites honest

Both are checked by mutation, not just by running green: break the behaviour in
`bin/reactor` or in an extension, confirm the intended test is the one that
fails, revert. `scripts/verify-recipes.py` is the reason this is a habit here —
it passed everything it was ever given until someone noticed
`packages.debian.org` serves "No such package" with status 200. A test that
cannot fail is worse than no test, because it gets quoted as evidence.

## Not covered

- **The exec-timeout branch** in `tool-registry`. Reaching it costs the
  extension's own 20 s budget, which is longer than both suites together
  ([ADR-0012](../docs/adr/0012-extensions-tested-through-pi-s-own-loader.md)).
- **Types.** The extension tests are JavaScript and there is no `tsconfig.json`
  or `typescript` dependency in the package, so nothing here catches a wrong
  field name that an editor would.
- **`scripts/install.py`**, which is verified by running it against the real
  upstream repositories rather than by test.
