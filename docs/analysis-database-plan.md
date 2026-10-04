# Cases, analysis database and analysis graph: implementation handoff

Version 4, 4 October 2026. Agreed with the project owner. Stage 0 is done on Linux (section 10.1). Stage 1 is done (section 10.2). Stage 2 is built apart from what needs the owner's eyes (section 10.3). Stage 3 is built (section 10.4). Stage 4 is built (section 10.5).

## 1. Purpose and how to use this document

Your task is to add three things to REactor:

- A **case**: one directory that holds an investigation — its evidence, its database and all its sessions.
- An **analysis database**: one SQLite file per case, compiled into the binary, holding the session logs, a change log, the graph, a registry of referenced files and a searchable corpus of context documents.
- An **analysis graph** in that database, with a drawn view in the GUI: the shared working model of the case for the user and the agent, which outlives any single session.

This document is the integration plan. It was worked out with the owner decision by decision.

| Part | Status |
|---|---|
| Section 3, owner requirements | Binding. |
| Section 4, decisions D1 to D16 | Binding. Agreed with the owner; do not reopen without asking. |
| Sections 5 to 8, target design | The agreed shape. Table and column names, SQL and command names are reference designs: keep the invariants, adjust names to the code. |
| Section 10, stages | The agreed order and exit criteria. |
| Section 12, unverified points | Things this plan assumes but nobody has checked. Check them before relying on them. |

Two input documents sit next to this file. They are local and not tracked in git:

- `analysis-database-handoff.md` ("the handoff"): the database design. Its Requirements section is binding; the rest is advice, and section 9 lists where this plan departs from it.
- `analysis-graph-concept.md` ("the graph concept"): the graph model. It is the model to implement; section 9 lists the few departures.

Read both in full before starting. Where they conflict with this document, this document wins.

**Start with Stage 0** (section 10). Write the ADRs before implementing; that is the repo's rule (`docs/adr/README.md`).

## 2. REactor today: what you are changing

Read `CONTEXT.md` (glossary), `README.md` (layout) and the ADRs named below. The facts this plan builds on:

| Area | Today | Where |
|---|---|---|
| Session store | Append-only `session.jsonl` per session in `~/.reactor/sessions/<id>/`, a derived `index.json`, a `blobs/` folder for truncated tool output. Entry ids are dense per session; branches are parent pointers | `crates/reactor-agent/src/store.rs`, `entry.rs`; ADR-0036 |
| The loop | `Agent` owns the message list. Every entry passes through `Agent::append`; tools run in `run_tools` | `crates/reactor-agent/src/agent.rs`; ADR-0041 |
| Session state | Manifest, identity, reporting, scenario and context settings are `custom` entries in the log; the latest on the branch wins | `crates/reactor-agent/src/context.rs` (`SessionState`) |
| Manifest | Goal, guidelines and the agent's steps, all per session | `crates/reactor-context/src/manifest.rs` |
| System prompt | Rebuilt on every request from pure functions, in a fixed order, so unchanged state gives identical bytes and the provider's prompt cache holds | `crates/reactor-agent/src/prompt.rs`; ADR-0006 |
| Tools | A `Tool` trait and a `Tools::standard` list. A tool is advertised according to session state. There is no permission or approval layer | `crates/reactor-agent/src/tools/mod.rs`; ADR-0030, ADR-0033 |
| Model seam | rig 0.42 (`rig-core`) is used only as the provider layer, behind the `Llm` trait | `crates/reactor-agent/src/llm.rs`, `provider.rs` |
| Settings | Built-in default, then `~/.reactor/settings.json`, then a session override | `crates/reactor-context/src/settings.rs`; ADR-0038 |
| GUI | Hosts the agent in process on its own tokio runtime. One session per window. Starts on a workdir; `-c` continues the newest session for it, `-r` shows a picker | `crates/reactor-gui/src/backend.rs`, `start.rs`, `main.rs`, `SPEC.md` §3; ADR-0042 |
| `reactor` CLI | Catalogue commands only. The agent uses it through `bash` | `crates/reactor-cli`, `crates/reactor-core` |
| Files | `reactor-fs` has a virtual filesystem seam, hashing (SHA-256, SHA-1, MD5) and file identification. The GUI has explorer, editor, table, hex and data viewers | `crates/reactor-fs`; ADR-0044 |
| Build | No SQLite binding is in `Cargo.lock`. `reactor-gui` is not a default workspace member (`cargo build -p reactor-gui`). There is no CI | `Cargo.toml` |

## 3. Owner requirements (binding)

From the handoff, unchanged:

- SQLite with the sqlite-vec extension is the one database engine; vector search runs in the same database.
- Single executable: the database is compiled into the binary. No separate service, daemon, runtime or dynamically loaded extension.
- Local and offline: everything runs on the user's machine; network use is optional.
- Not proprietary: open-source components with permissive licences, an open on-disk format, exportable data.
- Rust, with rig for AI.
- A central store for the agent: artifacts, events and analysis data, reusable and transferable.
- Context documents are stored, searchable by meaning, and usable for retrieval.
- Context documents can be converted and written into the database.
- Data can be imported from other databases.
- The agent can insert data through scripts it writes.
- One database and one directory per case.

## 4. Decisions

All decided on 3 October 2026.

### D1. The graph concept is the data model for analysis results

- The handoff's `entity`, `edge` and `finding` tables are not built. They use entity-to-entity edges, which the graph concept forbids.
- A finding is an Annotation. STIX 2.1 is an export and import format only, using the mapping table in the graph concept.
- The handoff's `origin` label is covered by authorship plus the source reference. Content that came from a sample must still be presented to the model as untrusted data.

### D2. The session log lives in SQLite

- `analysis.db` is the source of truth for session entries. There is no JSONL file and no mirror. This supersedes ADR-0036's choice of store.
- `Store` keeps its public API and its in-memory entry list; only the storage underneath changes. Entry ids stay dense per session, so `#id` addresses, reductions and stubs are unchanged.
- In-session branching stays, alongside multiple sessions per case. A branch is "try this turn again differently"; a session is a new conversation about the same case.
- Reason: graph elements must cite session entries, search must reach every session of a case, and several sessions must share one case safely.
- Cost: the log is no longer readable with `grep` and `jq`. A JSONL export of a session replaces that.

### D3. A case is the working directory, created by saving

- One case is one directory, the workdir. Evidence in several folders goes into subfolders; a case never refers to local files outside its root.
- REactor's own files live in the hidden `<case>/.reactor/` (already git-ignored; it may also hold the project-scoped `state.json` of ADR-0003).
- Opening REactor in a folder that is not a case gives an **unsaved state**, stored in `~/.reactor/unsaved/<id>/` with the same layout as `.reactor/`. It is fully usable and survives a crash.
- The user saves it, which moves it into the folder and makes the folder a case. On quit with an unsaved state, the GUI asks: save as case, or delete.
- Files the agent wrote into the workdir are ordinary files and stay either way.
- Consequence: the contents of `.reactor/` are moved as a unit, so **nothing inside may store an absolute path**. Today's `session` entry stores an absolute `cwd`; that must go.

### D4. No socket, no single writer process

- `analysis.db` is written only by REactor's own process (the GUI, or a headless `reactor-gui` command), through its write path. No outside process writes it.
- Each REactor process has one writer thread. Several windows on one case share the file through WAL, `BEGIN IMMEDIATE` and `busy_timeout`.
- Contention is expected only on `reactor_data.sqlite`, which scripts and REactor both use. REactor creates it in WAL mode.
- Scripts that write run on the same machine with access to the case folder. A script or tool that runs elsewhere is responsible for getting its data onto REactor's machine.
- Lock files in `.reactor/run/` guard two things: one frontend per session, and one embed worker per case.
- The handoff's socket, IPC server and `interprocess` crate are deferred until contention is observed.

### D5. Agent data is a visible file; no fixed function or string tables

