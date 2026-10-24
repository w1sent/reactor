# The case is addressed by references: one grammar for what a person, the agent and an export point at

> Implemented, apart from what "Left for later" lists. Amends
> [ADR-0047](0047-the-analysis-graph-is-the-case-s-model.md) (the graph's write tools become
> one), and replaces the `#` and `#doc:` references of the composer. The integration interface
> (programs such as a disassembler that register a scheme) is left to a following ADR; this one
> only leaves room for it, in `reference::register_scheme`.

The case has several addresses already, each made for one place. A session entry is `#id` in a
stub and `session:<id>#entry=<n>` in a graph source. A file is a path to the tools, a `FileRef`
to the viewers, `csv:<path>#row=<n>` or `sqlite:<path>?table=<t>` to `graph_pull`, and `#path`
in the composer. A document is `#doc:<name>#<section>` in the composer and a bare id in the
database. A graph element is a hash id, or `{type, value}` for an entity. None of these can be
quoted in a chat and found again, none can leave the case in an export and be mapped back, and
three tools write to the graph with parameter sets that barely overlap. This records one grammar
for all of it.

## Decisions

**A reference is a string, `kind:rest`, and its body is a key the database already has.** There
is no registry of references and no new id column: `graph:<element id>`, `session:<session
id>#entry=<n>`, `artifact:<path>`, `doc:<document id>`, `blob:<sha256>`, `change:<n>`.
`artifact.path` is unique and is what the explorer opens, so the path is the key; the artifact's
own id stays internal. Types and roles of the vocabulary are `vocab:<type>`. Edges have no
reference: they are structure, and show as links on an element (ADR-0047). A scenario has none
either: it is a file looked up in three places and the first wins (ADR-0065), so the same id can
mean different content on another machine.

**A version is `@<n>` after a graph id, and no version means the latest.** An export writes the
pinned form, so a report that quotes a claim does not change when the claim is superseded. An
entry never changes, and needs no pin.

**An entity can be named by its natural key.** `graph:entity/net.ipv4/198.51.100.7`, with
`?scope=` when the entity has one, values percent-encoded. It resolves to the derived id
(ADR-0047), exists whether or not the entity was ever pulled, and replaces the `Target::Entity`
the tools take today, so there is one way to say it.

**A reference has two forms.** The short form is relative to the open case, and is what the
composer, the transcript, the grid and the agent write. The canonical form,
`reactor://<case>/<kind>/<rest>`, is what leaves the case: "Copy reference", an export, a report.
`parse` reads both and `Display` prints either. A canonical reference to another case parses and
resolves to "not this case"; it never opens another database.

**A case has a hidden id and an optional alias.** The id is the UUIDv7 `case.json` already
held; the alias, and the aliases the case had before, are two more fields of that file, so there
is no table and no migration. The alias is a name the person may set (`reactor-gui case alias`):
letters, digits, `-` and `_`, no spaces, up to 64, compared without regard to case. `<case>` in the canonical form is the alias when
there is one and the UUID otherwise. Renaming keeps the old aliases accepted. An export records
both in its header, so two cases that chose the same alias are told apart. The alias is shown to
the person before it is first used in an export, since a directory name can say more than they
meant.

**The place in a file is on the edge, the file is the External.** `graph_write` takes a source
as `artifact:data/a.csv#row=12` and keeps the file as the External and `row=12` as the edge's
locator (`reference::split_source`), so many rows of one file are one External and not one each.
What names the thing itself (`#table=<t>` of a database, `#/pointer` of a JSON file) stays in the
External, as `graph_pull` stores it.

**A locator follows the file's media type.** A reference to a file may end in `#<locator>`,
and what it means is decided by the file, not by the scheme: `line=`/`char=` for text (RFC 5147),
`row=`/`col=`/`cell=` for CSV and TSV (RFC 7111), a JSON pointer for JSON (RFC 6901),
`table=<t>&row=<rowid>` for SQLite and `offset=&length=` for bytes. The parser keeps the fragment
as written, so that what is hashed into an External element is deterministic; the viewer reads
it after the file has been identified. `csv:`, `sqlite:` and `json:` as schemes end. A source
for a whole table has no `row`.

**The existing spellings are replaced, not read.** `session:<id>#entry=<n>` stays as it is; the
rest become `artifact:`. An External element's id is a hash of its URI, so a new spelling is a new
id. The project is in beta and there is no case to keep: the schema version moves, old cases
are recreated, and there is no legacy parser. The external type derived from a scheme becomes
`external.artifact`, `external.session`, `external.https`.

**`@` in the composer is the person's trigger, and `#` stops being one.** `@` counts at the start
of a text or after whitespace, with something after it, as `#` did; `user@host` and `C#` are not
references. After `@` the menu lists the kinds and the files; with no kind it means files, and
`@vpn.csv` is `@artifact:data/raw/vpn.csv`. The registered files rank first. Each kind completes
from the same query as the tool (below), so a menu and a call do not drift. Choosing an item
inserts the stable reference, and when a message is sent the `@` is dropped: the stored text and
the model see `artifact:data/raw/vpn.csv`, the same as the agent writes. The log keeps the message as typed
([ADR-0061](0061-typed-input-is-a-fact-in-the-log-before-it-is-sent.md)); the `@` is dropped
when the request is made. A reference typed in full resolves too. Each resolved reference goes with the message as an attachment, cut to the
same 20,000 tokens as today, and one that names nothing is left as text. `#N` stays in stubs and
in `history_read`'s text as the entry of this session; it is not a reference and the composer
does not parse it.

