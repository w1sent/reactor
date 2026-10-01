# REactor

A reverse-engineering agent harness, written in Rust over [rig](https://rig.rs/).
It turns an LLM into an RE workstation the agent actually understands: a
catalogue of the tools that exist on this machine, an agent loop with context
management built for long analysis sessions, and a native GUI (`reactor-gui`)
to drive it.

REactor does **not** document the tools it exposes. `<tool> --help` and
`man <tool>` are the documentation, and where a tool's author ships their own
Agent Skill, REactor fetches theirs rather than writing a worse one. What an
agent genuinely cannot discover on its own is *which tools exist on this machine,
whether they are installed, and whether the ones that are services are actually
running* — that gap is what REactor fills.

See `CONTEXT.md` for the project glossary, `docs/concept.md` for the idea in
full, `docs/adr/` for the decisions behind it, and [`MIGRATE.md`](MIGRATE.md)
for how it got here.

**REactor used to be a pi package.** That flavor (TypeScript extensions
hosted in [pi](https://pi.dev)) was retired in favour of the Rust harness and
removed from the tree; it lives in git history. The last commit that has it is
`014a9b8` — check that out for the pi version
([ADR-0033](docs/adr/0033-reactor-is-a-rust-project-on-rig.md),
[ADR-0035](docs/adr/0035-portable-surface-is-machine-facts.md)).

## The idea in one screen

```
                    ~/.reactor/tools.toml            (the catalogue)
                              │
              ┌───────────┴───────────┐
              ▼                       ▼
        reactor CLI              reactor-agent
       doctor/tools              probes and injects
       services                        │
              │                        ▼
              │      ## Available RE tools (this machine)
              │      bn         reverse engineering framework  [BN session: up]
              │      frida      dynamic instrumentation        17.9.7
              │      jadx       decompile Android DEX/APK      1.5.6
              │      adb        Android device bridge          [adb: 2 devices]
              │
              │      `<tool> --help` is the documentation.
              ▼
        $ reactor doctor
          missing (10)
            yara         sudo pacman -S yara
            ipsw         (manual) https://github.com/blacktop/ipsw/releases
```

The agent is told what is *here*, in one compact block, and nothing more. It
reads `--help` when it needs to know how something works.

Around that spine, the agent loop manages long sessions — a session store you
can branch and resume, a context budget that reduces old messages before the
window fills (with preview and undo), a session manifest, personas and a
reporting discipline. Where a setting exists it resolves built-in → global →
session ([ADR-0038](docs/adr/0038-settings-resolve-global-then-session.md)).

## Layout

```
tools.toml       Shipped tool catalogue — seeds ~/.reactor/tools.toml
toolsets.toml    Shipped toolset definitions — seeds the user's copy
crates/
  reactor-core     catalogue, probes, activation, install, skills — files and
                   subprocesses only
  reactor-cli      the `reactor` binary over it (ADR-0034)
  reactor-client   the seam the GUI panels use: in-process, or the CLI
  reactor-context  manifest / identity / reporting / scenario state machines
                   (ADR-0040)
  reactor-agent    session store, the loop over rig, the context budget
                   manager (ADR-0041); `examples/repl.rs` is a dev REPL
  reactor-gui      the native frontend (gpui-kit); SPEC.md is its spec
skills/          Skills REactor authors itself — only where --help is insufficient
prompts/         Multi-step analysis scenarios
scripts/         repo-management scripts (Python is fine here — ADR-0039)
docs/            Concept, ADRs, HOWTOs
```

Standalone tools REactor builds are **not** in this repo. They are sibling
repositories with their own release cycles, referenced by the catalogue. See
[ADR-0002](docs/adr/0002-package-ships-assets-tools-are-sibling-repos.md).

## Install

```bash
cargo install --git https://github.com/w1sent/reactor reactor-cli   # the `reactor` binary
cargo install --git https://github.com/w1sent/reactor reactor-gui   # the GUI
reactor setup                             # seed ~/.reactor/, fetch skills, completions
reactor doctor                            # what is missing, and how to get it
reactor install yara                      # opt-in, does it for you
```

Already cloned? `cargo install --path crates/reactor-cli` (and `crates/reactor-gui`)
does the same from the checkout. It needs a Rust toolchain
([rustup.rs](https://rustup.rs)) to build, and nothing to run: the shipped
catalogue is compiled into the binary, and no Python is involved in installing or
running REactor ([ADR-0039](docs/adr/0039-the-executables-contain-no-python.md)).
`reactor setup` is idempotent and never clobbers your `tools.toml`; re-running
it is the way to update. On Linux it also installs the GUI's desktop launcher
and icon. The GUI links a native windowing stack (fontconfig,
xkbcommon, Vulkan/Wayland/X11); see `crates/reactor-gui/README.md`.

Point the GUI at a model with `settings.models` in `~/.reactor/settings.json`
or `/model provider/name` (anthropic, openai, gemini, openrouter, ollama).

```bash
cargo test                                # core, CLI, context, agent, client
cargo build -p reactor-gui                # the GUI is opt-in: not a default member
```

## Status

REactor is in **alpha**, on the road to v1.0. The design is settled —
`docs/adr/` records every decision. What moves any of it forward is real use,
not another round of building; deferred ideas are deliberately not tracked in
these files (the reasoning lives in git history and the ADRs).
