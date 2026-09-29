# scripts/

Repo-management scripts.

## Installing REactor — `reactor setup`, not a script

There is no installer script. `scripts/install.py` was retired with the Python
CLI ([ADR-0039](../docs/adr/0039-the-executables-contain-no-python.md)): an
installer that needs Python would put back the interpreter the Rust binary
removed. What it did is now two commands:

```bash
cargo install --git https://github.com/w1sent/reactor reactor-cli   # the binary, onto ~/.cargo/bin
cargo install --path crates/reactor-cli   # ...or from a checkout you already have
reactor setup                             # everything that is not a pi resource
reactor setup --no-skills --no-completions --dry-run   # the switches
```

`pi install git:…/reactor` gets the assets; `reactor setup` gets everything else:

1. Seed `~/.reactor/tools.toml` and `toolsets.toml` from the copies compiled into
   the binary — **only if absent**. Never clobbers; if they exist and differ, it
   says so and points at `reactor diff-config`
   ([ADR-0004](../docs/adr/0004-config-updates-via-plain-diff.md)).
2. Create `~/.reactor/state.json` if absent.
3. Fetch configured upstream skills into `~/.reactor/skills/<tool>/`, pinned to
   the ref in the catalogue, recording source, ref, resolved commit and fetch
   time in `.reactor-skill.json`
   ([ADR-0008](../docs/adr/0008-aggregate-upstream-skills.md)).
4. Write bash, zsh, and fish completion scripts to each shell's conventional
   per-user completions directory
   ([ADR-0015](../docs/adr/0015-shell-completion-generated-not-hand-written.md)).
   Unlike the config files these are reactor's own output, so they are
   overwritten unconditionally. zsh needs one manual step — `~/.zfunc` on `fpath`
   before `compinit` — and setup reminds you every run.
5. Report. Warn **only** where a configured skill could not be fetched — a tool
   with no configured skill is the normal case and gets no warning.

Idempotent; re-running is the supported way to update. Building `reactor-gui`
is no longer part of it: `cargo install --path gui/crates/reactor-gui` until the
GUI joins the root workspace (MIGRATE.md phase 2).

**Still here, until phase 1's gate is green** — `install.py` and `parity.py`,
scaffolding rather than tools. `python3 scripts/parity.py [path/to/reactor]`
runs ~150 commands through the Rust binary and the Python original against
identical fixture directories and diffs stdout and exit codes byte for byte,
then checks that each can read the other's `cache.json`. Delete both, with
`bin/reactor`, when the extension suite is green.

## `verify-recipes.py`

Checks every `[tool.*.install]` recipe in the catalogue against its package
manager's real index, and exits non-zero if any recipe names a package that does
not exist.

```bash
scripts/verify-recipes.py                # the shipped tools.toml
scripts/verify-recipes.py ~/.reactor/tools.toml
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

Two failure modes to keep in mind when adding a checker. It must be able to say
no — `packages.debian.org` returns **200 with a "No such package" page**, so the
Debian check reads the body, and for a while it did not and passed everything.
And it checks that the package **exists**, not that it provides the binary the
tool's `detect` looks for. Those names diverge often — Debian's `aapt` ships
`aapt2`, Arch's `android-tools` ships `adb`, `wireshark-cli` ships `tshark` — so
a recipe can name a real package that installs the wrong thing. A clean run is
not a substitute for someone who knows the tool.

This is authoring-time verification, which is a different thing from the
runtime package-availability probing ADR-0010 rejects. The cost that made
probing wrong — a network round trip per candidate per tool, on a user's
machine, to refine a recommendation they are about to read — is not a cost here,
because it is paid once by whoever edits the catalogue.

## `check-in-pi.mjs`

Drives a **real** `pi --mode rpc` process, with every REactor extension loaded
the way pi actually loads them, over the documented RPC protocol
(`docs/rpc.md`), and watches for `extension_error` events.

```bash
scripts/check-in-pi.mjs                                   # the default smoke set
scripts/check-in-pi.mjs "/reactor-status mute adb" "/reactor-status"
```

Exists because `tests/extensions/*.test.mjs` — for all that it runs through
pi's own *loader* against the real CLI
([ADR-0012](../docs/adr/0012-extensions-tested-through-pi-s-own-loader.md)) —
is still a mock host underneath: `ctx` and `pi` are fakes this repo
maintains. `/reactor-toolbox off` shipped with a real bug the mocked `ctx`
could not have caught until `harness.mjs` grew a `guard` simulating it after
the fact — pi invalidates a captured `ctx`/`pi` the instant `await
ctx.reload()` resolves, and the handler used `ctx` again right after
(`docs/pi-api-notes.md`). This script is the check that needs no simulation:
a `/name` prompt over RPC runs the real extension command directly (no LLM
call, no API key needed), and a thrown error surfaces as `extension_error` on
stdout instead of being caught by an assertion that has to already know to
look for it.

Every run gets a fresh, throwaway `PI_CODING_AGENT_DIR`, `REACTOR_CONFIG_DIR`
(empty, so `reactor` falls back to its compiled-in
`tools.toml`/`toolsets.toml`) and cwd — isolated from whatever is on the
machine actually running it, and cleaned up after.

Each command is sent and its own RPC `response` awaited (matched by `id`)
before the next one goes out — not a fixed delay. This matters beyond
pacing: `ctx.reload()` invalidates a whole extension instance, not just the
handler that called it, so two commands from the *same* extension file
genuinely in flight at once can race a reload from one against the other's
still-suspended `await` — a real way to hit the same `extension_error`, but a
different bug from the ordering-within-one-handler kind this script was
written for (`docs/pi-api-notes.md`). Waiting for each response is what real
usage already does by construction, so it is what this script does too.

Deliberately **not** part of `tests/`, for the same reason as
`verify-recipes.py`: it spawns a real process (~2–3 s for the default set)
and needs `pi` on `PATH`, not stdlib-offline-in-a-second. Run it after
touching anything that calls `ctx.reload()`, `ctx.newSession()`,
`ctx.fork()`, or `ctx.switchSession()` — the class of bug it exists to catch.

## Why installation is two steps

pi never builds anything, never initialises submodules, and runs
`git clean -fdx` inside the installed package on every update
([ADR-0002](../docs/adr/0002-package-ships-assets-tools-are-sibling-repos.md)).
So the binary, the seeded config and the fetched skills all have to happen
outside pi's package tree, by something pi does not manage.

A `postinstall` hook in `package.json` *would* fire — pi does not pass
`--ignore-scripts` on package installs — and was rejected: it would run on every
install and every update, and would put network fetches and filesystem writes in
front of pi's startup.
