# reactor-gui lives in this repo, and installs with it

> **Amended by [ADR-0039](0039-the-executables-contain-no-python.md):** the GUI
> still lives here and still degrades gracefully, but `scripts/install.py` is
> gone. Until the GUI joins the root cargo workspace (MIGRATE.md phase 2) it is
> built and installed with `cargo install --path gui/crates/reactor-gui`, and
> `reactor setup` does not build anything.

The GUI is a Rust workspace in `gui/` inside this repository — not a sibling
repo like the catalogue's tools ([ADR-0002](0002-package-ships-assets-tools-are-sibling-repos.md)) —
and `scripts/install.py` builds it by default, warning and continuing when
Rust is missing. Its specification is [`gui/SPEC.md`](../../gui/SPEC.md).

## Why

**The GUI and the extensions are one contract, so they are one checkout.**
The GUI's whole reason to exist is rendering what reactor's extensions want
shown ([ADR-0032](0032-the-gui-extends-pis-rpc-through-existing-channels-only.md)):
the selector and guide gain a GUI-mode branch, a stateless `guiview` helper
joins `extensions/lib/`, and a new `gui-bridge` extension ships. A contract
that spans an extension change and a client change wants them in the same
commit, the same review, the same test run — splitting them across repos
re-couples them through release timing, which is the failure ADR-0002's
boundary exists to avoid, not one it can absorb.

**Checking out this repo yields a working thing.** Install runs the CLI, the
extensions load through pi, and now also `reactor-gui` builds from the same
tree. Nothing the user has to assemble from two checkouts at two versions
that may not agree.

**The GUI is a frontend, not a standalone tool.** ADR-0002's sibling repos
are for things the *agent invokes* — their own release cycles because the
catalogue references them at a version. The GUI references nothing the
catalogue can pin; it is coupled to `extensions/`, which lives here.

## What it costs

- The repo gains a Rust toolchain surface: `gui/Cargo.toml`, a pinned
  gpui-kit dependency, `cargo` as an optional component of install. The
  python/node suites do not depend on it; the test suites are untouched, and the
  GUI's tests are hermetic (`cargo test`, fake JSONL peer — live-pi tests
  are `#[ignore]`d).
- `pi install` runs `git clean -fdx` inside the package on update, which
  wipes `gui/target/`. That is correct behaviour, not a problem: the build
  is local and reproducible, and `install.py` rebuilds from source anyway.
- Missing Rust degrades gracefully by design: install completes with the
  CLI and extensions, minus the GUI, and says so once, clearly.

## Considered and rejected

- **A sibling repo (`reactor-gui`), per ADR-0002's pattern.** Rejected: it
  puts the two halves of the extension-UI contract behind two release
  cycles, and a checkout no longer equals a working REactor. ADR-0002 was
  about *tools* the catalogue pins; the GUI pins nothing.
- **In-repo, installed as a separate artifact (npm/cargo published).** More
  moving parts for no coupling benefit; `install.py` already owns "put
  binaries on PATH from this tree".
- **`reactor gui` as a python subcommand.** The CLI is deliberately
  stdlib-only Python; a GUI is neither stdlib nor python. The CLI may gain
  an exec-style sugar that runs `reactor-gui` *if found on PATH* — nothing
  more.