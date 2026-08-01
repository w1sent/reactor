# bin/

The `reactor` CLI. Stdlib-only Python, 3.11+ (for `tomllib`), symlinked onto the
user's `PATH` by `scripts/install.py` — default `~/.local/bin/reactor`, mirroring
how `bn` installs. Not implemented yet.

[ADR-0005](../docs/adr/0005-reactor-cli-stdlib-python.md) records why this is one
command, why Python, and why nothing may be imported that is not in the standard
library: the program whose job is to report what is missing must never itself be
missing.

## Surface

```
reactor doctor                    what is present, what is not, how to get it
reactor tools list [--tag T]      catalogue listing
reactor tools show <id>           one entry in full
reactor skills list               skills REactor knows about, incl. fetched
reactor skills show <id>          print a skill for review before trusting it
reactor install <id>...           opt-in install, platform-appropriate
reactor diff-config               diff(1) shipped vs installed config
reactor overwrite-config          force-replace installed config (backs up first)
```

`--format json` is supported on every subcommand and is **not optional**: it is
the only interface the pi extensions have, so its shape is part of REactor's
contract and changes to it are breaking. Human-readable output carries no such
guarantee.

## Constraints

- Reads `~/.pi/reactor/`, never the shipped copies in the package
  ([ADR-0003](../docs/adr/0003-tools-toml-single-source-of-truth.md)) — except
  `diff-config` and `overwrite-config`, whose whole job is to compare the two.
- Does not import from pi and does not require pi to be running.
- Probes need timeouts and a cached-value fallback. Once the registry extension
  is wired up, a hanging probe stalls an agent turn.
- A probe that times out is reported as *unknown*, not as absent.
