# gui/

reactor-gui — the native Rust frontend (gpui-kit), hosts the Rust agent in-process (phase 5;
[ADR-0042](../docs/adr/0042-the-gui-hosts-the-agent-in-process.md)). [`SPEC.md`](SPEC.md) is the specification this directory will be
implemented against; the decisions that put it here are
[ADR-0031](../docs/adr/0031-reactor-gui-lives-in-the-repo-and-installs-with-it.md)
and
[ADR-0032](../docs/adr/0032-the-gui-extends-pis-rpc-through-existing-channels-only.md),
and the pi facts the design depends on are recorded under *"Facts for
reactor-gui, verified against pi 0.87.0"* in
[`docs/pi-api-notes.md`](../docs/pi-api-notes.md).

Layout:

```
crates/reactor-client ReactorClient trait: LibClient (reactor-core, in-process) + CliClient (debug fallback)
crates/reactor-gui    the application
```

Nothing here
is a pi resource directory (`skills/`, `prompts/`, `themes/` pi would
register — `gui/` is inert to pi, see `docs/package-resources.md`).