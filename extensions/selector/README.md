# selector

The human's lever on what the agent is told about. One overlay, two panes
(`Tab` to switch): the catalogue's tools and the toolsets, with presence and
live service state.

```
/reactor-tools           open the overlay
type                     fuzzy search filters as you type (`/` resets it)
space                    toggle the selected tool or toolset
Enter                    inspect the entry in full
Ctrl+R                   unpin a per-tool override
```

Every write goes through the CLI, and one `ctx.reload()` on close applies the
curation to the system prompt on the next turn. `toolbox: false` in
`<agent dir>/reactor.json` removes this command entirely.

The rendering choices, the toggle-answer contract and the edit-vs-outcome
reasoning: [ADR-0011](../../docs/adr/0011-selector-edits-overrides-not-outcomes.md);
the gate: [ADR-0016](../../docs/adr/0016-extension-toggles-live-in-their-own-pi-side-file.md).
