# The table viewer is a grid over sources, and SQL is one of them

The table viewer ([ADR-0044](0044-filesystem-panels-read-through-a-vfs.md) names the panel)
opens delimited text, and since ADR-0046 it also opens any SQLite file: tables and views
are picked from a toolbar and read through `reactor_db::browse`. It cannot ask a database
anything. This records how it grows a schema view and SQL queries without becoming a
second viewer.

## Why

**The grid is a `Sheet` of strings.** Every value is text, so a NULL is the string
`NULL`, a number is parsed again to be sorted, and a table is copied into memory whole
before anything is shown. That is why browsing stops at 100,000 rows.

**A database has no header rows and no delimiter, and a CSV has no schema.** The toolbar
shows both controls whatever the file is. What is around the grid has to follow what feeds
it; the grid itself — sort header, selection, copy, theme — is the same.

**The viewer is used on files that are evidence.** The agent's `reactor_data.sqlite`, a
case's `analysis.db` and a database from a seized machine are all opened here. Opening one
must not change it, and "opened read-only" does not guarantee that (below).

## Decisions

**There is one Table viewer, over three sources.** Delimited text (`TextSource`, today's
parser, delimiter detection, header rows and numeric sort unchanged), a SQLite table or
view (`SqlTable`: sort and filter go into the SELECT, rows are fetched a window at a
time), and a SQL result (`QuerySource`). The panel keeps its name `table`, `Viewer::Table`
and `/open table` keep working, and `files::suggest` still routes SQLite files here by
their first 16 bytes. What is shown around the grid — the schema rail and query tabs for a
database, the delimiter and header rows for text — follows the source.

**Cells are typed.** `Null`, `Integer`, `Real`, `Text` and `Blob`, not strings. A NULL is
drawn as NULL, numbers align right, and a BLOB is a hex preview with its length, with the
bytes kept for *Hex* and *Save…*. `Sheet` stays the form for text and is not widened.

**Queries read and nothing else.** A query runs on a connection whose authorizer allows
SELECT, read, function and recursive, and refuses everything else — the lock-down the
graph pulls already use. A write, an `ATTACH` or a `PRAGMA` is refused with SQLite's own
"not authorized", and the file is not touched. There is no writable mode; if one is ever
wanted it is a separate, confirmed *Open writable…*, not a setting of this one.

**The file is opened in the mode its WAL state allows, and nothing is created beside it.**
An ordinary read-only open of a WAL-mode file in a writable folder leaves `-shm` and `-wal`
files behind. So the mode is chosen from the file:

- The header does not say WAL (bytes 18 and 19 are not 2): opened `mode=ro`, as before.
- WAL, with an `-shm` beside it — a database in use, such as the case's own, or one whose
  sidecars are already there: opened `mode=ro`. They exist, so none is created, and the
  frames in the `-wal` are seen.
- WAL, no `-shm`, and no `-wal` or an empty one: opened `immutable=1`. There is nothing to
  replay, and nothing is written.
- WAL, no `-shm`, and frames in the `-wal` — a copy taken without it: the database and its
  `-wal` are copied to a temporary directory and read there, so the original is never
  written and the frames are seen.

If a `mode=ro` open of a WAL file fails where it lies (read-only media, a stale `-shm`), it
is read from a copy as in the last case.

**A query streams, and its limit is memory.** The statement runs on a worker, which hands
rows to the grid in chunks of a couple of thousand; the first page appears at once and the
footer counts up. It ends when the statement does, when the person presses Stop (an
interrupt on the connection), or when the rows held reach a memory budget (256 MiB to
begin with, adjustable), and the footer says which. There is no row-count cap. Browsing a
table needs neither, because it fetches only the window in view.

**Sorting a query result re-runs it.** The statement is wrapped in
`SELECT * FROM (…) ORDER BY <column position>`, so the order covers every row and not
just those that were read.

**A CSV is queried on request.** A *Query…* button on a delimited file loads the sheet,
on a worker, into a table of a temporary SQLite file and shows it beside the sheet: a
*Sheet* tab with the grid as it was, a rail with the table and its columns, and a query tab
that has already run `SELECT * FROM <file name> LIMIT 100`. Opening a CSV costs nothing
more than it does now. The file, not memory, so that a second connection can read it and
a long query does not hold up the sheet, and so the sheet's rows are not held twice.
The temporary file goes when the viewer is closed, and nothing is written beside the CSV.

Columns are named as the viewer already names them from its header rows (a repeated name
gets `_2`, `_3`), and typed from what is in them: `INTEGER` only when every non-empty cell
is an integer that reads back as written, `REAL` when every one is a plain decimal or
exponent number, otherwise `TEXT`. So `007` stays text and a sum over a port column works.
An empty cell is NULL in a number column and an empty string in a text one. Changing the
delimiter or the header rows loads the rows again, and the query tabs come back with them.

## Consequences

**`reactor_db::browse` is the one place that opens a database for looking.** It grows an
open `Database` handle instead of a connection per call, `schema()`, a chunked `run()` and a
`window()`; `tables()` and `read_table()` stay and use the same open, so the viewer stops
leaving sidecar files beside a WAL file as soon as it is rebuilt on top.

**A query can still be expensive.** SQLite has no per-statement memory limit of its own, so
a query that sorts a large table holds it in SQLite's memory, outside the budget. Stop
interrupts it; the budget bounds what reaches the grid, not what the engine does on the
way. No length limit is set on the connection, because a database taken from a machine
may hold values larger than any limit worth choosing.

**The `-wal` copy costs a copy.** It happens only for a WAL file with frames and no `-shm`,
which is a database copied off a machine, and the copy is deleted when the viewer closes.

**Spilling results to a temporary database is not built.** It would remove the memory
bound and let a result be sorted and filtered by SQL like a table, at the price of writing
every result to disk. The grid does not change if it arrives: a spilled result is a
`SqlTable` over the scratch file.

## Considered and rejected

**A separate *Database* panel.** It would repeat the grid, selection, copy, theming and
the closing and popup behaviour, and leave two viewers for "a table", one of which a CSV
could not be opened in.

**A SQLite-only rewrite that folds delimited text in later.** It changes behaviour people
already rely on — delimiter detection, header rows, numeric sort — before the new part
has proved itself.

**Loading every CSV into SQLite when it is opened.** A quick look at a file would pay for
a copy of up to 64 MiB. Loading on request gives the same pane for the same price when it
is wanted.

**An in-memory database for the CSV.** A second connection cannot see a private in-memory
database, so a long query would block the sheet, and the sheet's rows would be held twice.
A temporary file costs a write that a 64 MiB sheet makes in a moment.

**A row cap, as browsing has today.** It was there because results were collected whole.
With a cursor and a memory budget the cap protects nothing the budget does not, and it
hides rows for no reason the person can act on.

**`immutable=1` for every WAL file.** It never touches the folder, but it ignores the
`-wal`, so a database in use appears to be missing everything not yet checkpointed.
