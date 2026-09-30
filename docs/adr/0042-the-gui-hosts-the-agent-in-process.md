# The GUI hosts the agent in process, and activation is session-scoped

Phase 5 of the migration ([ADR-0033](0033-reactor-is-a-rust-project-on-rig.md)) switches
`reactor-gui` from `pi --mode rpc` to the Rust harness. This supersedes
[ADR-0032](0032-the-gui-extends-pis-rpc-through-existing-channels-only.md).

## Decisions

**The agent runs inside the GUI process, on its own tokio runtime.** No child process, no
wire protocol: `Backend` calls `reactor-agent` and receives events over a `std::sync::mpsc`
channel polled by the UI thread. gpui's executor never runs agent work, so a slow provider
cannot stall painting. `reactor-rpc` is deleted.

**The transcript is derived from the store.** The rows are a pure function of the current
branch plus live streaming buffers; a finished turn rebuilds from disk. Reductions show as
cards with *undo*, and rows a reduction hides from the model are dimmed.

**The extension-view contract is retired.** It existed to let pi extensions draw in the
GUI; without pi there is no sender. `/guide` is not ported now.

**Activation is session-scoped, with an explicit promotion.** A tool or toolset toggle
writes `<session dir>/activation.json`; the machine `state.json` is the default a session
inherits. *Make default* copies the session's state to `state.json`; *inherit* deletes the
override. The same cascade (built-in → global → session) governs context settings
([ADR-0038](0038-settings-resolve-global-then-session.md)), and the UI
labels every value with where it came from.

## Consequences

- Toggling a tool in one window no longer changes another window's session.
- The thinking-level picker is gone until the agent exposes it.
- The pi flavor keeps its own extension suite; the GUI does not depend on it.
