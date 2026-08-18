# bin/

The `reactor` CLI. One stdlib-only Python file, 3.11+ (for `tomllib`), symlinked
onto the user's `PATH` by `scripts/install.py` — default `~/.local/bin/reactor`,
mirroring how `bn` installs.

[ADR-0005](../docs/adr/0005-reactor-cli-stdlib-python.md) records why this is one
command, why Python, and why nothing may be imported that is not in the standard
library: the program whose job is to report what is missing must never itself be
missing. One file rather than a package for the same reason — a symlink with no
`sys.path` machinery behind it cannot half-work.

## Surface

```
reactor doctor                    what is present, what is not, how to get it
  --check-skills                  also compare fetched skills against their remote ref
reactor registry                  the block injected into the agent's system prompt
reactor services                  what is running, of the tools that declare a probe
reactor tools list                catalogue listing  [--tag T] [--active] [--present|--missing]
reactor tools show <id>           one entry in full, including install candidates
reactor tools enable|disable <id> activation state (a hint; never blocks — ADR-0007)
reactor tools reset <id>          drop the override; go back to what the toolsets say
reactor toolsets list|show <id>   named groups
reactor toolsets enable|disable   activate a group
reactor state                     what is active right now, and from which file
reactor skills list               configured upstream skills and whether they are here
reactor skills show <id>          print a skill for review before trusting it
reactor skills fetch [<id>...]    fetch configured upstream skills
reactor install <id>...           opt-in install  [--method M] [--dry-run] [--yes]
  reactor install all             every catalogued tool
reactor refresh                   drop the probe cache and re-probe
reactor completion <shell>        print a completion script (bash, zsh, fish -- ADR-0015)
reactor diff-config               diff(1) shipped vs installed config  [--file tools|toolsets]
reactor overwrite-config          force-replace installed config (backs up first)
```

`reactor __complete tools|toolsets` also exists — bare, unprobed ids that the
completion scripts above shell back into. It is left out of `--help`
deliberately (ADR-0015): it is not a documented interface, only something the
scripts that ship alongside it depend on.

`--format json` is supported on every subcommand and is **not optional**: it is
the only interface the pi extensions have, so its shape is part of REactor's
contract and changes to it are breaking. Human-readable output carries no such
guarantee. `tests/test_reactor.py::TestJsonContract` pins the shape.

`registry`, `state`, `toolsets` and `skills fetch` were added during
implementation and are not in ADR-0005's original sketch. `registry` is the
notable one: rendering the block here rather than in the extension keeps the
determinism the prompt cache depends on testable in a single language, and
leaves the extension a pure transport.

## Constraints

- Reads `~/.pi/reactor/`, never the shipped copies in the package
  ([ADR-0003](../docs/adr/0003-tools-toml-single-source-of-truth.md)) — except
  `diff-config` and `overwrite-config`, whose whole job is to compare the two.
  If the installed catalogue is absent it falls back to the shipped copy and
  says so, so `reactor doctor` works before `install.py` has run.
- Does not import from pi and does not require pi to be running.
- Probes have timeouts and a cached-value fallback. Once the registry extension
  is wired up, a hanging probe stalls an agent turn.
- A probe that times out is reported as *unknown*, not as absent.
- `--format json` never writes anything to stdout but the payload. Confirmation
  prompts, installer output and progress all move elsewhere in that mode.
- `install` runs a command only when its package manager was verified present.
  Free-text install keys (`manual`, a URL) are shown and never executed
  ([ADR-0010](../docs/adr/0010-install-recipes-keyed-by-package-manager.md)).
- Nothing time-derived may reach `render_registry`. That is enforced by a test,
  not by care.
- Activation edits are **minimal**: `enable`/`disable` store an override only
  where the active toolsets do not already produce that answer, so toggling a
  tool off and on again leaves `state.json` byte-identical
  ([ADR-0011](../docs/adr/0011-selector-edits-overrides-not-outcomes.md)). Each
  entry in `tools list` carries `override` (`"on"`, `"off"`, `null`) alongside
  `active` so a client can say which of the two it is looking at.
- **A tool that is not installed has no service state.** `services` reports it
  as `unknown`, never `down` — "down" is a claim that something exists and is
  not running, and it would send the agent looking for a thing to start.
- **Every JSON document is written through a per-process temp file**, then
  renamed. Two `reactor` processes can be writing `cache.json` at the same
  moment, because every extension shells out on its own and they share nothing
  else ([ADR-0014](../docs/adr/0014-extensions-share-the-cache-not-each-other.md)).
- A toolset's `tags` **intersect**: a tool is a member when it carries every tag
  listed, not any of them
  ([ADR-0013](../docs/adr/0013-toolset-tags-intersect.md)). Union between groups
  is what activating two toolsets already does. `tools` and `tags` still union
  with each other, and `doctor` reports any toolset that ends up selecting
  nothing.

## Environment

| Variable | Effect |
|---|---|
| `REACTOR_CONFIG_DIR` | Overrides `~/.pi/reactor`. Used by the tests and by `scripts/install.py --config-dir`. |