**A collection is a reference with a query.** `graph:?type=net.&tag=c2&text=beacon&limit=40` is
a set, and what comes back is a page of references with a line of preview each. The filters are
the ones `graph_query find` has (`kind`, `type` prefix, `text`, `tag`, `include_deleted`), plus
`orphans` and `stale`, `text` for the other kinds and `changed` for artifacts; `limit` and
`offset` are the tool's parameters. Search of the documents' passages by meaning stays
`docs_search`'s, because it needs the embedding model the database does not have. It is not SQL. SQL
stays in the table viewer, read-only (ADR-0050), and in `graph_pull`, where it is run over the
data being brought in and not over the case.

**`case_get(uri)` reads the case, and it reads rows.** A single reference returns the element,
entry, document passage or registry row in full; a collection returns a page. `limit` and
`offset` are parameters, not part of the string, so the model never has to encode a cursor, and
the output follows ADR-0053. It never returns a file's bytes: for those the answer is a path and
the agent's `read`. The switch of the kind decides whether it answers (*Case graph*, *Documents*,
*Other sessions*), not the tool. A reference into another session is answered
under the same switch as `history_read` has for it (*Other sessions*), which is on unless
the person turned it off; a person who types one in the composer is not held to a switch.
The five reading tools stay at first and become aliases for `case_get` once it has been
watched in use; the ladder's check for a result "read back with `history_read`" and the GUI's
matches on tool names move with them.

**The graph has one tool to write, `graph_write`.** An element has a `kind` of `entity`, `event`,
`observation` or `annotation`; an annotation's `type` is one of the six, its `text` is the
value, and `about`, `supports`, `refutes` and `members` are `links` with those roles, as the
database already stores them. An element with an `id` is a new version of it, with `status`,
`add_tags`, `remove_tags` and `reason`. The rule of ADR-0047 stays, now as a check by kind: an
event or an observation is refused without a `source`, an annotation needs none, and a judgement
needs `about` and a confidence. `graph_revert` and `define_type` are their own small tools,
because they write no element. `graph_pull` stays, with `source` an `artifact:` reference.
Wherever a tool takes an id, it takes a reference.

**An export writes every reference with the case's name.** The element tables get a `reference`
column (pinned, canonical), a source is written canonical, a reference inside a text is
rewritten, and a STIX bundle carries both names of the case in `x_reactor_case`. Restoring a
bundle into another case turns the references that carried the bundle's own names back to short
ones, so the sources are what they were; those that name another case stay as written.

**A session has the reference it is shown with.** `<case id>#<n>` in the session list is
`reactor://<case>/session/<n>`; `/open`, the command line and the composer read all of
`3`, `#3`, `session:3` and the canonical form.

**A native path is not a reference, and is not turned into one.** `read`, `write`, `edit` and
`bash` keep paths. One that is given `kind:...` for a kind of this grammar answers with the tool
to use. A `reactor-gui case get <dir> <reference>` lets a pipeline resolve one.

**The grammar is closed for the case and open for programs.** A scheme registered by an
integration is a source the graph can hold as an External element, and is not scoped to a case.
How one is registered, opened and completed is the next ADR.

## Left for later

- Folding `history_index`, `history_read`, `history_search`, `graph_query` and `docs_search` into
  `case_get`. It is added beside them; they stay until it has been watched in use.
- Opening at a place in a SQLite table or a JSON file, and in the data viewer: the editor
  (a line), a delimited file's table (a row) and the hex view (bytes) do.
- The interface for programs that register a scheme (the next ADR).

## Consequences

- A `Ref` type (`parse`, `Display`, `resolve`) in a crate with no window, with round-trip tests
  for each kind. `references::find` in `reactor-agent` becomes its scanner for the composer, for
  linkifying the transcript and for rewriting short references in text to canonical on export.
- `canonical_uri` in `reactor-db` takes the new spellings and the registered schemes. The case
  gains its identity table; the schema version moves.
- `ref_index` in the GUI completes by kind. The transcript, the explorer's artifacts list, the
  graph panel and a session's tool rows get *Copy reference*; the entry id is already on a tool
  row. `/open` takes a reference.
- A viewer needs an entry to open at a locator, which none has. The grammar and *Copy reference*
  come first, and opening at a locator is its own step.
- The prompt's case-graph block, the deferred line, `RESERVED`, the scenario briefing's list of
  earlier conclusions (it prints bare ids, and is the query `graph:?kind=annotation&tag=scenario`)
  and the reporting skill (cite by reference) all change. About eighty references to the old tool
  names in the tests move.
- `CONTEXT.md` gains *Reference*, *Artifact* and *Case alias*, and says "passage", not "chunk".
- An export is the place the canonical form is checked: it holds no absolute path and no
  directory name that is not the alias.

## Considered and rejected

- **`kind://` for the short form.** The paths and ids after the slashes are read as a host, and
  the spellings in the database are already `kind:rest`. Only the canonical form has an
  authority.
- **The UUID alone as the case's name.** Unreadable in a report. The alias alone collides.
- **A format scheme per locator (`csv:`, `sqlite:`).** The viewer is chosen from the bytes, so a
  scheme could only be a hint, and a wrong one for a `.csv` that is not CSV.
- **SQL in the query string.** Hard to read and to encode, and a second way into the database
  that the read-only authorizer has to cover. The filters are the ones already in use.
- **Folding every reading tool into `case_get` at once.** It changes a prompt that works, the
  ladder and the GUI in one step. One tool first, the others after it has been used.
- **Keeping `#` beside `@`.** Two triggers for one thing, and `#` collides with headings,
  `C#` and the entry shorthand.
- **A reference kind for scenarios and a legacy parser for old spellings.** See above: the first
  is not stable across machines, the second has nothing to serve.
