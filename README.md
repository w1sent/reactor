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
full, `docs/adr/` for the decisions behind it, and `extensions/README.md` for
the extension inventory — what each one does, its switches, its default state.

## The idea in one screen

```
                    ~/.pi/reactor/tools.toml         (the catalogue)
                              │
              ┌───────────┬───┴───────┬───────────┐
              ▼           ▼           ▼           ▼
        reactor CLI  tool-registry  selector    status
       doctor/tools   probes and    search and  what is up
       services       injects       toggles     right now
              │           │
              │           ▼
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

Around that spine, seven extensions exist to make long sessions usable —
memory that survives compaction, a working persona, and hands-off operation:

```
        goal-setting     the session manifest: goal, steps, guidelines
        identity         the working persona, one `/identity` away
        rolling-context  fades old messages instead of summarizing them
        auto-continue    resumes the agent after compaction -- no "continue" typing
        history-tools    line-addressed recovery over the session history
        context-editor   hand-curates what the model sees: fork or filter
        reporting        documents as it goes, escalating when nothing lands
```

None of them touch the catalogue; each is switched independently, and
`extensions/README.md` has the full inventory — what each one does, its
switches, its default state, and the reasoning behind it.

## Layout

```
tools.toml       Shipped tool catalogue — seeds ~/.pi/reactor/tools.toml
toolsets.toml    Shipped toolset definitions — seeds the user's copy
bin/             The `reactor` CLI (stdlib-only Python, symlinked onto PATH)
extensions/      pi extensions (TypeScript): tool-registry, selector, status,
                 scenario, goal-setting, history-tools, rolling-context,
                 auto-continue, identity
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
npm test                                  # both suites, nothing to install
python3 tests/test_reactor.py             # the CLI alone, 73 tests
node --test "tests/extensions/*.test.mjs" # the extensions alone, 123 tests
```

The extension half needs pi on `PATH` and skips without it: it loads each
extension through pi's own loader, so the module graph under test is the one pi
runs ([ADR-0012](docs/adr/0012-extensions-tested-through-pi-s-own-loader.md)).

## Scope

REactor targets pi and only pi. It is not a portable skill collection; if
another harness becomes interesting, the tool repositories are reused and a new
flavor of REactor is built for it. See
[ADR-0001](docs/adr/0001-pi-is-the-only-target-harness.md).

## Status

REactor is in **alpha**, on the road to v1.0. The design is settled —
`docs/adr/` records every decision — and everything below is built and tested.
What moves any of it forward is real use, not another round of building;
deferred ideas are deliberately not tracked in these files (the reasoning
lives in git history and the ADRs).

The spine works end to end: the catalogue, the `reactor` CLI, the installer
(symlinks the CLI, seeds `~/.pi/reactor/`, fetches upstream skills, installs
completions), the toolsets and scenarios, and the four RE-focused extensions —
tool-registry, selector, status, scenario.

Seven general-purpose extensions ship alongside them, switched independently of
the catalogue:

- `rolling-context`, the fade — an alternative to pi's own compaction
  ([ADR-0019](docs/adr/0019-rolling-context-ships-here-general-purpose.md),
  [ADR-0020](docs/adr/0020-rolling-context-measures-and-cuts-like-pi-does.md)),
  split into `goal-setting` (the session manifest, in the system prompt) and
  `history-tools` (line-addressed recovery over the session file)
  ([ADR-0024](docs/adr/0024-rolling-context-splits-into-goal-setting-history-tools-and-the-fade.md));
- `auto-continue`, which keeps the agent going after an automatic compaction
  ends its turn
  ([ADR-0025](docs/adr/0025-auto-continue-continues-after-automatic-compaction.md));
- `identity`, a persona in the system prompt with built-ins for the scenarios
  a security professional moves between
  ([ADR-0026](docs/adr/0026-identity-is-a-persona-block-in-the-system-prompt.md));
- `context-editor`
  ([ADR-0021](docs/adr/0021-context-editor-forks-or-filters-never-rewrites.md))
  and `reporting`
  ([ADR-0023](docs/adr/0023-reporting-enforcement-is-a-filesystem-probe-not-a-heuristic.md)).

`npm test` covers all of it: 81 tests on the CLI, 217 driving the extensions
against pi's own loader.
