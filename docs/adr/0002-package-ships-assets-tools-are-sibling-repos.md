# The pi package ships assets only; standalone tools are sibling repos

The REactor pi package contributes only things that need no build step:
extensions (`.ts`, loaded directly by pi's `jiti`), skills, prompt templates and
themes, plus a stdlib-only Python CLI. Standalone tools that REactor develops
live in their own repositories with their own release cycles and are referenced
by the catalogue, never vendored, never submoduled.

## Why: pi is not a build system

Read out of pi 0.83.0's `dist/core/package-manager.js`, `pi install git:<repo>`
does exactly three things:

1. `git clone <repo> <targetDir>` — **without `--recurse-submodules`**; the
   string `submodule` does not occur anywhere in that file
2. `git checkout <ref>`, if a ref was given
3. `npm install --omit=dev`, if the clone contains a `package.json`

and the update path (`ensureGitRef`) does:

1. `git fetch`
2. compare `HEAD` to the target commit, returning early if equal
3. `git reset --hard`
4. **`git clean -fdx`** — the source comment reads "Clean untracked files
   (extensions should be pristine)"
5. `npm install --omit=dev`

Three things follow directly. Submodules under a `tools/` directory would arrive
empty and stay empty, because nothing ever initialises them. Any build output
produced in-tree is untracked, so `git clean -fdx` destroys it on every update.
And a compiled tool has no way to get built at all, because pi never invokes a
build.

The one hook that does fire is `npm install`'s lifecycle scripts — pi does not
pass `--ignore-scripts` on package installs (that flag appears only in
`dist/config.js`, on pi's self-update commands), so a `prepare` or `postinstall`
in REactor's `package.json` would run. That was considered and rejected below.

## The shape that results

```
<workspace>/
├── reactor/          this repo — the pi package + the reactor CLI
├── <tool-a>/         sibling repo, own releases
└── <tool-b>/         sibling repo, own releases
```

The catalogue ([ADR-0003](0003-tools-toml-single-source-of-truth.md)) carries an
entry per tool with its detection probe and per-platform install recipes.
`reactor doctor` reports what is missing and how to get it on the platform it is
running on; `reactor install <tool>` and `scripts/install.py` do it, opt-in.
Third-party dependencies — ripgrep, jadx, frida — go through exactly the same
mechanism as tools REactor writes itself. There is one dependency story, not two.

This also means `pi install git:.../reactor` is fast and network-cheap: a clone
and an `npm install`, no toolchain required, no multi-minute compile blocking
pi's startup.

## Consequences

- REactor's own installation is two steps, not one: `pi install` for the assets,
  `scripts/install.py` for the CLI and the seeded config. Documented as such.
- A user can have REactor's assets without any of the tools, and the registry
  will honestly report an almost-empty toolbox. That is the correct behaviour,
  not a failure mode.
- Tools version independently of REactor. A tool fix does not need a REactor
  release, and vice versa.
- Nothing REactor ships requires `cargo`, a C toolchain, or a Python build
  environment to install.

## Considered and rejected

- **Submodules under `tools/`, built by a `prepare` script** — rejected. It
  works mechanically (npm lifecycle scripts do run) but puts a multi-minute
  `cargo build` in front of every `pi install` *and every update*, requires a
  Rust toolchain on any machine that installs the assets, and rebuilds from
  scratch each time because `git clean -fdx` deletes `target/` first. The
  failure mode — pi hanging on startup because a compile is running — is bad.
- **Submodules for development only, not for install** — rejected as clutter.
  Sibling repositories in the same parent directory give the same working-copy
  convenience with no ambiguity about whether the submodule is load-bearing.
- **Vendoring prebuilt binaries in the repo, or fetching them into
  `~/.pi/agent/bin/` the way pi does for `fd`/`rg`** — rejected for now. It
  requires owning a cross-platform release matrix and binary provenance for
  every tool before a single tool exists. Revisit if a tool turns out to be both
  widely wanted and painful to build.
- **Deferring standalone tools entirely** — rejected. The constraint needed
  settling before the layout was written, not after.
