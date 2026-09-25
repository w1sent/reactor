# gui-bridge

The commands reactor-gui needs that pi's RPC itself does not carry. pi's RPC
mode has no `navigate_tree` command — the command switch ends at
`get_commands` — but it wires `ctx.navigateTree` into extension command
contexts in every mode, so this extension is the bridge the GUI's session
tree rides.

```
/reactor-tree <entryId>   move the active branch to a session entry
```

Dispatched by reactor-gui's tree panel ("Switch branch here"); the affordance
is disabled while the agent streams, because navigation rejects then
([docs/pi-api-notes.md](../docs/pi-api-notes.md)). In the TUI the builtin
`/tree` already covers the need, and the command works there too — the same
handler, both hosts.

Nothing here is agent-facing: the command registers nothing else, touches no
CLI, and other RPC clients never see it unless they invoke it. The spec is
[`gui/SPEC.md`](../gui/SPEC.md) §4.6; the decision to ride existing channels
only is
[ADR-0032](../docs/adr/0032-the-gui-extends-pis-rpc-through-existing-channels-only.md).