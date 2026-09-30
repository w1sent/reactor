# reactor-agent

The agent: the session store, the loop over [rig](https://rig.rs), the context budget
manager, and the tools. Owning the message list is the point
([ADR-0033](../../docs/adr/0033-reactor-is-a-rust-project-on-rig.md),
[ADR-0041](../../docs/adr/0041-the-agent-loop-owns-the-message-list.md)).

```
reactor-core      catalogue, probes, install          machine facts
reactor-context   manifest, identity, scenario, …      session state and its text
reactor-agent     this crate                          the loop
```

| module | what it is |
|---|---|
| `store`, `entry` | append-only JSONL log, parent-pointer branches, blobs, a derived index ([ADR-0036](../../docs/adr/0036-reactor-owns-its-session-store-format.md)) |
| `context` | the projection of a branch into what the model sees, with reductions applied |
| `budget` | one policy, three modes, planned purely and previewable ([ADR-0037](../../docs/adr/0037-context-reduction-is-one-budget-manager.md)) |
| `history` | reading the log back whole, including everything a reduction hides |
| `llm`, `provider` | the seam to a model: rig for real, a script for tests |
| `tools` | `bash` (persistent shells), `read`/`write`/`edit`, `history_*`, `update_steps`, `reactor_phase_complete` |
| `truncate` | head-and-tail cutting with an address for the rest |
| `skills`, `prompt` | what goes in the system prompt, in a fixed order |
| `agent` | the loop, its invariants, and the session commands |

## Try it — a development harness, not a product

REactor has no TUI ([ADR-0033](../../docs/adr/0033-reactor-is-a-rust-project-on-rig.md)); the GUI
is the frontend. Until it runs on this backend (phase 5), `examples/repl.rs` is a plain
terminal REPL over the same `Agent`, so the loop can be run against a real model. It is an
*example*: not built by default, not installed by `cargo install`, and it goes away later.

```bash
export ANTHROPIC_API_KEY=…
cargo run -p reactor-agent --example repl -- --model anthropic/claude-sonnet-5-5
> look at ./a.out and tell me what it does
> /preview          # what a reduction would do
> /reduce compact   # summarize the older work
> /undo             # and bring it back
> /history 1 40     # the whole log, including what left your context
```

Sessions live in `~/.reactor/sessions/<id>/`; resume one with `--resume <dir>`.

## Tests

`cargo test -p reactor-agent` drives everything through a scripted model: tool rounds,
mid-turn reduction, the failed-reduction and cancellation invariants, state-gated tools,
truncation and blobs, persistent shells, resuming. Nothing there talks to a provider.
