# Migrating REactor from pi to rig

REactor becomes a Rust project. The harness is rewritten over
[rig](https://rig.rs/), `reactor-gui` becomes the frontend rather than a
second one, and pi stops being the thing REactor is built inside — it becomes
one of several harnesses that can consume the `reactor` CLI from outside.

The decisions are [ADR-0033](docs/adr/0033-reactor-is-a-rust-project-on-rig.md)
through [ADR-0038](docs/adr/0038-settings-resolve-global-then-session.md).
This file is the plan they add up to: what the end state is, what survives the
move, what is deliberately not built, and the order in which it happens.

## Why, in one screen

REactor's four hardest behaviours are all fights with a host that owns the
message list:

```
  the fade        cancels pi's own threshold compaction, because two
                  mechanisms that cannot coordinate contradict           ADR-0020
  context-editor  may never rewrite history, because pi owns the file    ADR-0021
  auto-continue   220 lines about which of pi's three compaction cases
                  leave a turn parked                                    ADR-0025
  the GUI         smuggles structured UI through line 0 of a string
                  widget, because the RPC protocol carries nothing else  ADR-0032
```

None of those shrink over time, and ADR-0032 already records that there is no
upstream path for the protocol changes wanted. Owning the loop deletes all
four rather than managing them.

The other half of the argument is that **the features worth building next
cannot be expressed in a terminal** — curating what the model sees, previewing
what a reduction is about to discard, restoring a summarized range, walking
the session tree. That is why the GUI exists (ADR-0031), and it is why a TUI
is out of scope here: other projects build good ones, and REactor does not
need to be one of them.

## The end state

```
                       ~/.reactor/            (catalogue, settings, sessions)
                              │
        ┌─────────────────────┴─────────────────────┐
        ▼                                            ▼
  reactor-core          ────────────────▶      reactor-cli
  catalogue, probes,      no rig, no gpui       the portable contract:
  cache, activation,      files and             `--format json`, machine
  install, skills         subprocesses only     facts only
        │                                            │
        │                                            └──▶ pi, and any other
        ▼                                                 harness that wants
  reactor-context                                         the toolbox
  manifest · identity · scenario · reporting
  state machines + deterministic rendering
        │
        ▼
  reactor-agent   ────────▶  rig  ────────▶  20+ providers
  loop, session store, budget manager, tools
        │
        ▼
  reactor-gui  (gpui-kit, links the crates in-process)
```

Two rules keep the layering honest:

- **`reactor-core` and `reactor-context` never mention rig or gpui.** They do
  files and subprocesses, they are testable without a model, and they are what
  another harness could in principle link.
- **Anything that turns facts into deterministic text lives below the agent.**
  Registry block, manifest block, identity block, phase briefing. The agent
  composes them; it does not author them.

## The decisions

| | Decision | ADR |
|---|---|---|
| Direction | Rust on rig; pi becomes a client, not the host | [0033](docs/adr/0033-reactor-is-a-rust-project-on-rig.md) |
| CLI | Rust library with a thin binary over it; the GUI links it | [0034](docs/adr/0034-reactor-cli-becomes-a-rust-library-with-a-binary.md) |
| Portability | The portable surface is machine facts; no `--session` | [0035](docs/adr/0035-portable-surface-is-machine-facts.md) |
| Sessions | REactor owns its store format; append-only, originals kept | [0036](docs/adr/0036-reactor-owns-its-session-store-format.md) |
| Context | One budget manager, three modes, `auto` by default | [0037](docs/adr/0037-context-reduction-is-one-budget-manager.md) |
| Settings | Global default, then session override | [0038](docs/adr/0038-settings-resolve-global-then-session.md) |

Two things decided by *not* building them:

- **No permission, approval or sandbox model.** ADR-0007's "enforcement would
  be theatre" holds one level up: REactor cannot build a containment boundary
  that survives an agent with bash, so it does not pretend to. The documented
  posture is that an RE agent runs inside a VM or container, and no feature
  may be designed as if containment existed at the harness level.
- **No session sharing between harnesses.** A manifest without its history is
  someone else's notes (ADR-0035).

## What moves where

**Survives untouched.** `tools.toml`, `toolsets.toml`, the probes, the install
recipes, the skills and prompts as content, and the GUI's panels, layout,
theme and views.

**Ported.** `bin/reactor` → `reactor-core` + `reactor-cli`, with the 90 Python
tests as the acceptance gate. The four stateful extensions' logic →
`reactor-context`, as state machines plus rendering, with the UI left behind.

**Built new.** Session store · agent loop over rig · context budget manager ·
settings cascade · skill loading and activation gating · long-running and
high-volume tool I/O.

**Frozen, not ported.** The pi package splits (ADR-0035):

```
  maintained   tool-registry · selector · status · scenario
               already shell out to the CLI; keep working against the
               Rust binary unchanged

  frozen       goal-setting · identity · reporting · rolling-context
               auto-continue · context-editor · history-tools
               keep working as they are; no new features; Rust
               counterparts are not required to match them
```

A pi user keeps everything they have today and gains nothing new in the
workflow half. That is the intended shape, not a regression: the workflow half
needs a GUI to be worth using.

**Retired.** `reactor-rpc` (1,430 lines), the `setWidget` marker contract,
`reactor.json` / `pi-rolling-context.json` / `pi-auto-continue.json` on the
Rust side, and the state root `~/.pi/reactor/` → `~/.reactor/`.

## Invariants

These hold at every step, and a change that breaks one is wrong even if it
passes:

1. **`--format json` is frozen at the byte level during the port.** The four
   maintained extensions consume it, and ADR-0006's prompt-cache stability
   depends on the registry block rendering identically.
2. **The catalogue has exactly one implementation.** ADR-0005's rule outlives
   its language.
3. **Originals survive every context reduction.** Otherwise the history tools
   go blind on precisely the range they exist to reach.
4. **The generated blocks are never reclaimable.** Manifest, identity,
   registry, phase are re-rendered each request, not carried in history.
5. **pi keeps working throughout.** No step may leave the pi flavor broken;
   the CLI contract is what guarantees it.

## Steps

Phases 1–3 each stand alone and leave the project better off even if the
remainder never happens. That is deliberate — the expensive, irreversible work
is in 4, and nothing forces it until 1–3 have proven the boundaries.

### 1 · `reactor-core` + CLI parity  *(reversible)*

Port the Python CLI to Rust as a library with a binary over it. Nothing else
changes: pi runs, the extensions shell out, the GUI still spawns the CLI.

Python may stay in the repo as development tooling and inside skills; it may not
be part of the executables or of installing them
([ADR-0039](docs/adr/0039-the-executables-contain-no-python.md)) — which is why
the installer goes too, not just the CLI.

- [x] `reactor-core` crate: catalogue, probes, cache, activation, install,
      skills fetch, registry rendering
- [x] `reactor-cli` binary, same subcommands, same flags
- [x] Shell completion generator carried over (ADR-0015)
- [x] State root `~/.pi/reactor/` → `~/.reactor/`, with a one-time move
- [x] `reactor setup` replaces `scripts/install.py`; the shipped catalogue is
      compiled in; `cargo install` places the binary (ADR-0039)
- [x] The extension harness execs the Rust binary
- [x] `scripts/check-in-pi.mjs` puts the Rust build on `PATH` (it used to put `bin/`)
- [x] Building `reactor-gui` was a step of `install.py`; it is now
      `cargo install --git … reactor-gui` (phase 2 put the GUI in the root workspace)
- [x] Deleted `bin/reactor`, `tests/test_reactor.py`, `scripts/install.py` and
      `scripts/parity.py` once the gate below was green (`parity.py` needed the
      original to diff against, so it went last)

**Gate:**

- [x] The 90 Python tests, ported, pass against the Rust binary (151 tests:
      `cargo test`)
- [x] The registry byte-stability test passes, against golden bytes captured
      from the Python renderer
- [x] `scripts/parity.py`: 150 commands and both cache directions byte-identical
      to the Python CLI (the one deliberate difference, `doctor`'s
      `platform.python` → `platform.reactor`, is normalised and recorded in
      ADR-0039)
- [x] The extension suite (`node --test`, pi flavor) is green against the Rust
      binary: 301 pass, 0 skipped, on pi 0.99.1 and Node 24

### 2 · The GUI links the library  *(reversible)*

Replace the GUI's per-panel subprocess with a direct call into
`reactor-core`. Still pi-backed, still RPC for the session itself.

- [x] The GUI crates join the root workspace (one `Cargo.lock`, one place to
      build); `default-members` leave `reactor-gui` out so a bare `cargo test`
      does not need the native windowing stack
- [x] `reactor-client` depends on `reactor-core`; `LibClient` answers every
      `ReactorClient` method in-process
- [x] The subprocess path stays as a debug fallback: `REACTOR_GUI_CLIENT=cli`
- [x] A check that both paths agree: `crates/reactor-cli/tests/agree.rs`
- [x] `CliClient` surfaces the CLI's JSON `error` instead of a blank failure
- [x] `app.rs` holds a `Client` instead of a `CliClient` (builds; the window
      launches on GNOME/Wayland)
- [x] Window chrome: client-side title bar with controls, and the Layout menu
      drawn in-window (`chrome.rs`) — neither was ever rendered on Linux

**Gate:** the GUI behaves identically with the fallback on and off.
- [x] Data path: `agree.rs` — every method, both probe orders, writes, errors
- [x] Launched both ways under `strace -f -e execve`: the default spawns **no**
      `reactor` process and still populates `cache.json`; `REACTOR_GUI_CLIENT=cli`
      spawns `reactor services|tools list|toolsets list`; the two `cache.json`
      files are identical up to timestamps (the first run was not: the GUI's
      dependency graph enables `serde_json/preserve_order`, cargo unified it into
      `reactor-core`, and file JSON lost its sorted keys — now sorted explicitly,
      with the core tests built under that feature)
- [x] Eyes on the panels with the fallback on and off — checked by hand:
      identical, and window chrome and the Layout menu work

### 3 · `reactor-context`  *(reversible)*

Port the state machines and rendering of the four stateful extensions into a
crate, and prove the port faithful against the extensions themselves.

The plan first said the pi extensions would "become shims over it". They do not:
[ADR-0040](docs/adr/0040-reactor-context-is-a-port-verified-against-the-extensions.md)
records why — shims need the session-scoped CLI surface ADR-0035 rejects, or a
WASM toolchain in the frozen half — and what replaces them: the extensions are
driven through pi's own loader, what they say and do is captured, and the Rust
must reproduce it byte for byte.

- [x] Manifest, identity, scenario, reporting state + deterministic rendering
      (`crates/reactor-context`)
- [x] Settings cascade (ADR-0038), global half only: `settings.json` and
      `settings::resolve`
- [x] The capture: `tests/extensions/golden/capture.mjs` → `tests/golden/*.json`
      (4 components, 15 cases, 195 scripted operations, including every phase of
      the shipped `investigation` scenario)
- [x] ~~The pi extensions become shims over it~~ — replaced by the capture, above

**Gate:**
- [x] Rendered blocks are byte-identical to the extensions': the golden replays
      pass for all four (`cargo test -p reactor-context`)
- [x] The extension suite still passes — trivially, as they are untouched
- [x] `capture.mjs --check` is clean, so the goldens are the extensions' present
      behaviour
- [ ] Open: **embedding the shipped scenarios.** `scenario::Scenario` reads a
      directory; a bare binary has none. Phase 4 decides between compiling
      `prompts/scenarios/` in (as `tools.toml` is) and a configured path.

### 4 · `reactor-agent`  *(the commitment)*

The new part. What was built and the choices it settled are in
[ADR-0041](docs/adr/0041-the-agent-loop-owns-the-message-list.md).

- [x] Session store: append-only JSONL + derived index, branches as parent
      pointers, reductions as entries over a range, blobs for whole tool output
      (ADR-0036)
- [x] Loop over rig: tool dispatch, streaming, thinking — through rig's
      `CompletionModel::stream` (five providers wired), with the message list ours
- [x] Budget manager: fade, summarizer, three modes, checked before every request
      and after every tool result (ADR-0037); previewable, undoable
- [x] Loop invariants: consecutive-reduction budget; never continue after a failed
      reduction; every call in a recorded reply gets a recorded result
- [x] Skill loading + activation gating (`requires:` for authored skills, the
      registry for upstream ones)
- [x] Long-running and high-volume tool I/O: persistent shells, streaming,
      truncation policy with a shared address format, spill-to-disk
- [x] `examples/repl.rs`, a development-only terminal REPL over the same `Agent`, so it can
      be run before the GUI is on this backend (not a frontend; removed in phase 5)
- [ ] Open: budget knobs (`mode`, `pct`, `keep`, `reserve`, summarizer model) are
      constructed in code, not resolved through settings — phase 5, with the UI
- [ ] Open: scenarios and authored skills are read from directories; compile them in?

**Gate:** a real RE session — acquire, triage, analyse, report — runs end to end
without the operator working around the harness.

- [x] Everything up to the wire, against a scripted model (73 tests)
- [x] A real session against a real model: run by hand through `examples/repl.rs`
      (Ollama) and it works. That was one operator's smoke test, not the full
      acquire → triage → analyse → report session; longer runs will still turn up work.

### 5 · The GUI switches backends

- [ ] Session, transcript, composer and tree read the Rust harness
- [ ] Reduction preview and undo
- [ ] Settings scope visible in the UI (session vs default)
- [ ] `reactor-rpc` retired; `gui/SPEC.md` §2 rewritten
- [ ] pi shims published and pinned for the maintained four

**Gate:** no `pi --mode rpc` process in a GUI session, and the pi flavor still
passes its own suite.

## Open

- **The keep-window size, and whether it adapts.** The one real knob in `auto`
  mode. Start with the fade's existing `pct` and `reserve`; revisit after
  watching it run, not before.
- **What the summarizer runs on.** Same model by default, configurable
  (ADR-0038). Compaction now fires mid-turn at tool-result boundaries, so a
  slow summarizer is felt directly as a stall while the agent works. Worth
  measuring once there is something to measure.
