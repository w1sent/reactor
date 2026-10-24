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
| `store`, `entry` | append-only session log in the case's database, parent-pointer branches, blobs by content ([ADR-0036](../../docs/adr/0036-reactor-owns-its-session-store-format.md), [ADR-0046](../../docs/adr/0046-the-analysis-database-is-sqlite-inside-the-case.md)) |
| `context` | the projection of a branch into what the model sees, with reductions applied |
| `budget` | one policy, three modes, planned purely and previewable ([ADR-0037](../../docs/adr/0037-context-reduction-is-one-budget-manager.md)) |
| `history` | reading the log back whole, including everything a reduction hides, and the case's other sessions |
| `llm`, `provider` | the seam to a model: rig for real, a script for tests |
| `tools` | `bash` (persistent shells), `read`/`write`/`edit`, `history_*`, `update_steps`, `reactor_phase_complete`, `case_get` (the case read by reference, [ADR-0066](../../docs/adr/0066-the-case-is-addressed-by-references.md)), and the case graph's `graph_write`/`graph_query`/`graph_pull`/`graph_revert`/`define_type`/`artifact_register` ([ADR-0047](../../docs/adr/0047-the-analysis-graph-is-the-case-s-model.md)); a group that costs more than `toolSearch.deferOverTokens` (the graph's, or one MCP server's) is left for `tool_search` to load ([ADR-0055](../../docs/adr/0055-large-tool-groups-are-loaded-on-demand.md)) |
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

A session belongs to the case of the working directory, or to the unsaved state that
directory gets until it is saved as one ([ADR-0045](../../docs/adr/0045-a-case-is-the-working-directory.md));
resume one with `--resume <session id>`.

## Tests

`cargo test -p reactor-agent` drives everything through a scripted model: tool rounds,
mid-turn reduction, the failed-reduction and cancellation invariants, state-gated tools,
truncation and blobs, persistent shells, resuming. Nothing there talks to a provider.
