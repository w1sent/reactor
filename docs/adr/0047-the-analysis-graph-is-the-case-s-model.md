# The analysis graph is the case's model: claims are nodes, and history is never overwritten

The case ([ADR-0045](0045-a-case-is-the-working-directory.md)) carries an **analysis
graph** in its database
([ADR-0046](0046-the-analysis-database-is-sqlite-inside-the-case.md)): what was seen,
what is related, and what the analyst and the agent conclude. It is the shared working
model of the case, it outlives any session, and it is drawn in the GUI.

The graph is a view of the analysis, not a model of the data. Raw data stays the ground
truth, in files and tool databases. Only what is relevant is pulled in, and absence from
the graph means "not pulled", never "does not exist".

## Decisions

**Five element kinds, and nothing else is schema.** *Entity* (a thing that can appear in
many statements: an address, a host, a hash), *Event* (something that happened),
*Observation* (something a source reports as holding), *Annotation* (an assessment,
hypothesis, note or question by an analyst or the agent), *External* (a reference to
data outside the graph). Types, roles and tags are free text with suggested known
values, kept as rows in the database so they grow without a release.

**Claims are nodes; edges are structure.** Anything that can be true or false, sourced,
dated or questioned is a node. An edge only links a claim to what it involves, with a
role. There are no edges between Entities — "domain resolves to address" is an
Observation — and no edges on edges. Relations in an exploratory analysis are regularly
annotated, used as evidence and contradicted, so they must be things an edge can point
at.

**Events and Observations have a source; conclusions are Annotations.** This holds for
the agent as for a person: extracting from data creates sourced claims, concluding
creates Annotations. A finding is an Annotation, not a table of its own.

**Entity identity is deterministic.** Ids derive from normalized key fields, else from
the source location, else a stored random id for manual entries. The same thing from two
pulls lands on the same node. This is the one hard rule among the conventions, because
correlation depends on it. Scope is an optional key field the analyst states; it is
never inferred.

**Append-only through version chains.** A change creates a new version that supersedes
the last; edges point at elements and may pin a version. The state as of any moment can
be reconstructed. A revert pops versions from the top of one element's chain, blocked
when something pins them, with a preview of what a forced revert would take with it.
Soft-delete is a status, not a removal. Purge exists for data that must go.

**Two kinds of time, never mixed.** Domain time (when it happened or held) and record
time (when a version was recorded) are separate columns. Domain time keeps the UTC
value, the zone and how certain it is, and the raw string.

**Authorship on every version.** Who or what wrote it — analyst, tool, or agent with its
model — and on whose behalf. Agent output is told apart by authorship, not by a review
state, and anyone can question any item through an Annotation.

**The graph is filled by pulling or by hand.** A pull is plain SQL whose column names
follow a convention (`"@type"`, `"@time"`, `"<type>@<role>"`), run against a table in
`reactor_data.sqlite` or another SQLite file, or against a CSV, JSON or JSONL file
presented as a table. REactor runs it read-only with a timeout. It is recorded as an
Event, and pulling the same rows again changes nothing. By hand means the agent's tools
or the user in the graph view; both use the same write path and record their author.

**The invariants are enforced by the database, not only by the code.** Triggers reject
an update of a version, an edge between two Entities, and an outgoing edge from an
Entity other than its source; the write path refuses an Event or Observation without a
source. They hold against any writer, including a shell.

**Five tools for the agent.** `graph_add`, `graph_annotate`, `graph_query`,
`graph_pull` and `artifact_register`, advertised by state
([ADR-0030](0030-gated-tools-advertise-by-state.md)).

**No snapshot of the graph is put in front of the model.** A session starts with the
case's goal and guidelines and nothing else from earlier work; a summary of the graph
would only clutter the context. The user leads and says what to focus on, and the agent
queries the graph when it needs to. A short static block in the system prompt says that
a case graph exists and when to record into it. It changes only with the switch below,
so the prompt cache holds ([ADR-0006](0006-registry-injected-into-system-prompt.md)).

**Two switches decide what a session may draw on, both on by default.** *Session
history* off: the session cannot search or read other sessions' chat. *Case graph* off:
the session is detached from the graph, can neither read nor write it, and its tools are
not advertised. With both off, a session is a discussion on the case's data that leaves
the case's results untouched. Each is a setting in the cascade of
[ADR-0038](0038-settings-resolve-global-then-session.md). Neither hides the goal and
guidelines.

**The graph ships with its drawn view.** Binary claims are collapsed to a labeled line
between two entities; status, confidence and authorship are visible; tags and clusters
can be drawn as outlines; the full structure appears only where it is needed. The view
is a requirement, not an option: if gpui-kit has no component for it, it is built.

**STIX 2.1 is an export and import format**, not the model.

## Consequences

- **A simple relation costs one node and two edges**, and a traversal between entities
  takes two hops. A `relations` view presents binary claims as `(from, to, type)` rows,
  and graph algorithms run on a projection built per question. At the size of a case
  graph — bulk data stays in Externals — this is negligible.
- **The model's verbosity is the view's job.** That is why the view is mandatory: the
  stored structure is not what a person should have to read.
- **The switches are not a sandbox.** They withdraw tools and state a rule;
  `bash` is unrestricted ([ADR-0033](0033-reactor-is-a-rust-project-on-rig.md)).
- **Several limits are accepted and silent**: context-dependent identity, unpulled data,
  overlapping pulls, confidence that does not propagate along a path, contradicting
  sources. The graph flags what it can — staleness, orphans, overlap — and resolves
  nothing on its own.
- **The vocabulary is seeded, then the user's.** Starting types, normalizers, roles, the
  Words of Estimative Probability scale for confidence and the Admiralty scale for source
  reliability ship as rows. A judgement pins the version it judged; notes, clusters and
  tags do not.
- **The graph view is the largest unknown in the work** and needs its own section in the
  GUI's SPEC before it is built.

## Considered and rejected

- **STIX-shaped entities joined by typed edges, plus a findings table.** It is the
  familiar shape and exports directly. Rejected: a relation that is an edge cannot be
  sourced, dated, annotated or contradicted, which is most of what an exploratory
  analysis does to relations.
- **A custom ontology or query language.** Rejected: a standard property graph works
  with existing libraries, and conventions kept as data grow without a release.
- **Overwrite in place, with an audit log beside it.** Rejected: "what did we believe
  when we decided" becomes a reconstruction instead of a query.
- **Automatic identity resolution across contexts.** Rejected: it needs domain
  knowledge, and when it is wrong it is wrong silently.
- **Inject a summary of the graph into each new session.** Rejected: it spends context
  on what the user may not want pursued, and steers the agent before the user has.
- **A detached session that still writes, or a private graph merged later.** Rejected:
  the point of detaching is that the case's results are not touched; a merge is a
  feature of its own.
- **Ship the graph first and the view later, or a list view as a stand-in.** Rejected:
  without the drawn view the graph is visible only through the agent, and its structure
  is not meant to be read raw.
- **Scripts create claims directly.** Rejected: data enters the graph when it is
  relevant, and that is a decision made at the pull, by the agent or the user.
