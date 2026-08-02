# tests/

```bash
python3 tests/test_reactor.py          # all of it, verbose
python3 tests/test_reactor.py TestRegistryDeterminism
```

stdlib `unittest`, no dependencies and no runner to install — the same reasoning
that keeps the CLI stdlib-only
([ADR-0005](../docs/adr/0005-reactor-cli-stdlib-python.md)). `bin/reactor` has
no `.py` extension, so the suite imports it through `SourceFileLoader`.

Nothing here touches `~/.pi/reactor/`: fixtures point `REACTOR_CONFIG_DIR` and
the module's `CONFIG_DIR`/`PACKAGE_ROOT` globals at a temporary directory.

## What is actually being defended

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

## Not covered

The extension. `extensions/tool-registry/index.ts` was exercised by hand through
`jiti` with a stubbed `ExtensionAPI` — enough to confirm it loads, registers the
right events and produces the right `systemPrompt` and `skillPaths` — but there
is no automated TypeScript test and no test runner in the package. Worth adding
when the selector arrives and there is more than one extension to justify the
setup.
