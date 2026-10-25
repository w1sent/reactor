# REactor

A reverse-engineering agent harness in Rust: a `reactor` CLI and an agent loop
(over rig) with a native GUI, that tell an agent what RE tooling exists on the
machine it is running on and keep long sessions workable.

## Language

**REactor**:
The whole thing — this repository: the `reactor` CLI, the agent, and the GUI.
Capitalised "RE" is deliberate; it is not "Reactor".
_Avoid_: the harness, the framework, reactor-pi.

**Catalogue**:
`tools.toml` — the single source of truth for what tools REactor knows about:
identity, one-line description, detection probe, service probe, per-platform
install recipes, and the upstream skill source if the tool ships one. Shipped in
this repo, copied to `~/.reactor/tools.toml` at install time; the installed
copy is the one everything reads at runtime.
_Avoid_: registry (that is the injected block), manifest, tool database, index.

**Tool**:
One catalogue entry — an external binary or library REactor can detect, describe
and install. `binja`, `frida`, `jadx`, `yara`, `rg`. A tool is not written by us;
even the ones we do write live in their own repositories and enter REactor only
as a catalogue entry. Every entry declares its `source` — the tool's true
upstream, https only — which is what the manual install paths are trusted
relative to ([ADR-0028](docs/adr/0028-every-tool-declares-its-source.md)).
_Avoid_: integration, plugin, backend.

**Toolset**:
A named group of tools the user can activate or deactivate as a unit — `triage`,
`android`, `firmware`. Predefined ones ship in `toolsets.toml`; the user edits
their own copy at `~/.reactor/toolsets.toml`. Activation is purely about what
gets advertised to the agent; it never blocks anything. Nothing is active until a
toolset (or `all`) or a single tool is activated.
_Avoid_: profile, preset, bundle, workspace.

**MCP server** / **MCP tool**:
A server REactor connects to as an MCP client — a local command or a remote URL, listed in
`~/.reactor/mcp.json` — and the tools it offers. Not part of the concept, but often
unavoidable. An MCP tool is a catalogue entry (activated, tagged and put into toolsets like
any other) that is *not* in the registry: the model is given its definition as a tool, which
costs its whole schema in every request while it is active, so the Toolbox tags it with
that cost ([ADR-0048](docs/adr/0048-mcp-tools-are-catalogue-entries-the-model-gets-as-definitions.md)).
_Avoid_: plugin, extension, integration.

**Registry**:
The compact block REactor injects into the system prompt each turn: one line per
*present and active* tool — name, one-line description, how to invoke it, and
live service state where applicable. Rendered deterministically so that an
unchanged machine produces byte-identical text and the prompt cache holds.
_Avoid_: context block, tool list, inventory, preamble.

**Probe**:
The command that answers one question about a tool. A *detection* probe answers
"is it installed"; a *version* probe answers "which version"; a *service* probe
answers "is the thing it talks to actually running" (e.g. the MCP Extender's health
endpoint, for a live Binary Ninja session). Probes are cached; only service probes are re-run on
a timer.
_Avoid_: check, health check, test, ping.

**Manager**:
A package manager declared in `[platform.manager]` — `pacman`, `brew`, `uv`,
`cargo`. Install recipes are keyed by manager, never by distribution, so that a
recipe key is a testable predicate: the manager's binary is on `PATH` or it is
not. Ranked by `[platform].prefer`, which the machine's owner edits.
_Avoid_: platform, distro, backend, provider.

**Recipe** / **note**:
A *recipe* is an install command whose manager REactor has verified is present —
the only kind `reactor install` will ever run. A *note* is any other value in an
`[tool.*.install]` table: `manual`, a release URL, a command for a manager this
machine does not have. Notes are always shown and never executed — with one
deliberate exception: `manual-install-oneliner`, a runnable manual path that
`--auto-install-manual` (fallback) and `--force-install-manual` (override)
execute, promoting the result into the user-local PATH
([ADR-0027](docs/adr/0027-manual-install-oneliner-is-executed-only-under-explicit-flags.md)).
_Avoid_: instruction, hint, fallback (for either).

**Upstream skill**:
An Agent Skill written by a tool's own author (`binja`'s skill in the bn-mcp-extender repo,
`ipsw-skill`) that REactor fetches at install time into
`~/.reactor/skills/<tool>/` rather than writing its own. The tool's author
knows the tool best. A configured-but-unfetchable upstream skill is a warning; a
tool with no upstream skill at all is normal and simply relies on `--help`.
_Avoid_: vendored skill, external skill, third-party doc.

