# tool-registry

Tells the agent what RE tooling exists on this machine. One compact block —
`## Available RE tools (this machine)` — appended to the system prompt: one
line per *present and active* tool, with live service state where applicable,
byte-stable so the prompt cache holds. Plus a footer entry (`RE <present>/<catalogued>`) and the answer to pi's
`resources_discover`: the skill directories of the active, present tools.

```
/reactor                show the registry block as the agent sees it
/reactor refresh        drop the cache, re-probe, show again
/reactor-toolbox        report on/off
/reactor-toolbox off    disable the toolbox, then reload
```

`/reactor-toolbox` is registered even when the toolbox is off — it is the one
command able to turn it back on. A failed probe keeps the last good block; a
missing CLI is warned about once, then silent.

`toolbox: false` in `<agent dir>/reactor.json` removes the injection, the
footer and `/reactor` entirely, checked once at registration.

The reasoning — why the CLI pre-renders the block, why it must be
byte-stable, why everything shells out to `reactor` — is in
[ADR-0006](../../docs/adr/0006-registry-injected-into-system-prompt.md) and
[ADR-0005](../../docs/adr/0005-reactor-cli-stdlib-python.md); the toolbox gate
in [ADR-0016](../../docs/adr/0016-extension-toggles-live-in-their-own-pi-side-file.md).
