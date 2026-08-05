# REactor

A reverse-engineering agent harness built on [pi](https://pi.dev). REactor is a
pi package: it contributes extensions, skills, prompt templates and themes, plus
a `reactor` CLI, that together turn a stock pi install into an RE workstation the
agent actually understands.

REactor does **not** document the tools it exposes. `<tool> --help` and
`man <tool>` are the documentation, and where a tool's author ships their own
Agent Skill, REactor fetches theirs rather than writing a worse one. What an
agent genuinely cannot discover on its own is *which tools exist on this machine,
whether they are installed, and whether the ones that are services are actually
running* — that gap is what REactor fills.

See `CONTEXT.md` for the project glossary, `docs/concept.md` for the idea in
full, `docs/adr/` for the decisions behind it, and `TODO.md` for the plan.

> **Status: Milestone 1 (the spine) is built, and the selector with it.** The
> `reactor` CLI, the installer, the catalogue, the `tool-registry` extension and
> `/reactor-tools` work end to end, and `npm test` covers all of it — 56 tests
> on the CLI, 42 driving the extensions against it. The status panel (the rest
> of Milestone 2) and scenarios (Milestone 3) are not started. See `TODO.md`.

## The idea in one screen

```
                    ~/.pi/reactor/tools.toml         (the catalogue)
                              │
              ┌───────────────┼────────────────┐
              ▼               ▼                ▼
        reactor CLI     tool-registry ext   selector ext
      doctor / tools      probes, injects    search / inspect
      skills / install    into system prompt   toggle toolsets
              │               │
              │               ▼
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

## Layout

```
tools.toml       Shipped tool catalogue — seeds ~/.pi/reactor/tools.toml
toolsets.toml    Shipped toolset definitions — seeds the user's copy
bin/             The `reactor` CLI (stdlib-only Python, symlinked onto PATH)
extensions/      pi extensions (TypeScript): tool-registry, selector, status
skills/          Skills REactor authors itself — only where --help is insufficient
prompts/         Prompt templates, including multi-step analysis scenarios
themes/          pi themes
                 ^ all three are discovered by pi from their directory name;
                   docs/package-resources.md covers them, and no doc may live
                   inside them (pi would register it as a resource)
scripts/         install.py and repo-management scripts
tests/           stdlib unittest for the CLI, node --test for the extensions
docs/            Concept, ADRs, HOWTOs, reference notes
```

Standalone tools REactor builds are **not** in this repo. They are sibling
repositories with their own release cycles, referenced by the catalogue. See
[ADR-0002](docs/adr/0002-package-ships-assets-tools-are-sibling-repos.md).

## Install

```bash
pi install git:github.com/<you>/reactor   # assets: extensions, skills, prompts
python3 scripts/install.py                # CLI onto PATH, seed ~/.pi/reactor/
reactor doctor                            # what is missing, and how to get it
reactor install yara                      # opt-in, does it for you
```

Two steps because `pi install` never builds anything, never initialises
submodules, and runs `git clean -fdx` inside the package on every update — so
the CLI symlink, the seeded config and the fetched upstream skills have to live
outside pi's package tree. See
[ADR-0002](docs/adr/0002-package-ships-assets-tools-are-sibling-repos.md).

Requires Python 3.11+ (`tomllib`). Nothing else — the CLI imports no
third-party package by design.

```bash
npm test                                  # 98 tests, nothing to install
python3 tests/test_reactor.py             # the CLI alone, 56 tests
```

The extension half needs pi on `PATH` and skips without it: it loads each
extension through pi's own loader, so the module graph under test is the one pi
runs ([ADR-0012](docs/adr/0012-extensions-tested-through-pi-s-own-loader.md)).

## Scope

REactor targets pi and only pi. It is not a portable skill collection; if
another harness becomes interesting, the tool repositories are reused and a new
flavor of REactor is built for it. See
[ADR-0001](docs/adr/0001-pi-is-the-only-target-harness.md).