**Scenario**:
A multi-phase analysis workflow expressed as prompt templates, one directory per
scenario under `prompts/scenarios/`. Each scenario answers one question — the
shipped set is `acquisition`, `investigation`, `malware-analysis`,
`reverse-engineering`, `patching`, `tool-building` and `response` — so a case runs
only the phases it needs. A scenario starts from a question, which becomes the session's goal,
and its briefings say how to do a stage rather than where files are, so it needs no other scenario
to have run first. Scenarios run one at a time; what carries over is the case, including each phase's
summary, kept in the graph as a note tagged `scenario`. A case can have scenarios of its own in
`.reactor/scenarios/`, written with the agent after a triage, which win over the global and shipped
ones of the same id ([ADR-0065](docs/adr/0065-a-scenario-is-general-starts-from-a-question-and-can-be-written-for-the-case.md)). The agent advances by calling
`reactor_phase_complete`, whose *return value is the next phase's briefing*.
Scenarios steer the agent's sequencing; like everything else in REactor they
persuade rather than enforce. The person can go to any phase from the stepper at the edge of the session's transcript, between turns; that starts no turn, and their next prompt is recorded as the summary of the phase they left ([ADR-0049](docs/adr/0049-a-scenario-moved-by-hand-waits-for-the-next-prompt.md)). A scenario's stages are *phases* — distinct from
the manifest's *steps* (see goal-setting).
_Avoid_: workflow, pipeline, playbook, state machine.

**Phase briefing**:
The text returned by `reactor_phase_complete` — what the next phase is, what it
should not do yet, and which tools and skills just became relevant. It is a tool
result, not an injected message, so it costs nothing extra in context.
_Avoid_: stage prompt, step prompt.

**Spine**:
The non-optional chain every other feature builds on: catalogue → `reactor` CLI
→ registry block in the agent's system prompt. Each needs the one before it.
Used when talking about the core the rest assumes.
_Avoid_: core, base, foundation layer.

**Sibling repo**:
A standalone tool REactor develops but does not contain — its own repository,
its own release cycle, installed independently and referenced only by a
catalogue entry. They are cloned, released and installed independently,
and REactor assumes nothing about where they sit.
_Avoid_: submodule, vendored tool, subproject.

**Case**:
One investigation, held in one directory: its evidence, its analysis database and
every session about it. The case *is* the working directory; REactor's own files
sit in a hidden `.reactor/` inside it. A directory becomes a case only when the
user saves ([ADR-0045](docs/adr/0045-a-case-is-the-working-directory.md)).
_Avoid_: workspace, project, investigation folder.

**Session**:
One conversation with the agent inside a case. A case holds many. A *branch* is
a fork within one session ("try this turn again differently"); a new session is
a new conversation about the same case.
_Avoid_: chat, thread, run.

**Unsaved state**:
What REactor works in when the folder it was opened on is not a case yet: the
same layout as `.reactor/`, kept under `~/.reactor/unsaved/`. Saving moves it
into the folder and makes the folder a case; quitting asks whether to save or
delete it.
_Avoid_: temp case, draft, scratch session.

**Analysis database**:
`.reactor/analysis.db` — the case's SQLite file: session logs, change log, the
graph, the file registry and the context corpus. Written only by REactor itself
([ADR-0046](docs/adr/0046-the-analysis-database-is-sqlite-inside-the-case.md)).
_Avoid_: store (that is the session store, the API over it), tool database, case db.

**Data file**:
`reactor_data.sqlite` in the case root — the tables the agent and its scripts
create. A visible, ordinary file, queried with `sqlite3`; never the analysis
database.
_Avoid_: scratch database, working db.

**Graph**:
The case's analysis graph: what was seen, what is related, and what was
concluded. A view of the analysis, not a model of the data — absence means "not
pulled", not "does not exist"
([ADR-0047](docs/adr/0047-the-analysis-graph-is-the-case-s-model.md)).
_Avoid_: knowledge graph, memory, mental model, ontology.

**Element**:
A node of the graph, of one of five kinds: *Entity* (a thing that can appear in
many statements), *Event* (something that happened), *Observation* (something a
source reports as holding), *Annotation* (an assessment, hypothesis, note or
question), *External* (a reference to data outside the graph).
_Avoid_: object, record, item.

