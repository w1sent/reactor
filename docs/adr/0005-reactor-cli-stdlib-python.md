# One `reactor` CLI, stdlib-only Python, and extensions shell out to it

REactor ships a single command, `reactor`, symlinked onto the user's `PATH`. It
is Python with no third-party dependencies, following the same reasoning
[ADR-0038 in the plugins repo](https://github.com/w1sent/bn-plugins/blob/main/docs/adr/0038-binja-cli-skill-frontend.md)
applied to `bn`. Every subcommand supports `--format json`.

```
reactor doctor                  what is present, what is missing, how to get it
reactor tools list|show         catalogue queries
reactor skills list|show        skills REactor knows about, incl. fetched ones
reactor install <tool>...       opt-in install, platform-appropriate
reactor diff-config             diff shipped vs installed config
reactor overwrite-config        force-replace installed config
```

## Why a CLI at all

pi's philosophy is CLI tools over MCP servers, because a CLI's stdout can be
filtered by `jq`, `rg` or a shell pipeline *before* it becomes a tool result —
the unfiltered data never reaches the model's context, whereas an MCP tool call
has no shell stage between return value and context. REactor holds the same
position and applies it to itself.

A CLI is also the only form that is simultaneously usable by the user at a
prompt, by a skill as a shell command, by a script, and by an extension. A pi
slash command would be none of those but the last.

## Why the extensions shell out instead of importing

The tool-registry, selector and status extensions are TypeScript running inside
pi, and could read `tools.toml` directly with a TOML parser dependency. They do
not. They invoke `reactor … --format json` and parse that.

This keeps exactly one implementation of catalogue semantics — resolution order,
activation state, probe execution, platform detection, install-recipe selection.
The TUI can be wrong about how it renders something, but it cannot be wrong
about what the catalogue says, because it does not know. The subprocess cost is
one process spawn per refresh, against operations already measured in
LLM-turn latency.

## Why Python, and why stdlib-only

Python for rapid iteration while the tool surface is still moving, and because
the ecosystem REactor coordinates is largely Python already — LIEF, angr, z3,
scapy, frida, tree-sitter all expose Python APIs, so an analysis script and the
CLI share a language.

Stdlib-only because the job is small: argument parsing, `tomllib`, `shutil.which`,
`subprocess`, `json`, and platform detection. Adding a dependency would mean the
tool whose purpose is to tell you what is missing could itself be missing.
`tomllib` requires Python 3.11, which is the recorded floor.

`bn`'s measured numbers apply here too: bare `python3` startup is ~11–17 ms, and
~35–40 ms with `argparse` + `json` + `http.client` loaded. `reactor` is invoked
interactively or once per registry refresh, never in a hot loop, so that is
irrelevant either way.

## Consequences

- `reactor` is a second command on `PATH` alongside `bn`. Deliberate: they are
  different things — `bn` drives one tool, `reactor` describes the toolbox.
- The CLI does not require pi to be running, and does not import anything from
  pi. It reads `~/.pi/reactor/` because that is where its config lives
  ([ADR-0003](0003-tools-toml-single-source-of-truth.md)), not because it is
  coupled to the harness.
- `--format json` is not optional on any subcommand; it is the extensions' only
  interface and must stay stable. Its shape is part of REactor's contract in a
  way the human-readable output is not.
- Installation puts the symlink in place; `scripts/install.py` owns that, with
  `--cli-dest` defaulting to `~/.local/bin/reactor`, mirroring how `bn` installs.

## Considered and rejected

- **Loose scripts (`scripts/doctor.py`, `scripts/install.py`) with no unified
  command** — rejected. No stable name for skills, docs or extensions to
  reference, and every reference becomes an install-location-dependent path.
- **Rust** — rejected for now on the same grounds ADR-0038 rejected it for `bn`,
  plus a bootstrap paradox specific to this tool: the program whose job is to
  tell you what you are missing and install it would itself need a toolchain or
  a release download first. Startup time, the usual argument for Rust here, is
  irrelevant for an interactive command.
- **A pi extension/slash command instead of a CLI** — rejected. Unavailable to
  skills as a shell command, to scripts, and to the user outside pi, and it
  would put catalogue logic in TypeScript where the installer cannot reach it.
- **Extensions importing a TOML parser and reading the catalogue directly** —
  rejected; two implementations of catalogue semantics that must be kept in
  sync, for the sake of avoiding one subprocess spawn.
