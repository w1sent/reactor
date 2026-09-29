# tests/

```bash
cargo test                                     # the CLI and core
cargo test -p reactor-core render              # one file of it
cargo test the_block_is_byte_identical         # one test

node --test "tests/extensions/*.test.mjs"      # the extensions (needs `cargo build` first)
node --test tests/extensions/selector.test.mjs
```

Two suites, no runner to install in either language: `cargo test` for the CLI
and core, and `node --test` for the extensions, loading each extension through
pi's own loader
([ADR-0012](../docs/adr/0012-extensions-tested-through-pi-s-own-loader.md)). The
extensions' harness execs the Rust binary — `$REACTOR_BIN`, else
`target/release/reactor`, else `target/debug/reactor`.

Nothing anywhere touches `~/.reactor/`: the Rust fixtures point a `Paths` at a
temporary directory, and the binary is spawned with `REACTOR_CONFIG_DIR` and a
throwaway `HOME` — which is also how the extension fixtures make the real CLI
read their fixture catalogue.

`bin/reactor` (Python) and `scripts/parity.py` are migration scaffolding and the
only Python here: `python3 scripts/parity.py` diffs the Rust binary against the
original across ~150 commands, byte for byte, until both are deleted
([ADR-0039](../docs/adr/0039-the-executables-contain-no-python.md)).

## What is actually being defended

### `crates/*/tests` — the CLI and core

`reactor-core/tests/` drives the library against fixture catalogues;
`reactor-cli/tests/cli.rs` spawns the binary. Test names below are the Rust ones.

- **`render.rs`: determinism and the golden block** — the load-bearing one. Replacing the system
  prompt invalidates the provider's cached prefix, so the rendered block must be
  byte-identical across turns when nothing about the machine changed, and must
  change when something did ([ADR-0006](../docs/adr/0006-registry-injected-into-system-prompt.md)).
  Both directions are asserted, plus the absence of anything time-derived — a
  timestamp or a probe duration slipping into the renderer would be invisible in
  review and expensive forever. `the_block_is_byte_identical_to_the_python_renderers`
  pins the bytes against golden output captured from the renderer this one
  replaced, and `cli.rs` repeats the determinism check across two separate
  processes, so the cache path is covered too.
- **`shipped.rs`** — runs the real `tools.toml` and `toolsets.toml`
  through the real loader: every `desc` inside its length budget, every install
  key either a declared package manager or `manual`, every `prefer` entry a
  manager that exists, no toolset selecting nothing, no toolset naming a tool or
  tag that does not exist. These are the mistakes that a hand-maintained
  catalogue actually accumulates.
- **`cli.rs`, and its `golden/` directory** — `--format json` is the
  extensions' only interface, so its shape is pinned by test rather than by
  convention, and its bytes by golden files (non-ASCII escaping, key order and
  all) captured from the Python CLI. Also checks that an
  unknown tool produces a JSON error and no traceback, and that `install` never
  runs anything without confirmation.
- **`catalogue.rs`: recipe ranking** — that an install key naming no known manager can
  never become a command REactor runs
  ([ADR-0010](../docs/adr/0010-install-recipes-keyed-by-package-manager.md)).
- **`catalogue.rs`: override editing** — that an activation edit is the *smallest* edit
  that produces the requested outcome, so toggling is not a way to accumulate
  pins ([ADR-0011](../docs/adr/0011-selector-edits-overrides-not-outcomes.md)).
- **`probe.rs`: services** — that a tool which is not installed reports `unknown`
  rather than `down`, which is the one way this command can lie.
- **`probe.rs`: atomic writes** — that the temp file a JSON write goes through is
  per-process, since two `reactor` processes can be writing the cache at once
  ([ADR-0014](../docs/adr/0014-extensions-share-the-cache-not-each-other.md)).

### `extensions/` — the extensions

All three files drive their extension against the real CLI, so they fail when
the JSON contract moves underneath them rather than agreeing with a stale
transcription of it. `harness.mjs` holds the fixture catalogues, the `reactor`
shim and the fake host; no test file stubs `reactor` itself.

- **`tool-registry.test.mjs`** — that the block is *appended* to the system
  prompt rather than replacing it, that absent and deactivated tools are not
  advertised, and that skills are withdrawn when their tool is. Then the failure
  modes, which are most of the extension: a failed probe keeps the last good
  block, a missing CLI is announced once rather than every turn, non-JSON output
  and error payloads become a status line instead of an exception — styled as
  the statusbar's other blocks: `⌗ <present>/<catalogued> tools` under an
  anchor-coloured glyph, failures as a red ✗.
- **`goal-setting.test.mjs`** — the manifest row: its own line above the
  footer, with the steps count (muted, warning past the soft limit), a goal
  that ellipsizes before the count does on a narrow window, cleared and
  paused states hiding the row, and the rpc path re-sending the row as a
  string snapshot, since a snapshot cannot read module state at paint time.
- **`status.test.mjs`** — that the footer says what is running and nothing
  else, and that it reads at a glance: down services first, uninstalled ones
  left out, state glyph and state words coloured by state, service ids
  coloured by service from a rotation that never carries state meaning, the
  same colour in footer and panel. Then the width ladder — details shed
  first, then names, then counts, a number never cut into a different
  number — and the panel wrapping instead of truncating. Plus the panel's
  lifecycle — toggled, repainted on a turn while up, not drawn while hidden.
- **`selector.test.mjs`** — that keystrokes produce the writes they claim to.
  The round trip is the one to keep: toggling a tool off and back on leaves
  `state.json` byte-identical, which is what makes the selector safe to browse
  in. Also the two display bugs that were real — the overlay asking for the full
  terminal width, and a version column wide enough that `10.1.1.8388` is never
  cut into a different version — plus the invariant behind them, that no
  rendered line exceeds the width it was given.

## Keeping the suites honest

Both are checked by mutation, not just by running green: break the behaviour in
`reactor-core` or in an extension, confirm the intended test is the one that
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
- **Upstream skill fetching over the network.** `reactor setup` and
  `reactor skills fetch` are tested for what they do around a fetch (config
  seeding, never clobbering, completions), not against real upstream
  repositories.