- The agent's free-form tables live in `<case>/reactor_data.sqlite`, in the root of the workdir. It is an External like any other file. Other SQLite files in the case may be used the same way.
- The agent does not write to `analysis.db` with SQL. It changes it only through tools that extend the graph or the file registry.
- The handoff's `function` and `string_item` tables are dropped. REactor is no longer limited to reverse engineering and does not duplicate the models of specialised tools. Those tools are the ground truth and are better at querying their own data; the overhead of asking them is not the bottleneck.
- The handoff's `artifact` table stays, narrowed to a registry of referenced files.

### D6. The graph is filled by pulling or by hand

- The handoff's JSONL record format, `db ingest`, `db schema`, `db undo` and the `schemars` dependency are dropped.
- **Pulling:** a script produces data in the workdir; the agent turns the relevant rows into graph elements with `graph_pull`. Sources are SQLite tables, CSV files, and JSON or JSONL files (section 7.4).
- **By hand:** the agent through `graph_add` and `graph_annotate`, the user in the graph view. Both use the same write path and record their author.
- An Event or Observation added by hand still needs a source (a file, a row, a session entry). Conclusions are Annotations.
- The requirement "insertion by scripts" is met by scripts writing those files.

### D7. Automation runs through headless `reactor-gui` commands

| Command (names are placeholders) | Does |
|---|---|
| `reactor-gui case create <dir> [--title …]` | creates a case in a folder without opening a window |
| `reactor-gui ingest <dir> <files, folders or URLs…>` | bulk-ingests context documents: convert, chunk, embed |
| `reactor-gui case list` / `case export` / `case import` | case management |
| `reactor-gui db selfcheck` | the build self-check of Stage 0 |

- The GUI offers the same bulk import interactively, with progress.
- Subcommands are dispatched before the window stack starts, so they work over SSH and in scripts with no display.
- Bulk ingest is idempotent: an unchanged document is skipped by content hash.
- **The `reactor` CLI is not touched.** It is the agent's tool for looking into the catalogue through `bash`. Nothing about cases or the database is added to it, and `reactor-cli` gains no database dependency.

### D8. Embeddings are remote, and the user chooses the model

- The embedding model is a remote one through rig, or an Ollama server. No in-process model: `rig-fastembed` and ONNX Runtime are not added.
- The user chooses the model in settings, in the same `provider/name` form as the chat model. There is no built-in default.
- Until a model is chosen, search is keyword-only (FTS5). Documents are still ingested and chunked, so embedding can be added later without re-importing.
- The case records which model produced its vectors. Choosing another model re-embeds in the background; search keeps using the old vectors until the new set is complete.
- No code-specific model for now.
- Expected scale: at most about 100 documents of 100 to 1000 pages. Latency is measured at 100k chunks and, as a worst case, at 1M chunks.

### D9. Goal and guidelines belong to the case

- The goal and the guidelines are the strategic part of a case and are the same in every session of it. The agent's steps stay per session.
- They are stored in `analysis.db` as versioned rows with author and time, not in the session log. Branching a session does not branch the goal.
- The manifest block in the prompt is composed from the case's goal and guidelines plus the session's steps.
- `/goal` and `/guidelines` write to the case. Another open session of the same case sees the change on its next request.
- The memory switches (D12) do not hide them.

### D10. Files are referenced in place

- Anything that exists as a file in the workdir (samples, dumps, captures, unpacked output, scripts, reports) is referenced by relative path plus SHA-256. Nothing is copied or moved.
- `.reactor/blobs/` holds only content that has no file: the full text of truncated tool output, and fetched web pages with their normalised Markdown.
- A referenced file can change behind the database's back. The stored hash detects it, and the reference is flagged stale.

### D11. Old sessions are left behind

REactor is in alpha. Sessions under `~/.reactor/sessions/` are not read, imported or migrated. The JSONL store code is removed.

### D12. Two memory switches, both on by default

Each is a setting in the usual cascade: global default, then session override.

| Switch | Off means |
|---|---|
| Session history | the session cannot search or read other sessions' chat history |
| Case graph | the session is detached from the graph: it can neither read nor write it. The graph tools and `artifact_register` are not advertised |
| Documents (added later, same cascade, key `documents`) | `docs_search` is not advertised, and a call that still comes is refused with the reason. The user's own Documents panel is unaffected |

- With both off, a session is a discussion on the case's data that leaves the case's results untouched.
- This is not a sandbox. `bash` stays unrestricted, so the switch withdraws tools and states a rule; it does not make writing impossible.

### D13. A session starts without a snapshot of the graph

- No summary of the graph is put into a new session; it would clutter the context. A session sees the case's goal and guidelines and nothing else from earlier work.
- The user leads and says what to focus on. Open hypotheses and everything else are queried through `graph_query` when needed.
- A short static block in the system prompt tells the agent that a case graph exists and when to record into it. It is omitted when the case-graph switch is off.

### D14. Six agent tools, no SQL tool, no ingest tool

- The tools are `graph_add`, `graph_annotate`, `graph_query`, `graph_pull`, `artifact_register` and `docs_search` (section 8.1).
- There is no SQL tool. The agent queries `reactor_data.sqlite` with the `sqlite3` program through `bash`; `sqlite3` is already a catalogue entry.
- Ingesting context documents is not an agent tool. Only the user triggers it, in the GUI or headless.

### D15. The graph ships with its drawn view

- The graph and its GUI view are delivered in the same stage. The graph is not released as something visible only through the agent.
- The drawn view is a must. There is no fallback to a list or table. If gpui-kit has no suitable component, the view is built.

### D16. Export is an encrypted archive; verification is by hand

