# gui/

reactor-gui — the native Rust frontend (gpui-kit), specified, not yet
built. [`SPEC.md`](SPEC.md) is the specification this directory will be
implemented against; the decisions that put it here are
[ADR-0031](../docs/adr/0031-reactor-gui-lives-in-the-repo-and-installs-with-it.md)
and
[ADR-0032](../docs/adr/0032-the-gui-extends-pis-rpc-through-existing-channels-only.md),
and the pi facts the design depends on are recorded under *"Facts for
reactor-gui, verified against pi 0.87.0"* in
[`docs/pi-api-notes.md`](../docs/pi-api-notes.md).

Planned layout (from the spec, §8):

```
crates/reactor-rpc    JSONL RPC client — commands, responses, events, UI sub-protocol
crates/reactor-cli    ReactorClient trait + CliClient (shells out to `reactor --format json`)
crates/reactor-gui    the application
```

Until the crates exist, this directory holds the spec alone. Nothing here
is a pi resource directory (`skills/`, `prompts/`, `themes/` pi would
register — `gui/` is inert to pi, see `docs/package-resources.md`).