# MCP tools are catalogue entries that the model gets as definitions, and nothing is active until asked for

> **Amended by [ADR-0052](0052-the-window-is-a-rail-a-drawer-and-a-workspace.md):** the Tools
> and Toolsets panels are one panel, the Toolbox, because a tool can be in several toolsets
> and two lists could not show that. Where this says "the Tools panel", read the Toolbox: the
> `MCP · ~N tok` tag on the row and the line totalling the active ones are there, unchanged.

REactor's position is that a CLI composes and an MCP call does not
([concept](../concept.md#why-a-cli)): a command's output can be filtered by `jq` or
`rg` before it becomes context; a tool result cannot. That position stands. But MCP
servers exist, are often the only way to reach a piece of tooling, and are often
unavoidable, so REactor has an MCP *client*. This ADR says where it sits, so that it
costs the concept as little as it can.

## Decisions

**An MCP tool is a catalogue entry.** The tools of every configured server join the
catalogue after the tools of `tools.toml` (so the registry never reorders,
[ADR-0006](0006-registry-injected-into-system-prompt.md)). They are listed in the Tools
panel and by `reactor tools`, activated and deactivated by the same toggles, held in
`state.json` by the same overrides ([ADR-0011](0011-selector-edits-overrides-not-outcomes.md)),
and picked up by toolsets through the same tags
([ADR-0013](0013-toolset-tags-intersect.md)). Nothing about activation, session
scope ([ADR-0042](0042-the-gui-hosts-the-agent-in-process.md)) or the `all` toolset is
special-cased for them. A tool's id is also the name the model calls it by. By default
that is the name its server gives it; a setting (`prefixToolNames` in `mcp.json`, in
Settings › MCP servers) makes it `mcp__<server>__<tool>`, as in other MCP clients. A name
that is already taken — by a tool of `tools.toml`, by one of the agent's own
(`reactor_core::mcp::RESERVED`, kept true by a test) or by an earlier MCP tool — keeps the
prefix whatever the setting, so nothing is shadowed. The catalogue and the host assign ids
with the same function over the same list, which is why a server that is down still
counts for the names it last offered. Changing the setting renames every tool: activation
entries made one tool at a time are by the old name; toolsets (selected by tags) are
unaffected.

**It is not in the registry block.** A catalogue tool costs one line of the system
prompt and is run from the shell. An MCP tool is run by the model calling it, so what the
model gets is a tool *definition* — name, description and the whole JSON schema — in the
request's tool list, for as long as the tool is active. Both the line and the definition
would say the same thing twice, so only the definition goes out.

**The cost is on the row.** That difference is the thing the user must see when choosing
what to activate, so every MCP tool carries the tag `mcp` (which a toolset can select),
its server's name and the server's own tags, and the Tools panel marks it `MCP · ~N tok`
with the estimated size of its definition, plus a line above the list totalling the
active ones. The same sizes appear per tool in the context inspector, where definitions
were already listed.

**Servers have tags; a server's tags are on all its tools.** Tags are how a server's
tools enter a toolset: `tags = ["research"]` in a toolset includes the tools of every
server the user tagged `research`, without naming a tool that does not exist until the
server is reached.

**Each server has a toolset of its own.** `mcp-<server>` activates every tool of that
server at once. It is made, not written: it selects by the tags `mcp` and the server's name,
so it follows the server's tools, and its description gives the total cost. It exists while
the server has tools to list and is switched on; a toolset of the same id in
`toolsets.toml` replaces it.

**Nothing is active until asked for — for every tool, not only MCP ones.** With no toolset
active the base is *empty*; it used to be "everything"
([ADR-0011](0011-selector-edits-overrides-not-outcomes.md)). A server with forty tools
that is switched on by adding it would otherwise put forty definitions into every
request, and the old rule had no way to say so. `all` is the toolset for everything, and
it includes MCP tools: the user asked for all of them.

**Deactivation stays soft.** A tool that is not advertised can still be called, and runs
([ADR-0007](0007-deactivation-is-soft.md)); a model reading its own history may call a
tool the user has since turned off. There is no approval layer for MCP calls either
([ADR-0033](0033-reactor-is-a-rust-project-on-rig.md)): a local server is a process
REactor starts as the user. The Settings page and the editor say so.

## Where it lives

- **`reactor-core`** (`mcp.rs`) reads and writes `~/.reactor/mcp.json` — the `mcpServers`
  form other clients use, so an entry can be pasted: `command`/`args`/`env` for a local
  server, `url`/`headers` for a remote one (streamable HTTP), plus `tags` and `enabled`.
  `${NAME}` in a value is an environment variable, so tokens need not be written down.
  A file that cannot be read is an *error*, never an empty list, so a save cannot
  overwrite what a person wrote by hand; the catalogue then simply lists no MCP tools.
  It also reads `mcp-cache.json` and turns the two into catalogue entries. It does not
  connect: the crate is files and subprocesses
  ([ADR-0034](0034-reactor-cli-becomes-a-rust-library-with-a-binary.md)).
- **`reactor-agent`** (`mcp.rs`) is the client, over the `rmcp` SDK. rig's own MCP support
  (`rmcp_tools`) attaches tools to rig's `Agent`, which REactor does not use — the loop
  owns the message list ([ADR-0041](0041-the-agent-loop-owns-the-message-list.md)) — so
  the host talks to `rmcp` directly and hands the loop what it already understands: a
  `ToolSpec` to advertise and a `ToolOutput` to record. One `McpHost` is shared by every
  session of a window, with a runtime of its own, so a connection outlives the session
  that started it. It writes `mcp-cache.json`: per server whether it was reached and, per
  tool, its name, first description line and estimated size. A server that fails keeps the
  tools it last offered, listed as unavailable, so what a user activated is not forgotten.
- **`reactor-gui`**: Settings › MCP servers (add, edit, reconnect, turn off, remove) and
  an editor window; the Tools panel's tag and cost line. Servers are connected to once
  when the window starts and again on a change or on *Reconnect*.

An MCP tool is neither detected, probed nor installed: whether it is "present" is whether
its server answered last time. `doctor`, `refresh` and `install` leave it alone.

## Consequences

- A fresh `reactor setup` seeds an empty `state.json`, which now means *nothing active*.
  A new user activates a toolset (or `all`) once.
- Existing `state.json` files that relied on the empty default lose their tools until a
  toolset is chosen. Tools pinned on while no toolset was active now stay on.
- Output a server returns is text to the model; an image or audio block is replaced by a
  note saying what it was. A server's own error text and, for a local server, the end of
  its standard error, are what the user sees when it fails to start.
- Not done: servers' `tools/list_changed` notifications (use *Reconnect*), resources,
  prompts and sampling, OAuth for remote servers (a bearer token or header covers most),
  per-case server lists. A server that dies mid-session reports an error per call until
  reconnected.
- The MCP call goes straight to context, as the concept says it must. The tag and the cost
  figure exist so that this is a choice the user makes with the number in front of them.

## Considered and rejected

- **A gateway tool** (`mcp_call` plus a registry line per tool, schemas read on demand).
  Far cheaper in context and closer to how REactor treats CLIs, and a reasonable later
  addition. Rejected for now: the request was for MCP tools to be shown, switched and
  priced like the others, and a model calls a tool with a real schema more reliably than
  it composes arguments for one it has not seen.
- **Servers inside `settings.json`.** One file, but the catalogue (in `reactor-core`,
  which `reactor-context` depends on) would have to read it, and tokens would sit beside
  unrelated preferences.
- **Active by default, as catalogue tools were.** See above: the cost is per request.
