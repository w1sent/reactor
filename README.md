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

**REactor is migrating off pi.** The harness is being rewritten in Rust over
[rig](https://rig.rs/), with `reactor-gui` as its frontend; pi becomes one
client of the `reactor` CLI rather than the host. [`MIGRATE.md`](MIGRATE.md)
is the plan — what the end state is, what survives, and the order it happens
in — and [ADR-0033](docs/adr/0033-reactor-is-a-rust-project-on-rig.md) through
[ADR-0038](docs/adr/0038-settings-resolve-global-then-session.md) are the
decisions. Everything described below is what runs today, and keeps running:
the pi package is not going away, it is narrowing to the toolbox.

## The idea in one screen

```
                    ~/.reactor/tools.toml            (the catalogue)
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

Around that spine, eight extensions exist to make long sessions usable —
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
tools.toml       Shipped tool catalogue — seeds ~/.reactor/tools.toml
toolsets.toml    Shipped toolset definitions — seeds the user's copy
crates/          Rust: `reactor-core` (catalogue, probes, activation, install,
                 skills — files and subprocesses only) and `reactor-cli`, the
                 `reactor` binary over it (ADR-0034)
gui/             reactor-gui, the native frontend (Rust, gpui-kit) — spec in
                 gui/SPEC.md, its own cargo workspace until it links
                 reactor-core (MIGRATE.md phase 2); the frontend the Rust
                 harness is being built behind
extensions/      pi extensions (TypeScript): tool-registry, selector, status,
                 scenario, goal-setting, history-tools, rolling-context,
                 auto-continue, identity
skills/          Skills REactor authors itself — only where --help is insufficient
prompts/         Prompt templates, including multi-step analysis scenarios
themes/          pi themes
                 ^ all three are discovered by pi from their directory name;
                   docs/package-resources.md covers them, and no doc may live
                   inside them (pi would register it as a resource)
scripts/         repo-management scripts (Python is fine here — ADR-0039)
tests/           cargo test for the CLI and core (crates/*/tests), node --test
                 for the extensions
docs/            Concept, ADRs, HOWTOs, reference notes
```

Standalone tools REactor builds are **not** in this repo. They are sibling
repositories with their own release cycles, referenced by the catalogue. See
[ADR-0002](docs/adr/0002-package-ships-assets-tools-are-sibling-repos.md).

## Install

```bash
pi install git:github.com/<you>/reactor   # assets: extensions, skills, prompts
cargo install --git https://github.com/w1sent/reactor reactor-cli   # the `reactor` binary, onto ~/.cargo/bin
reactor setup                             # seed ~/.reactor/, fetch skills, completions
reactor doctor                            # what is missing, and how to get it
reactor install yara                      # opt-in, does it for you
```

Two steps because `pi install` never builds anything, never initialises
submodules, and runs `git clean -fdx` inside the package on every update — so
the binary, the seeded config and the fetched upstream skills have to live
outside pi's package tree. See
[ADR-0002](docs/adr/0002-package-ships-assets-tools-are-sibling-repos.md).

Already cloned? `cargo install --path crates/reactor-cli` does the same from
the checkout. Either way it needs a Rust toolchain ([rustup.rs](https://rustup.rs))
to build, and nothing to run: the binary is static, the
shipped catalogue is compiled into it, and no Python is involved in installing
or running REactor ([ADR-0039](docs/adr/0039-the-executables-contain-no-python.md)).
`reactor setup` is idempotent and never clobbers your `tools.toml`; re-running
it is the way to update.

```bash
cargo test                                # core + CLI, 151 tests
cargo build && node --test "tests/extensions/*.test.mjs"   # pi flavor; needs node and pi
```

The extension half needs pi on `PATH` and skips without it: it loads each
extension through pi's own loader, so the module graph under test is the one pi
runs ([ADR-0012](docs/adr/0012-extensions-tested-through-pi-s-own-loader.md)).

## Scope

REactor was built as a pi package and targeted pi alone
([ADR-0001](docs/adr/0001-pi-is-the-only-target-harness.md)). That is being
reversed ([ADR-0033](docs/adr/0033-reactor-is-a-rust-project-on-rig.md)), along
the line ADR-0001 itself drew: the tool repositories are reused unchanged, and
a new flavor of REactor is built against a harness it owns.

The line the migration settles on
([ADR-0035](docs/adr/0035-portable-surface-is-machine-facts.md)) is **the
toolbox, not the workflow**. What any harness gets, through the `reactor` CLI,
is machine facts — what RE tooling exists here, whether it is installed,
whether its services are up, how to install what is missing, and which subset
to advertise. What stays REactor's own is the session half: the manifest,
identity, scenarios, context management and the session tree, which need a GUI
to be worth using and a loop REactor controls to be buildable at all.

For pi specifically that means `tool-registry`, `selector`, `status` and
`scenario` stay maintained against the CLI, and the seven workflow extensions
are frozen at what they do today — still working, still tested, no longer
developed.

## Status

REactor is in **alpha**, on the road to v1.0. The design is settled —
`docs/adr/` records every decision — and everything below is built and tested.
What moves any of it forward is real use, not another round of building;
deferred ideas are deliberately not tracked in these files (the reasoning
lives in git history and the ADRs).

The spine works end to end: the catalogue, the `reactor` CLI, `reactor setup`
(seeds `~/.reactor/`, fetches upstream skills, installs completions), the toolsets and scenarios, and the four RE-focused extensions —
tool-registry, selector, status, scenario.

Eight general-purpose extensions ship alongside them, switched independently of
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
  ([ADR-0023](docs/adr/0023-reporting-enforcement-is-a-filesystem-probe-not-a-heuristic.md));
- `guide`, a popup for the person at the keyboard: the concept and the flows,
  one `/guide` away.

`cargo test` and the extension suite cover all of it: 151 tests on the CLI and core, 281 driving the extensions
against pi's own loader.