**Claim**:
An Event or an Observation: a statement with a source, stored as a node so it can
be dated, questioned and contradicted. What an analyst or the agent concludes is
an Annotation, not a claim.
_Avoid_: fact, finding (a finding is an Annotation), relationship, link.

**Context document**:
Reference material the user imports into a case — a wiki dump, a vendor paper, an advisory,
a report — so the agent can find what it says. Converted to Markdown, cut into passages and
indexed by keyword and, if an embedding model is chosen, by meaning. Imported by the user
only, never by the agent; its text is data, never instruction
([ADR-0046](docs/adr/0046-the-analysis-database-is-sqlite-inside-the-case.md)).
_Avoid_: knowledge base, corpus entry, attachment, evidence (evidence is a file in the workdir).

**Passage**:
One searchable piece of a context document, with the headings it sits under
(`Install > Linux`). What `docs_search` returns, quoted with where it came from.
_Avoid_: chunk (the code's word for it), snippet, excerpt.

**Reference**:
A string that names one thing of the case and that a person, the agent and an export can all
write: `graph:<id>`, `artifact:<path>#row=3`, `session:<id>#entry=7`, `doc:<id>`, `change:<n>`,
`vocab:<type>`, or a set such as `graph:?type=net.`. Its body is a key the database already
has. Inside the case it is written short; out of it, with the case's name:
`reactor://<alias>/graph/<id>`. In the composer the person triggers one with `@`
([ADR-0066](docs/adr/0066-the-case-is-addressed-by-references.md)).
_Avoid_: link, URI (a source's URL is one; a reference is the case's own), address, id.

**Artifact**:
A file of the case, named by its path relative to the case directory. Registered when the
case should keep its hashes and notice a change (`artifact_register`, the Files tab's artifacts
list); every file in the directory can be referred to as `artifact:<path>`. What the glossary elsewhere calls evidence is the
artifacts of the case.
_Avoid_: sample (a kind of artifact), upload.

**Property namespace**:
The first segment of a property's name, `time` in `time.first_seen`, which says how the value is
drawn and read: a time, a size, a reference, a command kept as written. There are eleven, one
renderer each; what follows is the writer's, so `size.unpacked` is a size without being defined.
A name with no known namespace is text. A namespace is a shape, never a kind of thing: a
file, an address or an account is an element, not a property
([ADR-0067](docs/adr/0067-a-property-is-named-in-a-namespace-that-decides-how-it-is-drawn-and-read.md)).
_Avoid_: field type, tag (a node tag outlines and clusters; a `category` property does not).

**Case alias**:
The name the person gives a case, which a reference written out of it carries instead of the
case's id (`reactor://acme/...`). Letters, digits, `-` and `_`. Renaming keeps the old ones
accepted; the id, in `case.json`, tells two cases of the same alias apart.
_Avoid_: case name (that is the title), slug.

**Pull**:
Turning rows of an External — a SQLite table, a CSV, JSON or JSONL file — into
graph elements, each linked back to where it came from. The only bulk way into
the graph; pulling the same rows twice changes nothing.
_Avoid_: import, ingest (that is for context documents), sync, load.

**Manifest**:
The session block `reactor-context` keeps in the system prompt: the user's goal,
the agent's self-maintained steps (each a conceptual summary with a 3-word
status), and the session guidelines. Injected on content only, so an untouched
session's prompt stays byte-identical. It survives every reduction — keeping it
current is how work outlives them.
_Avoid_: goal list, todo list, step list (the steps are one part of it), memory (too broad).

**Reduction**:
How the agent keeps a session inside the context window
([ADR-0037](docs/adr/0037-context-reduction-is-one-budget-manager.md)): one stand-in
message replaces a range of old entries, in mode `fade` (stubs only), `compact`
(a summary) or `auto`. The session log is never rewritten — a reduction is an
entry, and a `restore` entry undoes it.
_Avoid_: truncation, pruning, compaction (that is one mode).

**Identity**:
The working persona `reactor-context` injects into the system prompt: a built-in
(`reverse-engineer`, `cyber-forensics`, `forensics`, `software-engineer`,
`infrastructure`, `publisher`), a saved user identity, or an adhoc custom text. The
human selects it; the model never does ([ADR-0026](docs/adr/0026-identity-is-a-persona-block-in-the-system-prompt.md)).
_Avoid_: role, profile, mode.