- `reactor-gui case export` writes the whole case folder into one ZIP with AES-256. The password is `infected` unless one is given. The default stops accidental execution and scanner quarantine; a provided password gives confidentiality.
- There is no CI. The build self-check is run by hand.
- **Platforms:** REactor supports Linux, Windows and macOS as hosts. Linux is Tier 1; macOS and Windows are Tier 2. Nothing in the design may be Linux-only: paths, lock files, atomic renames and the bundled C builds must work on all three (see the handoff's Windows notes).
- **What the tiers mean (owner's definition):** Tier 1 is built and tested; every stage's exit criteria are met on Linux. Tier 2 builds but is not tested in detail: macOS and Windows must compile, and the exit criteria are not run there.

## 5. Case

### 5.1 Layout

```
<case>/                         the workdir; evidence and all Externals live here
  reactor_data.sqlite           the agent's data tables (created on first use)
  .reactor/
    case.json                   id, title (for listings), created, format version
    analysis.db                 plus -wal and -shm while open
    blobs/sha256/ab/cd/<sha256>[.zst]
    sessions/<id>/activation.json
    run/                        lock files; safe to delete
```

`activation.json` stays a file because `reactor-core` reads it (`Paths::session_state`) and must not depend on the database.

### 5.2 Lifecycle

| Action | Trigger | Effect |
|---|---|---|
| Open a folder that is not a case | GUI start | an unsaved state in `~/.reactor/unsaved/<id>/`, keyed to the folder |
| Reopen that folder | GUI start | offers to continue its unsaved state |
| Save as case | the user, in the GUI | moves the unsaved state to `<case>/.reactor/`, writes `case.json` |
| Create a case headless | `reactor-gui case create <dir>` | creates `.reactor/` directly, with an empty database |
| Open a case | GUI start, headless command | opens the database, runs the self-check, takes no global lock |
| New session | GUI | a `session` row; sees the case's goal and guidelines; empty steps |
| Resume | `-c`, `-r`, `--session <id>` | the picker lists the case's sessions from the database |
| Quit with an unsaved state | GUI | asks: save as case, or delete |
| Export, import | `reactor-gui case export` / `import` | section 5.3 |

### 5.3 Export and import

- Export writes the workdir files and `.reactor/` into one AES-256 ZIP, with a consistent copy of the database (`VACUUM INTO`).
- Import unpacks into an empty folder, which is then a case.

### 5.4 Vocabulary to add to `CONTEXT.md`

- **Case** — one investigation: a directory, its database and its sessions. Avoid: workspace, project.
- **Session** — one conversation with the agent inside a case.
- **Unsaved state** — a case that has not been saved into its folder yet.
- **Analysis database** — `analysis.db`. Avoid: store (that is the session store), tool database.
- **Data file** — `reactor_data.sqlite`.
- **Graph**, **Element**, **Claim**, **External**, **Pull** — as defined in the graph concept.

## 6. Crates

| Crate | Change | Holds | Must not depend on |
|---|---|---|---|
| `reactor-db` | new | migrations, writer thread, read pool, blob store, change log, session tables, graph, pull importer, search SQL, export | rig, tokio, gpui |
| `reactor-ingest` | new | document converters (Markdown, HTML, PDF, DOCX, MediaWiki XML) and chunking | the database, rig |
| `reactor-agent` | changed | `Store` over `reactor-db`; the six tools; embed worker; rig's `VectorStoreIndex` | — (rig stays confined here) |
| `reactor-context` | changed | manifest split into a case part and a session part (D9); new settings for the embedding model and the two switches | — |
| `reactor-gui` | changed | case start flow, save and quit prompts, session picker per case, switches, graph view, bulk import, headless subcommands | — |
| `reactor-fs` | reused | hashing and file identification | — |
| `reactor-core`, `reactor-cli` | unchanged | — | — |

- Storage code imports no rig types. rig releases breaking versions often; the handoff's reasoning on this holds.
- Cargo features in `reactor-db`, off by default: `parquet`, `import-sql`.
- `crates/reactor-agent/examples/repl.rs` creates sessions under `~/.reactor/sessions/`; adapt it to an unsaved state or remove it.

Libraries, relative to the handoff's suggestions:

| Suggestion | Decision |
|---|---|
| `rusqlite` (`bundled`, `hooks`, `backup`, `limits`), `sqlite-vec`, `rusqlite_migration`, `zstd`, `uuid`, `text-splitter`, `pulldown-cmark`, `htmd`, `scraper`, `pdf-extract`, `zip`, `quick-xml` | as suggested |
| `sha2`, `md-5`, `sha1`, `infer` | reuse through `reactor-fs` |
| `clap`, `tempfile`, `serde`, `csv`, `reqwest` with rustls | already in the workspace or lock file |
| `r2d2_sqlite` | replaced by a small hand-rolled pool, to avoid a second crate pinning the rusqlite version |
| `interprocess`, `schemars` | dropped (D4, D6) |
| `rig-fastembed`, `fastembed` | dropped (D8) |
| `rig-sqlite` | not used, for the handoff's own reasons |
| `petgraph` | deferred; recursive SQL and a projection export cover the first needs |
| `arrow`, `parquet`, `sqlx` | behind features, built on demand |

## 7. Data model

All DDL is a reference design for migration 1. Timestamps are RFC 3339 UTC unless noted. Connection settings are the handoff's: `journal_mode=WAL`, `foreign_keys=ON`, `synchronous=NORMAL`, `busy_timeout=5000`; `query_only=ON` on read connections; sqlite-vec registered before the first connection opens.

### 7.1 Sessions and case state

```sql
CREATE TABLE session (
  id TEXT PRIMARY KEY, title TEXT,
  format_version INTEGER NOT NULL, created_at TEXT NOT NULL
);
CREATE TABLE entry (
  session_id TEXT NOT NULL REFERENCES session(id),
  id INTEGER NOT NULL,                 -- dense per session, as today
  parent INTEGER, ts INTEGER NOT NULL, -- unix ms, as today
  type TEXT NOT NULL,                  -- user|assistant|tool_result|custom|reduction|restore|label
  body TEXT NOT NULL,                  -- the entry's JSON, as one line of today's log
  blob_sha256 TEXT REFERENCES blob(sha256),
  PRIMARY KEY (session_id, id)
);
CREATE TABLE case_manifest (            -- D9; the highest ver per field is current
  field TEXT NOT NULL,                  -- goal|guidelines
  ver INTEGER NOT NULL, value TEXT NOT NULL,
  recorded_at TEXT NOT NULL, author_id INTEGER NOT NULL REFERENCES author(id),
  PRIMARY KEY (field, ver)
);
```

- `entry` is append-only, enforced by triggers. `entry_fts` (FTS5) indexes the model-visible text for search across sessions.
- There is no `cwd` column and no absolute path anywhere (D3).
- A tool result's `Blob { file, bytes }` becomes a blob hash.
- `index.json` disappears: the session tree and `history_index` read the table.

### 7.2 Change log

The handoff's `event` table, narrowed. One row per write batch (pull, manual graph change, document ingest, import, export), with `session_id`, `entry_id` and author; append-only by trigger.

- Tool calls and results are **not** copied into it. The `entry` table is already that record.
- Rows written by a batch carry the event's `seq`, which is what makes reverting a batch possible.

### 7.3 Graph and file registry

```sql
CREATE TABLE author (id INTEGER PRIMARY KEY, kind TEXT NOT NULL,  -- analyst|tool|agent
                     identity TEXT NOT NULL, version TEXT, UNIQUE(kind, identity, version));
CREATE TABLE element (id TEXT PRIMARY KEY, kind TEXT NOT NULL,    -- entity|event|observation|annotation|external
                      type TEXT NOT NULL);
CREATE TABLE element_version (
  element_id TEXT NOT NULL REFERENCES element(id), ver INTEGER NOT NULL,
  recorded_at TEXT NOT NULL, author_id INTEGER NOT NULL REFERENCES author(id),
  on_behalf_of INTEGER REFERENCES author(id), event_seq INTEGER REFERENCES event(seq),
  status TEXT NOT NULL DEFAULT 'active',     -- active|conflicting|soft-deleted
  confidence TEXT, value TEXT, props TEXT NOT NULL DEFAULT '{}', tags TEXT NOT NULL DEFAULT '[]',
  occurred_at TEXT, duration_ms INTEGER, valid_from TEXT, valid_to TEXT,
  tz TEXT, tz_certainty TEXT, time_raw TEXT, time_precision TEXT,
  PRIMARY KEY (element_id, ver)
);
CREATE TABLE edge (id TEXT PRIMARY KEY, from_id TEXT NOT NULL REFERENCES element(id),
                   to_id TEXT NOT NULL REFERENCES element(id));
CREATE TABLE edge_version (edge_id TEXT NOT NULL REFERENCES edge(id), ver INTEGER NOT NULL,
  role TEXT NOT NULL, pinned_ver INTEGER, locator TEXT, fingerprint TEXT,
  recorded_at TEXT NOT NULL, author_id INTEGER NOT NULL, event_seq INTEGER,
  PRIMARY KEY (edge_id, ver));
CREATE TABLE known_type (name TEXT PRIMARY KEY, kind TEXT, definition TEXT,
                         normalizer TEXT, key_fields TEXT, stale_after_days INTEGER);
CREATE TABLE type_alias (alias TEXT PRIMARY KEY, name TEXT NOT NULL);
CREATE TABLE known_role (name TEXT PRIMARY KEY, pair TEXT, direction TEXT);
CREATE TABLE artifact (                 -- registry of referenced workdir files (D5, D10)
  id TEXT PRIMARY KEY, path TEXT NOT NULL,          -- relative to the case root
  sha256 TEXT NOT NULL, sha1 TEXT, md5 TEXT, size INTEGER NOT NULL, mime TEXT,
  kind TEXT, parent_id TEXT REFERENCES artifact(id),
  attrs TEXT NOT NULL DEFAULT '{}', registered_at TEXT NOT NULL,
  event_seq INTEGER REFERENCES event(seq)
);
```

- **Current state** is the highest `ver` per element or edge: views `element_current`, `edge_current`. **As-of** is the highest `ver` with `recorded_at <= t`.
- **Hard invariants.** By trigger: no update of a version row; no edge whose two ends are Entities; an Entity's only outgoing edge is `source` to an External. By the write path, inside the transaction: an Event or Observation is not committed without a `source` edge. Record time and domain time are separate columns and never substituted.
- **Identity** is computed in Rust before the write: hash of normalized key fields, else hash of the source location, else a stored random UUID. Key fields are immutable.
- **Conventions are data.** Types, aliases and roles are rows. A normalizer is a named built-in function that a `known_type` row refers to.
- **Views.** `relations (from_id, to_id, type, claim_id)` for binary claims; a projection query for export to graph libraries.
- **Revert.** Pops versions from the top of an element's chain, per element or per change-log batch, with the pin rules and the cascade preview of the graph concept.
- **Purge, staleness, orphans.** As in the graph concept. Staleness and orphans are computed by queries, not stored.
- **Registry and Externals.** An External element refers to a file by URI; the `artifact` row holds the file's hashes and kind. A file whose hash no longer matches is flagged stale.

### 7.4 Externals, URIs and pulling

URIs are relative to the case root. Local resources must be inside it; no `..`, no `file:` scheme, no credentials.

| Form | Points at |
|---|---|
| `<relative path>` | a file in the case; fingerprint is its SHA-256 |
| `sqlite:reactor_data.sqlite?table=<t>` | a table in the data file |
| `sqlite:<path>?table=<t>` | a table in another SQLite file in the case |
| `sqlite:?table=<t>` | a table in `analysis.db` itself (reserved form) |
| `csv:<path>#row=<n>` | a CSV row |
| `jsonl:<path>#line=<n>`, `json:<path>#<pointer>` | a JSONL line, a JSON element |
| `blob:<sha256>` | content in `blobs/` that has no file |
| `session:<id>#entry=<n>` | a session entry, so a claim taken from tool output cites it exactly |
| `binja:…`, `https://…`, `smb://…` | kept as links; a resolver is added per need |

Pulling uses the SQL alias convention of the graph concept for every tabular source:

| Source | How the pull query sees it | Row locator | Fingerprint |
|---|---|---|---|
| SQLite table | attached read-only, queried directly | the `@row` column | hash of the row |
| CSV file | a temporary table `src`, one column per header | row number | hash of the line |
| JSONL file | a temporary table `src`; top-level keys are columns, nested values stay JSON | line number | hash of the line |
| JSON file | as JSONL, over the array selected by a JSON pointer | array index | hash of the element |

- The pull query is plain SQL in every case: `SELECT ts AS "@time", src_ip AS "net.ipv4@src" … FROM src`.
- REactor runs the pull query on a connection restricted by SQLite's authorizer (read-only, no `ATTACH`, no pragmas, no extension loading) with a progress handler as timeout. This is the one place the handoff's authorizer design is used.
- Each pull is recorded as an `analysis.pull` Event with its query and External. Re-pulling the same rows changes nothing.
- An overlap with an earlier pull from the same External is warned about, not resolved.

### 7.5 Graph vocabulary: starting values

Seeded by migration 1 as rows, editable afterwards. The owner expects types and roles to be extended or modified later.

- **Entity types.** `net.ipv4`, `net.ipv6`, `net.domain`, `net.url`, `net.email`, `file.hash.md5`, `file.hash.sha1`, `file.hash.sha256`, `file.name`, `file.path`, `host.name`, `account.user`, `process.name`, `process.instance`, `registry.key`.
- **Annotation types.** `analysis.hypothesis`, `analysis.judgement`, `analysis.note`, `analysis.cluster`, `analysis.same_as`, `analysis.search_negative`. **Event type:** `analysis.pull`.
- **Normalizers.** Lowercase for domains and hashes; canonical form for IP addresses, with IPv4-mapped IPv6 turned into IPv4; trailing dot removed from domains. Unknown types match on the exact string.
- **Age thresholds.** Only where clearly needed; none on hashes.
- **Roles.** `src`/`dst`, `actor`/`target`, `parent`/`child`, `subject`/`object`, `query`/`answer`, `cause`/`effect`, `basis`, `about`, `supports`, `refutes`, `member`, `candidate`, `source`. Drawing direction comes from the pairs `src → dst`, `query → answer`, `subject → object`, `parent → child`, `cause → effect`.
- **Confidence.** Words of Estimative Probability: almost no chance, very unlikely, unlikely, roughly even, likely, very likely, almost certain.
- **Source reliability.** Admiralty scale, A to F, on Externals.
- **Pinning.** A judgement pins the version of the claim it judged and is flagged possibly stale when that claim changes. Notes, clusters and tags do not pin.
- **Disjunctions.** `candidate` roles are stored, shown as uncertain, and excluded from projections by default.

### 7.6 Context corpus and blobs

As in the handoff: `blob`, `document`, `chunk`, `chunk_fts`, `embedding_model`, one `chunk_vec_N` per model, `embedding_status`, the hybrid search query with reciprocal rank fusion, and all its invariants on blobs and chunks. Two differences: the blob store holds only content that has no file (D10), and vectors come from a remote or Ollama model (D8).

## 8. Agent and GUI integration

### 8.1 Tools

| Tool | Does | Stage |
|---|---|---|
| `graph_add` | adds Entities, Events and Observations with their sources | 2 |
| `graph_annotate` | adds a hypothesis, judgement, note, cluster or negative search; changes status; reverts | 2 |
| `graph_query` | elements around an entity, evidence for a hypothesis, `relations`, as-of | 2 |
| `graph_pull` | runs a pull against a SQLite table, CSV, JSON or JSONL file | 2 |
| `artifact_register` | adds a workdir file to the registry: path, hashes, kind, parent | 2 |
| `docs_search` | returns ranked passages from the context documents, by keywords and meaning | 3 |

- Add them to `Tools::standard`; `ToolCtx` gains a handle to the case. Advertise them by state, following ADR-0030: the graph tools and `artifact_register` only while the case-graph switch is on.
- No approval step (ADR-0033). Protection is append-only versions, database triggers and revert.
- Every retrieved passage is wrapped in clear delimiters with its source. `docs_search` is an explicit tool, not always-on retrieval.
- `history_search` gains the ability to search all sessions of the case, only while the session-history switch is on.
- Each tool costs prompt tokens on every request; do not add more without a reason.

### 8.2 Session log, authorship, prompt

- **Capture.** With `Store` on SQLite, `Agent::append` records everything; add no new call sites.
- **Concurrency.** A dedicated `std::thread` owns the read-write connection and takes commands over a channel, as `Backend` already does for the agent. `Store` stays synchronous behind its `Mutex`.
- **Authorship.** Every graph write records the author: the agent with model name and session, the analyst, or a tool with its version. `on_behalf_of` is the analyst when the agent acts on a prompt.
- **Prompt.** The manifest block takes the goal and guidelines from the case and the steps from the session. The static case-graph block (D13) is added in the fixed order. Neither changes unless its input changes, so the prompt cache holds.
- **Scripts.** They run as plain processes from the persistent `bash` tool, in the workdir, and write files there.

### 8.3 GUI

- **Start flow.** A folder is a case, has an unsaved state, or gets a new unsaved state. Save as case; quit prompt for an unsaved state. Session picker over the case's sessions.
- **Settings.** The embedding model, with a note that document text is sent to that provider; the two memory switches, with their origin shown as for every other setting.
- **Graph view (Stage 2).** Drawn as the graph concept's rendering section describes: binary claims collapsed to labeled lines, status and confidence visible, group outlines for tags and clusters, progressive disclosure, manual add, annotate, soft-delete, find source. Write a SPEC section before building it.
- **Data tables.** `reactor_data.sqlite` tables open in the existing table viewer.
- **Bulk import (Stage 3).** Choose files or a folder, watch progress.
- Update `crates/reactor-gui/SPEC.md` §3 (session model) for cases.

## 9. Departures from the input documents

The handoff asks that each departure be recorded with its reason.

| From | Original | Here | Reason |
|---|---|---|---|
| Handoff | `entity`, `edge`, `finding` tables | the graph concept's model | D1 |
| Handoff | `function`, `string_item` tables | dropped | D5 |
| Handoff | `event` records every tool call | change log of write batches only; the session log is the record of tool calls | one truth, section 7.2 |
| Handoff | artifacts moved into the blob store | files referenced in place | D10 |
| Handoff | `analysis.db` and `scratch.db` in `<case>/` | `analysis.db` in `.reactor/`; data in a visible `reactor_data.sqlite` | D3, D5 |
| Handoff | single writer process, socket, `db` CLI | direct access; headless `reactor-gui` commands | D4, D7 |
| Handoff | JSONL record format, script ingest stage | pull from SQLite, CSV, JSON, JSONL | D6 |
| Handoff | agent tools `db_query`, `db_ingest_document`, `db_ingest_records`, `db_get_artifact`, `db_record_finding`, `db_upsert_entity`, `db_link`, `db_get_neighbors` | the six tools of section 8.1 | D14 |
| Handoff | local embedding model behind a feature | remote or Ollama, chosen by the user | D8 |
| Handoff | release binaries built and checked in CI | checked by hand | D16 |
| Graph concept | root is the folder containing the SQLite file | root is the case folder, one level above `.reactor/` | D3 |
| Graph concept | storage layout left open | section 7.3 | — |

## 10. Stages

| # | Delivers | Done when |
|---|---|---|
| 0 | Three ADRs: the case; the analysis database (superseding ADR-0036's store); the graph model. Glossary entries in `CONTEXT.md`. Build spike: `rusqlite` bundled plus `sqlite-vec` in `reactor-gui`, with `reactor-gui db selfcheck` (`vec_version()`, an FTS5 round trip, `PRAGMA compile_options`). Rendering spike for the graph view in gpui | The owner has accepted the ADRs. The self-check passes on Linux, run by hand, with no SQLite installed. (Building on macOS and Windows is set aside for now; see 10.1.) The build has exactly one SQLite binding. The rendering spike says how the graph view will be built and roughly what it costs |
| 1 | `reactor-db` core: migrations, writer, pool, blob store with `fsck`, change log. Case directory and unsaved state. `Store` over SQLite; JSONL store removed. Case-level goal and guidelines. GUI start flow, save, quit prompt, picker. Session-history switch and search across sessions. Headless `case create` and `case list`. JSONL export of a session; backup with `VACUUM INTO` | The existing `reactor-agent` tests pass on the new store, changed only where D9 and D11 require. A kill mid-turn leaves a resumable session. Two sessions run in one case at once without lock errors. An update or delete on `entry` fails. Saving an unsaved state and reopening the case loses nothing |
| 2 | Graph tables and triggers, identity, version chains, revert with cascade preview, staleness queries, pull from SQLite, CSV, JSON and JSONL, resolvers, the file registry, `reactor_data.sqlite`, the five graph and registry tools, the case-graph switch and prompt block. **The drawn graph view** with manual editing; data tables in the table viewer | A re-pull changes nothing. Each hard invariant is rejected. As-of returns an earlier state. A pull query that tries to write is denied. In the view, the user can add a relation, annotate it, soft-delete it and find its source. With the switch off, the graph tools are absent |
| 3 | Corpus tables, `reactor-ingest`, embed worker, hybrid search, rig `VectorStoreIndex`, `docs_search`, a rebuild-indexes command; bulk import in the GUI and headless | Re-ingesting an unchanged document writes nothing. Switching the model re-embeds in the background without breaking search. With no model chosen, keyword search works. A small labelled query set returns the expected chunks. Latency is measured at 100k and at 1M chunks |
| 4 | Case export and import as an encrypted archive. STIX 2.1 export and import, CSV, projection export. On demand: importers from other databases, Parquet, further adapters | Export then import gives the same element and edge ids and counts. The STIX bundle validates against the 2.1 schemas |

- **Stage 1 is the commitment.** It changes where every session lives; the rest is additive.
- **The graph view is the largest unknown.** Stage 0's rendering spike exists to size it before Stage 2 is planned in detail.
- Record binary size at the end of each stage, as the handoff asks.

### 10.1 Stage 0 results (4 October 2026)

| Item | Result |
|---|---|
| ADRs | 0045 (case), 0046 (analysis database), 0047 (graph), accepted by the owner. ADR-0036 carries a superseded-in-part note |
| Glossary | Case, Session, Unsaved state, Analysis database, Data file, Graph, Element, Claim, Pull added to `CONTEXT.md` |
| Build spike | New crate `crates/reactor-db`: `rusqlite` 0.40.2 (`bundled`), `sqlite-vec` pinned to `=0.1.9`. Extension registered once as an auto extension (`sqlite.rs`). SQLite 3.53.2 |
| Self-check | `reactor-gui db selfcheck` checks the SQLite version, `vec_version()`, `ENABLE_FTS5` in the compile options, an FTS5 round trip, a `vec0` nearest-neighbour query and the JSON functions. Passes on Linux, with the display variables unset |
| One binding | `Cargo.lock` holds exactly one `libsqlite3-sys`; the binary does not link a system SQLite (`ldd`) |
| Headless dispatch | `crates/reactor-gui/src/headless.rs`, called in `main.rs` before gpui starts |
| Licences | No `cargo-deny` configuration: removed at the owner's request on 4 October 2026. Without CI it would only run when remembered. Licences of new dependencies are checked by hand when they are added |
| Tier 2 | Set aside by the owner on 4 October 2026: the work focuses on Linux for now. `reactor-gui` has not been built on macOS or Windows |

**Rendering spike: the graph view has to be built, and gpui has what it needs.**

- gpui-kit 0.7.0 (`gpui-component`) ships charts — area, bar, candlestick, line, pie, radar, sankey — and no node-link graph component.
- gpui's `canvas` element gives a paint callback with `PathBuilder` (lines, quadratic and cubic curves, arcs, polygons, dashes, stroke and fill), `paint_path`, `paint_quad`, text and SVG painting, and layers. `div` has mouse-move, drag and scroll-wheel listeners. The bundled sankey chart (about 900 lines) is a worked example of nodes, curved links, labels, hover and a per-frame layout cache on these primitives.
- **Proposed construction:** edges, group outlines and badges painted on one `canvas`; nodes as ordinary positioned elements above it, so they get the theme, hover, click, context menus and text for free. Pan and zoom are a transform held by the view. Positions are stored per element so a layout stays stable between sessions.
- **Layout** is the part with no ready answer. A force-directed layout is small enough to write in Rust (a few hundred lines) or to take from a crate (`fdg-sim` exists; its licence and maintenance are not checked). Keeping group members together and keeping positions stable when elements are added needs iteration with the owner looking at it.
- **Rough size, an estimate and not a measurement:** a first usable view (draw, pan, zoom, select, collapsed binary claims, status and confidence styling) is on the order of the sankey chart plus a layout, roughly 1,500 to 2,500 lines. Group outlines for overlapping tags (hulls or bubble sets), bundling of repeated claims and progressive disclosure are each a further increment. The view is the larger half of Stage 2.
- **No prototype was drawn.** The first increment of Stage 2 should be a throwaway panel that draws a fixed graph, for the owner to look at before the SPEC section is written.

### 10.2 Stage 1 status (4 October 2026)

**Done and tested on Linux**

| Item | Where |
|---|---|
| Writer thread, read pool, migrations (`user_version`), backup with `VACUUM INTO` | `crates/reactor-db/src/db.rs`, `migrate.rs` |
| Schema 1: `author`, `blob`, `session`, `entry`, `event`, `case_manifest`, with append-only triggers | `migrate.rs` |
| Case directory, unsaved state, save, discard, session lock, list of known cases | `case.rs` |
| Blob store by content, with `fsck` | `blob.rs` |
| `Store` over SQLite; JSONL store, `index.json` and the `cwd` field removed | `crates/reactor-agent/src/store.rs`, `entry.rs` |
| Case-level goal and guidelines, versioned with author | `crates/reactor-db/src/manifest.rs`, `reactor-agent/src/context.rs` (`save_manifest`) |
| History tools across the case's sessions, behind the session-history switch | `reactor-agent/src/history.rs`, `tools/session.rs` |
| Memory switches as a settings layer (global, then session) | `crates/reactor-context/src/settings.rs` (`MemorySettings`) |
| GUI backend, launcher and picker on cases; `--session` takes an id | `crates/reactor-gui/src/backend.rs`, `start.rs`, `main.rs` |
| Headless `case create / save / list / sessions / export-session / backup` | `crates/reactor-gui/src/headless.rs` |

Exit criteria met: the agent's tests pass on the new store; a writer killed with SIGKILL leaves a session that reopens and continues; two processes write two sessions of one case without lock errors; `UPDATE` and `DELETE` on `entry` fail; saving an unsaved state loses nothing.

**Built, tested where it can be without a window, and not yet looked at**

- **Case › Save as Case…** in the menu and the palette (`save-case`). It relaunches: the window closes, and a new one opens on the same session with `--save-case`, which saves before anything opens the case. Refused while a turn runs.
- **The prompt on closing an unsaved state:** *Save as Case*, *Delete*, *Cancel*. A helper process (`reactor-gui case save|discard <dir> --wait`) acts once the window has let go. An unsaved state in which nothing was said is deleted without asking.
- **Working sessions** shimmer in the session list; closing the window while any works asks *Keep working* / *Interrupt and close* before the unsaved-state question.
- **Opening old sessions:** sessions are not tied to the window. The Session Tree panel lists the case's sessions (what was said first, when, how many entries, id; empty ones left out; another window's greyed); a double click or *Open* adds a transcript tab for one without closing anything, *+* starts a new one, *Close* lets go of one. Many are open at once, each with its own pump and state; the *active* session — the one whose transcript was last in focus, or last opened — is what the tree, context, status bar, switches and graph edits follow, and its id is shown (status bar, panel titles, tab dot). The list and the lock check are tested (`gui/src/session_list.rs`); the multi-session window needs the owner's eyes.
- **The memory switches** (three since the documents switch was added) above the prompt in the transcript panel. A click sets this session's layer.
- The helper commands and the save-on-relaunch are covered by tests; the menu, the dialog and the switches need the owner's eyes.

**Not done**

- `case export` and `case import` as an encrypted archive are Stage 4.
- The *Case graph* switch is stored and shown, but has nothing to detach from until Stage 2 brings the graph.

**Departures from this plan, decided while building**

| Planned | Built | Reason |
|---|---|---|
| `rusqlite_migration` | a list of SQL scripts and `user_version`, about 30 lines | avoids a crate that pins the `rusqlite` version |
| Manifest split inside `reactor-context` | `reactor-context` unchanged; the agent overlays the case's goal and guidelines on load and splits them on save | the module's golden tests stay as they are |
| `entry_fts` for search across sessions | the existing regular-expression search, run over each session | keeps `history_search`'s semantics; an index can be added if it gets slow |
| Saving moves the case while it is open | saving needs every handle closed; the window will restart its session to save | the session's `activation.json` path is held by value throughout the agent |
| zstd-compressed blobs | uncompressed | `blob_path` hands a real file to callers; compression can come later |
| `uuid` v5 ids, change-log rows for every write | only what Stage 1 writes: manifest changes | the graph brings the rest in Stage 2 |

### 10.3 Stage 2 status (4 October 2026)

Built in three increments: storage and rules, then pulling and the agent's tools, then the drawn view.

**Increment 1, done and tested: the graph in the database** (`crates/reactor-db/src/graph/`, schema migration 2)

| Item | Notes |
|---|---|
| Tables `element`, `element_version`, `edge`, `edge_version`; views `element_current`, `edge_current`, `relations` | as section 7.3, plus a `status` on edge versions so an edge can be taken away without deleting it |
| Invariants held by triggers | no edge between two entities; an entity's only outgoing edge is its source; a source is an external; an external has no outgoing edges; versions, edge ends, and an element's kind and type are never updated |
| The source rule | checked by the write path at the end of each batch; a claim without a source rolls the whole batch back |
| Identity | derived from a type's normalized key fields and optional scope; random for an element typed in with no key; an external's id from its URI. Key fields cannot be changed |
| One way in: `graph::apply` | a batch is one transaction and one change-log row. Saying what the graph already holds writes nothing, not even a change-log row |
| Versions | every change is a new version with author and record time; history and as-of queries; soft-delete hides and keeps |
| Vocabulary as rows | 22 types with normalizers and aliases, 19 roles with pairs and directions, both scales. A type added as a row is usable at once |
| URI rule | local resources are relative and inside the case; no `file:`; no credentials |
| Queries | current, as-of, history, edges in and out, filtered listing, relations, orphans, types in use, a snapshot for drawing |
| Table `artifact` | created; nothing writes it yet |

**Increment 2, done and tested: pulling, revert, the registry and the agent's tools**

| Item | Where | Notes |
|---|---|---|
| File registry | `crates/reactor-db/src/artifact.rs` | in place, by relative path; SHA-256, SHA-1, MD5, size, MIME; a path that leaves the case (through `..` or a symlink) or points into `.reactor/` is refused; `verify` reports changed and missing files |
| Pull | `graph/pull.rs` | SQLite table or whole file, CSV/TSV, JSONL, JSON with a pointer. The query runs on a private connection that can only read (authorizer, 30 s limit). Row cap 5,000. Preview. Overlap with earlier pulls is reported. A source that changed since it was pulled is reported |
| Source timestamps | `time.rs` | read into UTC; a missing zone is assumed UTC and flagged `assumed`; unreadable values keep the raw text and are flagged `unknown` |
| Revert | `graph/revert.rs` | by change number or by element and version; blocked by anything that pinned a version to be popped; forced, it takes those along; a claim left without a source goes too; `plan` previews with authors |
| Staleness | `graph/query.rs` (`stale`) | a pin on a version that moved on; resting on a claim now conflicting or soft-deleted; older than its type's limit |
| Tools | `crates/reactor-agent/src/tools/graph.rs` | `graph_add`, `graph_annotate` (add, change, revert), `graph_query`, `graph_pull`, `artifact_register`. Author is the agent with its model, on the analyst's behalf, with session and entry |
| Prompt block | `prompt.rs` (`CASE_GRAPH`) | static text; present while attached |
| *Case graph* switch | `tools/graph.rs`, `agent.rs` | off: the five tools and the block are gone, and a stale call is told why |

**Measured:** the five tools and the block add about 2,300 tokens to every request (estimated at four characters per token; a test holds it under 3,000). It is constant text, so the provider's cache covers it after the first request.

**Departures decided while building**

- `graph_annotate` also carries change and revert, to keep the tool count at five.
- A claim added by hand has a random id, so adding the same thing twice by hand makes two claims; only pulled rows are recognised. The prompt tells the agent to look first.
- Purge is not built. Revert covers taking versions off the top; removing a version from the middle of a chain has no caller yet.
- The `artifact` table has no version chain: re-registering a changed file updates its row, and the change log keeps the earlier hash.

**Increment 3, a first version: the drawn view** (`crates/reactor-gui/src/graph/`)

- `model.rs`, tested without a window: binary claims collapse to labeled, directed lines; repeated claims are bundled; a claim that is discussed or has more than two participants stays a node; sources are not drawn. Force-directed layout that is reproducible and keeps what was already placed.
- `panel.rs`, **built blind**: it compiles, and nobody has seen it draw. Canvas-painted lines with arrowheads, nodes and labels as elements, pan, zoom about the pointer, fit, select, a detail box, *Add note*, *Soft-delete*, show or hide soft-deleted, refresh after every tool call and turn.
- Registered as `PanelKind::Graph`; it shares the centre with the transcript in the Default and Analysis presets.

**Editing by hand in the view, built blind** (`reactor_db::graph::edit`, tested; the controls in `panel.rs` compile and nobody has seen them): add an entity, state a relation between two elements (sourced to the current session entry), confidence from the calibrated scale, dispute and restore, hypothesis status, tags, notes, soft-delete, and undo of the latest version with a preview.

**Second pass on the panel after the owner's first look (4 October 2026), built blind again.** Added things are placed where they can be seen (beside the selection or in the view's middle; the view pans to them if they would be out of sight); right-click on the plane adds a node or a note exactly there; several nodes can be selected (Ctrl/Cmd+click, Shift+drag area) to tag, note, set confidence or status, or **connect** them (two get a role each, more become the members of one claim: `edit::relate_many`); type fields offer the known types as they are typed (`edit::type_choices`, `suggest_types`, tested); zoom now scales node boxes, text, lines and arrowheads (4 % to 300 %), with tooltips and dots standing in for what is too small to read; descriptive text moved to tooltips and (i) buttons. The pure parts are tested (`tests/edit.rs`, `model::free_spot`); the pointer handling and drawing are not, and need the owner's eyes: marquee, group drag, the chips, right-click placement and the small-zoom look.

**Data tables in the table viewer, done** (`reactor_db::browse`, tested; the picker in `files/table.rs` built blind): any SQLite file opens as a table, read-only and untouched, up to 100,000 rows per table.

**Still to do in Stage 2**

- The owner looks at the panel; what is wrong with it is fixed.
- In the view: group outlines for tags and clusters, progressive disclosure, filters, stored positions.
- A real session against a model: nothing here has been exercised by an actual agent yet.

### 10.4 Stage 3 status (4 October 2026)

**Done and tested on Linux**

| Item | Where |
|---|---|
| Converters to Markdown: Markdown, plain text, HTML (main content only), PDF, Word, OpenDocument, EPUB, MediaWiki dumps (streamed, one document per page) | `crates/reactor-ingest` |
| Format is told from the content, not the name; a binary file, a scan with no text and a damaged file are errors that say so | `convert.rs` |
| Heading-aware chunking (800–2,000 characters) that keeps each passage's heading path | `chunk.rs` |
| Fetching a web page (http and https; no credentials; 64 MiB limit) | `fetch.rs` |
| Schema 3: documents, passages, FTS5 index with triggers, embedding models and status; one `chunk_vec_<model>` table per model | `migrate.rs`, `reactor-db/src/corpus/` |
| Storing a document: raw bytes and Markdown as blobs; the same source and content is a no-op; changed content replaces the old document, passages, keyword entries and vectors in one transaction | `corpus::add_document` |
| Hybrid search: FTS5 and vector KNN, fused by reciprocal rank fusion; every word of a query is quoted so nothing is read as FTS5 syntax | `corpus/search.rs` |
| Passages with no words of their own (a heading or a code-fence opener the splitter left alone: `## Consequences`, ```` ```sql ````) are joined to the passage that follows when a document is stored (`corpus::merge_hollow`), never come back from a search whichever ranking liked them (`is_hollow`, which also covers cases stored before this), and importing the folder again repairs a document stored with any. Found by another agent: their embeddings sit near the middle of the corpus, so a search by meaning alone returned them first for vague questions, identical ones in blocks. Not done: the heading is not put in front of every passage under it for embedding; passages after the first of a long section still start mid-list | `db/src/corpus/mod.rs`, `db/src/corpus/search.rs` |
| Models: registered by the size the model actually answers with; search keeps using the old vectors until the new set is complete, and never embeds a query with a model other than the one that made the stored vectors | `corpus/vectors.rs`, `agent/embed.rs` |
| The embedding worker: batches of 32, resumable after a failure or a stop, one run per case (lock file) | `agent/embed.rs` |
| Providers: openai, gemini, ollama through rig 0.42. Tested end to end against a fake Ollama server speaking Ollama's real `/api/embed` | `agent/tests/docs.rs` |
| `docs_search` tool: offered only once the case has documents; passages are wrapped in a tag naming source, section and origin, cannot close it, and are labeled as data | `agent/src/tools/docs.rs` |
| `docs_search` takes a `mode` (`keyword`, `semantic`, `both`) only while a search by meaning can be made (a model is chosen and every passage has its vector); otherwise the parameter and every mention of meaning are left out of what the agent is shown, and a request for it is refused with the reason. (`SessionState.meaning`, `Tool::spec_for`) | `agent/src/tools/docs.rs`, `agent/src/embed.rs` |
| Documents panel: the same search by hand, with the same mode rule, marked results, the passage in full, its neighbours and its source (`PanelKind::Docs`, `corpus::surrounding`); built blind, the window has not been seen | `gui/src/docs_panel.rs` |
| A finished `docs_search` call is shown in the transcript as a summary and one line per top match (five), each opening the result in the Documents panel at that passage; the parser lives beside the formatter and is tested to round-trip (`tools::docs::parse_result`, `corpus::hits_at`); built blind | `agent/src/tools/docs.rs`, `gui/src/panels.rs`, `gui/src/app.rs` |
| The corpus as a rig `VectorStoreIndex`, with the request's filter read against source kind, origin, mime and metadata | `agent/embed.rs` (`ContextIndex`) |
| Setting `embedding.minSimilarity` (global; 0 or unset is none): a passage less similar in meaning than this (cosine) is not a match by meaning, for the agent and the Documents panel; a hit carries its similarity (`Hit.similarity`), which is not its score (that is made of the ranks) | `context/src/settings.rs`, `db/src/corpus/search.rs` |
| Setting `embedding.model` (global only; none by default) | `reactor-context/src/settings.rs` |
| Import for the user: `/import`, *Case › Import Documents…*, `/embedding`, status-bar progress; headless `ingest`, `embed`, `case rebuild-indexes` | `reactor-gui/src/import.rs`, `headless.rs`, `backend.rs`, `app.rs` |

**Measured** (release build, this machine, one reader, 384-dimension random vectors and random words; cost, not quality)

| Passages | `analysis.db` | keyword, rare identifier | keyword, 6-word question | meaning only (KNN) | hybrid, as `docs_search` runs it |
|---|---|---|---|---|---|
| 100,000 | 224 MiB | 0.0 ms (p50) | 22 ms | 65 ms | 69 ms (p95 78) |
| 1,000,000 | 2.2 GiB | 0.1 ms | 227 ms | 617 ms | 649 ms (p95 745) |

At the owner's stated scale (at most about 100 documents of 100 to 1,000 pages, probably far fewer), the worst case is a few hundred thousand passages, so a search is a fraction of a second. At 1,000,000 it is still under a second, with brute-force sqlite-vec. Building the 1,000,000-passage corpus directly through SQL took about a minute; real ingestion is dominated by conversion and by the embedding provider. The benchmark is `crates/reactor-db/tests/scale.rs` (ignored by default).

**Not verified, or not built**

- **No real embedding provider has been used.** The OpenAI and Gemini paths are the same generic code as Ollama's and were not run. Whether a real model's vectors give good results is untested: nothing here measures search *quality*; the plan's "labelled query set" is not built, and cannot be without the owner's real documents and questions.
- **The window controls** (`/import`, `/embedding`, the menu item, the status line) compile and are covered by backend tests, and nobody has seen them.
- **Real documents.** Every converter was tested on a fixture built from its format's structure, not on the messy files people have. PDF is the likeliest to disappoint: `pdf-extract` handles text PDFs and loses layout (columns, tables); there is no OCR.
- **A local model in process, a code-specific model and a reranker** are not built, as decided.
- **Search quality knobs** (the weight of headings in the keyword ranking, how deep each ranking is cut, chunk size) are first guesses.

**Departures decided while building**

- **Imported documents are copied into `blobs/`** (their bytes and their Markdown), an exception to D10: they come from anywhere, usually outside the case, and a case must hold what it was built on to be exported and handed over. A document that is one of many in a dump keeps its Markdown as its source, not the multi-gigabyte dump.
- **A source is named without an absolute path**: relative to the case where it is inside it, else relative to the folder it was imported from, with that folder's own name first (`wiki/tools/ghidra.txt`); a URL by itself. Two different imports with the same name are one source, so the later replaces the earlier, and the import says so.
- **Wiki dumps keep articles, project pages and help pages**; redirects and discussion pages are skipped.
- **The corpus's hit carries `source_kind`, `mime` and `meta`** so that a rig filter can be evaluated over it in Rust, rather than translated to SQL.
- **Rank fusion is done in Rust**, over two queries, instead of the single SQL statement the handoff sketches: the same result, and testable.

### 10.5 Stage 4 status (4 October 2026)

**Done and tested on Linux**

| Item | Where |
|---|---|
| Case export: the whole folder as one ZIP with AES-256, deflated, no executable bits; the databases as consistent snapshots (`VACUUM INTO`); write-ahead logs, lock files, links and the archive itself left out. Password `infected` unless one is given, with `REACTOR_CASE_PASSWORD` as a way that does not show in the process list | `reactor-db/src/export.rs` |
| Case import: into an empty folder; a wrong password is said so and leaves nothing; an archive that would write outside the folder is refused; the result is opened (migrated) and its blobs checked | `export.rs` |
| Round trip of a case: graph (ids, versions, edges), sessions, goal, registered files, corpus and its vectors, the agent's data file, evidence byte for byte | `reactor-db/tests/export.rs` |
| STIX 2.1 export following the mapping table; the case is the `identity`; deterministic (the same graph, the same bundle) | `reactor-db/src/interchange/stix.rs` |
| STIX import: a bundle with `x_reactor` restores the graph exactly (the same element and edge ids, counts, authors, record times, props, tags, pins, sources and where in them); a bundle from elsewhere is mapped as far as it goes and every claim is sourced to the bundle | `stix.rs` |
| Structural validation of a bundle | `stix::validate` |
| CSV tables of elements and edges; the entity-to-entity projection as an edge list and as GraphML, with limits on large claims and on entities in too many claims, candidate roles left out | `interchange/tables.rs`, `projection.rs` |
| Headless: `case export`, `case import`, `graph export` (stix, elements, edges, edge-list, graphml), `graph import`, `graph check` | `reactor-gui/src/headless.rs` |

**Checked against the official OASIS STIX 2.1 JSON schemas, by hand.** `scripts/check-stix.py` ran every object of a bundle made from a graph with every kind of element through the schema of its own type (`oasis-open/cti-stix2-json-schemas`, commit `9af1db4`, with `jsonschema` 4.19): 17 objects, no problems. That check found one real bug, now fixed: a truncated value in a hash entity was exported as a `file` with an invalid hash; it is now a custom entity unless it is a hash of the right length. Custom `x-reactor-entity` objects have no schema, so only the common properties were checked for them. The schemas' own `bundle.json` omits note, opinion and grouping, so the bundle is checked without its `objects`. The check is not part of `cargo test`: it needs the schemas, which are not in the repository.

**Not built, or not as the plan said**

- **History is not in a STIX bundle.** Only the current version of each element is exported, so a restored graph is at version 1 throughout; it keeps who wrote each element and when it was first recorded. A case's whole history travels in a case export, not a bundle.
- **The vocabulary does not travel in a bundle.** An entity of a type that a case added as a row and the receiving case lacks is refused (its id would not derive the same way). Case export carries the whole database.
- **`graph import` writes into the case it is given**; there is no preview.
- **No window controls** for export, import or the graph exports; they are headless commands (D7).
- **Importers from other databases, Parquet, MISP, OCSF and SARIF** are not built: the plan said on demand, and nothing has asked.
- **The archive's file times** are the time of export, not each file's own.
- **STIX relationships and observed data use `x_description`/`x_reactor`** for what STIX has no field for; another tool will not show them.

## 11. Risks

| Risk | Mitigation |
|---|---|
| Moving the store breaks reductions, branching or resume | The `Store` API and the agent test suite are the contract |
| The graph view needs a component gpui-kit does not have | Stage 0 spike; D15 says it is built regardless |
| A GUI dependency links a second SQLite | None today; the Stage 0 spike builds the GUI with it |
| A headless command initialises the window stack and fails without a display | Dispatch subcommands before gpui starts; test on a machine with no display server |
| Scripts and REactor contend on `reactor_data.sqlite` | WAL and busy timeout; the socket of the handoff as fallback (D4) |
| Document text is sent to a remote embedding provider | The user chooses the model; the settings screen says so; an Ollama server keeps it local |
| The agent writes to `analysis.db` with `sqlite3` despite the rule | Triggers reject invalid graph changes; versions are append-only; this is accepted (ADR-0033) |
| The agent floods the graph or ignores it | The prompt block and the user's direction; authorship makes agent output filterable; revert per batch |
| A Tier 2 platform (macOS, Windows) builds but misbehaves at runtime, unnoticed | Accepted by the tier definition (D16). Running `reactor-gui db selfcheck` there once is cheap and recommended, since extension registration is the likeliest thing to differ |

## 12. Not verified — check before relying on it

- **Which rig 0.42 providers offer embedding models.** The `EmbeddingModel` and `VectorStoreIndex` traits were read in `rig-core-0.42.0` (`embeddings/embedding.rs`, `vector_store/mod.rs`); the per-provider embedding clients were not.
- **The `sqlite-vec` crate builds and registers on macOS and Windows.** It does on Linux with `rusqlite` 0.40.2 (section 10.1). The handoff warns that the registration snippet has broken across versions.
- **A layout crate for the graph view** (`fdg-sim` or another): licence, maintenance and fit are not checked.
- **The `zip` crate writes AES-256 archives** that 7-Zip opens.
- **Headless use of `reactor-gui` on a machine with no display server at all.** `db selfcheck` runs with the display variables unset; a machine without the windowing libraries installed was not tried.
- **A stock `sqlite3` can open `analysis.db`** for its ordinary tables despite the `vec0` virtual tables.

## 13. Conventions to follow

- **Names.** The project is "REactor". Use the glossary in `CONTEXT.md`, including its "avoid" lists.
- **ADRs first.** One file per decision in `docs/adr/`, numbered from 0045, in the existing format with a "Considered and rejected" section. Mark ADR-0036 as superseded in part.
- **Layering.** `reactor-core` knows files and subprocesses only. `reactor-context` is pure functions of state with no model, loop or UI. rig types appear only at the seam in `reactor-agent`.
- **Tests.** Hermetic, with `tempfile` and the scripted model in `crates/reactor-agent/tests/`. `reactor-context` has golden tests that change only on purpose; D9 is such a purpose.
- **Build.** `cargo test` covers the default members; the GUI needs `-p reactor-gui`.
- **Errors.** One error enum per crate in the style of `reactor-agent::Error`, with `From` conversions upward.
- **JSON files** are written through `reactor_core::json::write_json_atomic`.
- **Git.** Commit only when the owner asks. The two input documents are excluded locally and stay untracked.
- **The owner's standing instruction:** do not take screenshots of the GUI; ask the owner to look instead.

## 14. Deferred

- A socket and single writer process (D4).
- A case-level tier in the settings cascade. Only the goal and guidelines are case-level, and they are not settings.
- A local in-process embedding model, and a code-specific model (D8).
- `petgraph`, approximate vector search, vector quantisation, a reranker.
- Importers from PostgreSQL and MySQL, Parquet, MISP, OCSF and SARIF adapters: built when a real need appears.
- Resolvers for tool databases such as Binary Ninja: the URI is kept as a link until one is needed.
