# The analysis database is SQLite inside the case, and the session log lives in it

Each case ([ADR-0045](0045-a-case-is-the-working-directory.md)) has one database,
`.reactor/analysis.db`: SQLite with FTS5 and the sqlite-vec extension, compiled into the
binary. It holds the session logs, a change log, the analysis graph
([ADR-0047](0047-the-analysis-graph-is-the-case-s-model.md)), a registry of referenced
files, and a corpus of context documents searchable by keyword and by meaning.

This supersedes the store choice of
[ADR-0036](0036-reactor-owns-its-session-store-format.md): the session log moves from a
JSONL file into this database. Everything else that ADR decided — REactor's own format,
append-only, originals survive reduction, branches as parent pointers — holds.

## Why

**ADR-0036 named the condition for revisiting SQLite, and it has arrived.** It rejected
SQLite "for now, not on principle", until the query load needed it. Three needs now do.
A graph element must cite the session entry it came from, and the database must be able
to check that the entry exists. Search must reach every session of a case. And several
sessions must share one case at the same time, safely. A file per session gives none of
these.

**One file is what makes a case a unit.** Copying `.reactor/` takes the chat history,
the graph and the corpus together, consistent with each other.

**A single executable is an owner requirement.** No service, no daemon, no extension
loaded at runtime. SQLite built from source (`rusqlite`, `bundled`) and sqlite-vec
linked statically meet it; vector search runs in the same database.

## Decisions

**SQLite is the source of truth for session entries, with no JSONL mirror.** `Store`
keeps its public API and its in-memory entry list; only the storage underneath changes.
Entry ids stay dense per session, so `#id` addresses, reductions and stubs are unchanged.
The entry body is the JSON of one line of the old log, in a column. `entry` is
append-only, enforced by triggers. `index.json` disappears.

**The change log records write batches, not tool calls.** One row per pull, manual graph
change, document ingest, import or export, with the session, the entry that caused it,
and the author. Rows written by a batch carry its sequence number, which is what makes
reverting a batch possible. Tool calls and their results are not copied into it: the
session log is already that record, and two copies would be two truths.

**Only REactor's own process writes `analysis.db`.** That is the GUI or one of its
headless commands, through one writer thread per process. Several windows on one case
share the file through WAL, `BEGIN IMMEDIATE` and a busy timeout. There is no socket and
no single writer process.

**The agent's data is a second, visible file.** `<case>/reactor_data.sqlite` holds
whatever tables the agent and its scripts create. It is an ordinary file in the workdir,
openable with any SQLite tool. The agent queries it with the `sqlite3` program through
`bash` — there is no SQL tool of REactor's own. The agent does not write to
`analysis.db` with SQL; it changes it only through tools that extend the graph or the
file registry. That is a rule stated to the agent, not a wall: `bash` is unrestricted
([ADR-0033](0033-reactor-is-a-rust-project-on-rig.md)).

**Scripts write files; REactor pulls from them.** A script produces rows in
`reactor_data.sqlite`, or a CSV, JSON or JSONL file, in the workdir. Scripts that write
run on the same machine; one that runs elsewhere is responsible for getting its data
here. What enters the graph does so by a pull (ADR-0047). There is no record format and
no ingest command for scripts.

**No tables for the models of specialised tools.** REactor is no longer limited to
reverse engineering, and it does not duplicate what Binary Ninja or Ghidra hold. Those
tools are the ground truth and are better at querying their own data; the overhead of
asking them is not the bottleneck. The one structural table is the file registry:
relative path, hashes, kind, parent.

**Embeddings are remote, and the user chooses the model.** A model through rig, or an
Ollama server, named in settings as `provider/name`. There is no built-in default and no
in-process model. Until one is chosen, search is keyword-only, and documents are still
ingested and chunked so embedding can be added later. The case records which model made
its vectors; choosing another re-embeds in the background while search keeps using the
old vectors.

