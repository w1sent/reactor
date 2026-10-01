# reactor-context

The session half of REactor as state machines and deterministic text: **manifest**
(goal, guidelines, steps), **identity** (the working persona), **reporting**
(folder-change enforcement) and **scenario** (multi-phase analysis), plus the
**settings** they resolve from. No model, no agent loop, no UI, no rig — a pure
function of state, settings and a folder on disk.

```
reactor-core        catalogue, probes, install             (machine facts)
      ▲
reactor-context     this crate                             (session facts)
      ▲
reactor-agent       the loop composes these blocks; it does not author them
```

Each module is a port of one of REactor's former pi extensions with the host removed. A command is
`command(&mut State, …) -> Effects`; the block for the system prompt is
`block(&State, …)`. What a handler did *to the world* — an entry to append, a
toolset to enable, a settings file to save — comes back as data for the caller to
perform.

The original extensions were the specification
([ADR-0040](../../docs/adr/0040-reactor-context-is-a-port-verified-against-the-extensions.md)).
`tests/golden/*.json` records what they actually said and did; `tests/golden.rs`
replays it and demands the same answers, byte for byte.

```bash
cargo test -p reactor-context
```

The extensions are no longer in the tree, so the goldens are now fixtures: they
change only when the behaviour is changed on purpose. To regenerate them from
the originals, check out commit `014a9b8` and run
`node tests/extensions/golden/capture.mjs` there (needs pi and a Node with
TypeScript support).
