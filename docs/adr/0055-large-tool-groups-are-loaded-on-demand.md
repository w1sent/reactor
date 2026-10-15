# Large tool groups are loaded on demand, through one tool that lists them

A tool definition costs every request, for as long as it is advertised. The case graph's
five tools were about 1,900 tokens of every request in every session attached to it, and
used in some of them; an MCP server with forty tools costs more than that, and
[ADR-0048](0048-mcp-tools-are-catalogue-entries-the-model-gets-as-definitions.md) made
the user's only remedy not to switch it on. Measured on the test rig, the graph's tools
and their block were 2,570 estimated tokens of every request; with this decision they are
764, and a session that uses the graph pays the rest from the step it loads them.

## Decisions

**A group is the tools that arrive together.** The case graph's five tools are one group.
Each MCP server's active tools are one group, named after the server. A tool of neither
(`bash`, `read`, `history_*`) is in no group and is always advertised.

**A group that costs more than a threshold is not advertised.** The threshold is
`toolSearch.deferOverTokens` in `settings.json` (in the GUI under Settings › Tools),
default 1,000 estimated tokens, `0` deferring every group. It is compared with the sum of
the group's definitions as the request would carry them, so a group is all in or all out:
a server with two small tools stays direct, one with forty is deferred. Nothing is
configured per tool.

**What the model gets instead is `tool_search`.** It is advertised exactly when some group
is deferred. Its description lists every tool of every deferred group by name and first
sentence, which is the part a model needs to know that the tool exists and what it is for;
its one required argument, `query`, is `select:name1,name2` to load those tools, or words
that are searched for in names and descriptions (a hit in a name counts for more; `hosts`
finds `host`). `max_results` bounds a words query, default 5. The names and the argument
form are the ones models already know from other clients' tool search, on purpose.

**A load is a state entry.** The result is a `tools` entry in the session log, holding the
whole loaded set, and the latest one on the branch wins: the same mechanism as the manifest
and the scenario ([ADR-0041](0041-the-agent-loop-owns-the-message-list.md)). A loaded tool
is advertised from the next request, on that branch, across a resume and across a context
reduction. The request is a pure function of the log, the catalogue and the settings.

**The listing does not change when a tool is loaded.** `tool_search`'s description names
the deferred tools whether or not they are loaded since, and says that on a repeat. The
only thing a load changes in the request is one more definition, so the provider's prompt
cache is broken once per load and never by the listing.

**Deferral is advertisement, not enforcement.** A tool that is not advertised can still be
called and runs, as in [ADR-0007](0007-deactivation-is-soft.md) and
[ADR-0030](0030-gated-tools-advertise-by-state.md): a model that guesses the arguments of
`graph_query` without loading it gets the call run, and a wrong one is answered by the
tool's own message. `tool_search` is a reserved name, so no MCP tool can take it.

**The case-graph block says so.** While the graph's tools are deferred the block ends with
the `select:` call that loads all five, because the block names `graph_add` and the model
should not have to guess why it is not in its list.

## Consequences

- The first use of a deferred tool costs one extra call. For the graph the block asks the
  agent to record as it goes; the call is made once, early, and the five stay loaded.
- A server's tools are loaded one by one, as asked for, and a server's cost on the row of
  the Toolbox (`MCP · ~N tok`, ADR-0048) is now what loading it costs, not what every
  request carries. The line that totals the active ones is an upper bound for a deferred
  server. The context inspector shows what the request carries: the loaded definitions, and
  `tool_search` with its list.
- Toolsets and activation are unchanged. Activating a tool still decides whether it is
  *offered*; deferral decides how, and a deferred tool is as present in the registry as
  before.
- `toolSearch.deferOverTokens` is global only, like `toolOutput.maxInlineBytes`
  ([ADR-0053](0053-a-call-chooses-how-much-of-its-output-it-sees.md)): the session already
  chooses by loading.
- The threshold is on estimated tokens (a quarter of the characters), the estimate the
  budget manager and the inspector already use, so the numbers on screen agree.

## Considered and rejected

- **A flag per tool or per server** (pi's `exposure`). Right for someone who knows what
  each tool costs, and one more thing to maintain for everyone else. A group's own cost is
  the information the choice would be made from, and one number applies it.
- **Always defer MCP, never defer the graph** (or the reverse). Both are arbitrary: the
  difference between five tools and five hundred is size, and size is what the threshold
  measures.
- **A gateway that calls the tool** (`mcp_call` with a name and arguments, rejected in
  ADR-0048). Loading keeps the call a real call with a real schema, which is what the
  rejection said models do more reliably, and puts the cost where it is paid once.
- **Listing only what is still unloaded.** Shorter after a load, and the description
  would change on every load; a longer prefix of the request would be invalidated each time.
- **Putting the list in the system prompt.** It would be seen by a session that has no
  deferred tools too, as a header with nothing under it, and would move when the registry
  does. The list belongs to the tool that uses it.
- **Ranking with BM25 or embeddings.** A few dozen tools do not need it; names, a stop
  list and a plural strip rank them well enough, and `select:` is exact.