**Context documents are ingested by the user, in bulk.** In the GUI, or headless with
`reactor-gui ingest`. It is idempotent: an unchanged document is skipped by content
hash. It is not an agent tool. The agent reaches the corpus through one explicit search
tool, never through always-on retrieval, and every returned passage is delimited and
carries its source.

**Two new crates.** `reactor-db` holds the storage: migrations, writer, read pool, blob
store, change log, session tables, graph, pull importer, search SQL, export. It depends
on no rig, no tokio and no gpui. `reactor-ingest` converts documents and chunks them,
with no database and no rig. rig types stay at the seam in `reactor-agent`
([ADR-0041](0041-the-agent-loop-owns-the-message-list.md)), which implements the embed
worker and rig's `VectorStoreIndex` over `reactor-db`.

**One SQLite binding, checked at start.** The build contains exactly one
`libsqlite3-sys`. `reactor-gui db selfcheck` proves the extension registered
(`vec_version()`), FTS5 works, and fails with a clear message otherwise.

**Linux is Tier 1; macOS and Windows are Tier 2.** Tier 1 is built and tested against
every exit criterion. Tier 2 builds but is not tested in detail. There is no CI; checks
are run by hand. Nothing in the design may be Linux-only.

## Consequences

- **The session log is no longer readable with `grep`, `jq` and `tail -f`.** This is the
  real loss, and it was ADR-0036's reason. A JSONL export of a session and `sqlite3`
  replace it.
- **The agent test suite is the acceptance gate for the move.** It must pass on the new
  store, changed only where ADR-0045 requires.
- **The build gains C code**: SQLite, sqlite-vec, zstd. A C compiler on build machines,
  nothing on user machines. The GUI already needs one for tree-sitter
  ([ADR-0044](0044-filesystem-panels-read-through-a-vfs.md)).
- **Contention moves to `reactor_data.sqlite`**, which scripts and REactor both use. It
  is created in WAL mode; a script should set a busy timeout.
- **Nothing stops the agent writing to `analysis.db` with `sqlite3`.** Triggers reject
  changes that break the graph's invariants, and versions are append-only; beyond that
  it is accepted, as everything in REactor persuades rather than enforces.
- **A remote embedding provider sees the text of the context documents.** The settings
  screen says so; an Ollama server keeps it on the machine.
- **Lock files** in `.reactor/run/` guard one frontend per session — until now a contract
  without a lock — and one embed worker per case.

## Considered and rejected

- **Keep JSONL as the truth and mirror it into the database.** It keeps the debugging
  workflow. Rejected: every entry exists twice, and the copy the graph cites is the
  derived one.
- **One writer process, others over a local socket.** Its purpose is one place for
  validation. Rejected for now: every writer here is the same code, and only REactor
  writes `analysis.db`. It returns if contention on the data file proves real.
- **Scripts emit JSONL records that an ingest command validates and writes.** Rejected:
  it exists to let outside processes write the core database, which they no longer do.
- **A hidden scratch database attached to the core one.** Rejected: every other artifact
  of the analysis is a visible file in the workdir, and this one is the one the analyst
  most wants to open.
- **Fixed `function` and `string` tables.** Rejected: a second, worse copy of a
  specialised tool's model, for a domain REactor is no longer limited to.
- **An SQL tool for the agent.** Rejected: `sqlite3` through `bash` does it, and a tool
  costs prompt space on every request.
- **A local embedding model in the process.** It is offline by construction. Rejected:
  ONNX Runtime is the build's largest native dependency and the likeliest to fail on a
  target, and a model provider is needed anyway. An Ollama server covers offline use.
- **rig's own SQLite vector store.** Rejected: it owns its tables, searches vectors
  only, embeds at insert time, and brings a second rusqlite requirement.
- **Another engine** (SurrealDB, DuckDB, LanceDB). Rejected on licence, build weight or
  runtime extension loading, against the single-executable and not-proprietary
  requirements.
