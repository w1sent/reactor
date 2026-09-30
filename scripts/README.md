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
reactor setup                             # seed config, skills, completions
reactor setup --no-skills --no-completions --dry-run   # the switches
```

`reactor setup` does the rest:

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
is not part of it: `cargo install --path crates/reactor-gui`.

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

Deliberately **not** part of `cargo test`: the test suite is offline and runs in
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
