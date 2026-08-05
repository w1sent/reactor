# Extensions are tested through pi's own loader

Extension tests run on **`node --test`** with nothing installed, are written in
plain ESM `.mjs`, and load the extension under test through **pi's own
`loadExtensions`** — the same jiti instance, the same alias map, the same
`ExtensionAPI` implementation pi builds at runtime.

What is faked is only what pi's *host* provides: `ExtensionContext`, `ctx.ui`,
and the `TUI`/`Theme`/`done` triple that `ctx.ui.custom` hands a component.

What is **not** faked is `reactor`. `pi.exec` is pi's real implementation, and
the `reactor` it finds on `PATH` is a shim that either execs the real
`bin/reactor` against a temporary `REACTOR_CONFIG_DIR` or reproduces one named
failure mode.

## Why

**Something has to load the TypeScript.** Node's native type stripping cannot:
the selector's overlay uses constructor parameter properties, which are not
erasable syntax, and Node rejects the file with
`ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX`. So the choice is not "loader or no loader",
it is *which* loader.

**jiti is the one pi uses, and sameness is the point.** Extensions do not import
`@earendil-works/pi-tui` from anywhere on disk — pi's loader aliases the
specifier to its own bundled copy, which is what makes `getKeybindings()` return
the bindings the running pi actually has rather than a second, empty instance.
A test that resolves those imports its own way is testing a different module
graph than production, and the ways it differs are exactly the ways that are
invisible until they matter.

**Going through `loadExtensions` rather than calling the factory** means
`pi.exec`, `registerCommand`, `registerEntryRenderer` and the rest are pi's
implementations, not ours, and the test inspects the same `extension.commands`
map the command palette reads. A stubbed `ExtensionAPI` — which is what the
hand-driven harnesses used — has to be kept in step with pi by hand, and it
agrees with pi right up until pi changes.

**The CLI is real because faking it would test the fake.** `reactor` is the
single implementation of catalogue semantics
([ADR-0005](0005-reactor-cli-stdlib-python.md)); an extension is a client of its
JSON, and the only useful question about a client is whether it works against
the actual server. The Python suite already points `REACTOR_CONFIG_DIR` at a
throwaway directory, so the same trick gives these tests a fixed catalogue
without touching `~/.pi/reactor/`.

**The shim exists for the failures.** "The CLI is missing", "it printed
garbage", "it returned an error payload" are most of what the registry extension
is made of, and none of them is reachable by pointing at a working CLI. A script
on `PATH` reproduces all three, and reproduces the first one *accurately*: pi's
exec resolves rather than throwing on ENOENT, so a missing binary and a crashed
one both arrive as code 1 with empty stdout.

**No dependency is added.** jiti and pi-tui come from the pi installation, which
[ADR-0001](0001-pi-is-the-only-target-harness.md) already makes a hard
requirement, and `node --test` is the runtime's own runner. The package still
declares no dependencies, which is the same reasoning that keeps the CLI
stdlib-only.

## Consequences

- **The tests need pi installed and on `PATH`.** They skip with a message rather
  than failing when it is absent, because "pi is not installed" is not a defect
  in REactor.
- **`dist/core/extensions/loader.js` is not a public export.** pi's `exports`
  map exposes `.` and `./rpc-entry` only, so the harness imports the loader by
  absolute path, which the exports map does not police. A pi upgrade that moves
  or renames it breaks the tests. Accepted: it breaks loudly, in one file, and
  the alternative is reimplementing the thing that upgrade would have changed.
- **The tests are JavaScript against TypeScript sources**, so they check
  behaviour and not types. Nothing type-checks this package today — there is no
  `tsconfig.json` and no `typescript` dependency — so this loses nothing that
  exists, but it does mean a test cannot catch a wrong `ToolRow` field name that
  an editor would.
- **A test run spawns the real CLI many times**, each a Python interpreter
  start. That is ~80 ms a call, and it is why the suite uses one fixture
  catalogue of three tools rather than the shipped one of twenty-four.
- **The `killed` branch is not covered.** Firing it means outlasting the
  extension's own 20 s exec budget, which is more than the whole suite costs;
  the shim has no `hang` mode for that reason. From the caller's side it lands
  in the same place as the other three — status set, `undefined` returned, last
  good block kept — so what is lost is the status text, not a path.
- **A test that wrongly opens an overlay must fail, not hang.** `ctx.ui.custom`
  never settles until the component calls `done`, so the fake throws when it is
  called outside `mode: "tui"` rather than returning a promise nobody will
  resolve. Found by mutation: gating on `hasUI` instead of `mode` made the suite
  hang rather than fail, which is the one outcome worse than a false pass.
- Anything the fixture asserts about presence has to be true everywhere, so the
  catalogue detects `sh` (present on any POSIX machine) and a deliberately
  absent binary, never a real RE tool.

## Considered and rejected

- **vitest.** pi itself uses it, so it would be a familiar choice, and it has
  watch mode and a nicer diff. Rejected because it is a large dependency tree in
  a package that has none, and it buys nothing here: `node --test` already
  supplies the runner, the reporter, `node:assert`, and subtests. The moment
  this package needs a bundler or a browser environment that calculus changes.
- **Node's native type stripping** (`node --test extensions/*.ts`). Zero
  dependencies and the shortest possible command. Rejected because it cannot
  load the files, and the only way to make it work is to strip parameter
  properties out of the selector — shaping production code around a test runner,
  for a runner that still would not give us pi's alias map.
- **A hand-rolled `createJiti` call with our own alias map.** This is what the
  scratch harnesses did, and it works. Rejected as the thing most likely to rot
  silently: it duplicates a map that lives in pi's loader and changes with pi's
  layout, and when it drifts the symptom is not an error but a test passing
  against the wrong module.
- **A stubbed `ExtensionAPI` object.** Simpler to read, and it makes assertions
  like "did it register the right event" trivial. Rejected for the same reason:
  it is a second implementation of pi's contract, maintained by us, diverging
  from pi on pi's schedule and never telling us.
- **Faking `reactor`'s JSON responses.** Fast, hermetic, no Python in the loop.
  Rejected: the fixtures would be transcriptions of what the CLI emitted on the
  day they were written, so the contract test would keep passing after the
  contract broke. The JSON shape is already pinned on the Python side
  (`TestJsonContract`); duplicating those literals here would create two
  documents that must agree and no mechanism to make them.
- **Golden-file assertions on `render()`.** Rejected because the layout is
  expected to keep changing and a golden file makes every layout change look
  like a regression. The properties worth defending are invariants — no line
  wider than the `width` passed in, no version truncated into a different
  version, the overlay asking for the full terminal width — and those are stated
  directly.
