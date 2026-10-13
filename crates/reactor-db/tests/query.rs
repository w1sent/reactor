//! Querying a SQLite file for the table viewer: typed cells, windows, the schema, a
//! streaming read-only cursor, and opening without leaving anything beside the file.

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reactor_db::browse::{
    self, Budget, Database, Event, OpenMode, Stop, Value, WindowSpec, ordered_by,
};
use rusqlite::Connection;

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("evidence.sqlite");
    let c = Connection::open(&path).unwrap();
    c.execute_batch(
        "PRAGMA journal_mode = DELETE;
         CREATE TABLE host (id INTEGER PRIMARY KEY, name TEXT NOT NULL, port INTEGER,
                            note TEXT, raw BLOB);
         CREATE INDEX host_name ON host(name);
         CREATE UNIQUE INDEX host_port ON host(port, name);
         CREATE TABLE conn (id INTEGER PRIMARY KEY, host_id INTEGER REFERENCES host(id),
                            bytes INTEGER);
         CREATE VIEW big AS SELECT * FROM conn WHERE bytes > 100;
         INSERT INTO host VALUES (1, 'ws-042', 3389, NULL, x'0102'),
                                 (2, 'ws-043', 22, 'second', NULL),
                                 (3, 'srv-db1', 5432, '100%_sure', NULL),
                                 (4, 'ws-017', 443, NULL, NULL),
                                 (5, 'ws-051', NULL, 'x', NULL);
         INSERT INTO conn VALUES (1, 1, 50), (2, 1, 500), (3, 2, 5000);",
    )
    .unwrap();
    (dir, path)
}

fn ids(page: &browse::Page) -> Vec<i64> {
    page.rows
        .iter()
        .map(|r| match r[0] {
            Value::Integer(i) => i,
            ref other => panic!("not an integer: {other:?}"),
        })
        .collect()
}

fn spec(filter: &str, sort: Option<(&str, bool)>, offset: u64, limit: u64) -> WindowSpec {
    WindowSpec {
        filter: filter.to_string(),
        sort: sort.map(|(c, a)| (c.to_string(), a)),
        offset,
        limit,
    }
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// --- typed cells and windows ---------------------------------------------------------

#[test]
fn cells_keep_the_type_sqlite_stored() {
    let (_d, path) = fixture();
    let db = Database::open(&path).unwrap();
    let page = db.window("host", &spec("", None, 0, 10)).unwrap();
    assert_eq!(page.columns, ["id", "name", "port", "note", "raw"]);
    assert_eq!(
        page.rows[0],
        [
            Value::Integer(1),
            Value::Text("ws-042".into()),
            Value::Integer(3389),
            Value::Null,
            Value::Blob(vec![1, 2]),
        ]
    );
    // NULL is not the text "NULL", and says so when drawn.
    assert!(page.rows[0][3].is_null());
    assert_eq!(page.rows[0][3].display(), "NULL");
    assert_eq!(page.rows[0][4].display(), "x'0102'");
    assert_eq!(
        Value::Blob(vec![0xab; 18]).display(),
        format!("x'{}…' (18 bytes)", "ab".repeat(16))
    );
}

#[test]
fn a_window_is_ordered_filtered_and_cut_by_sql() {
    let (_d, path) = fixture();
    let db = Database::open(&path).unwrap();
    // Rowid order, then a slice of it.
    assert_eq!(
        ids(&db.window("host", &spec("", None, 1, 2)).unwrap()),
        [2, 3]
    );
    // Numbers sort as numbers; a NULL is last whichever way round.
    let asc = db
        .window("host", &spec("", Some(("port", true)), 0, 10))
        .unwrap();
    assert_eq!(ids(&asc), [2, 4, 1, 3, 5]);
    let desc = db
        .window("host", &spec("", Some(("port", false)), 0, 10))
        .unwrap();
    assert_eq!(ids(&desc), [3, 1, 4, 2, 5]);
    // A filter looks in every column, ignores case, and a NULL never matches.
    assert_eq!(
        ids(&db.window("host", &spec("WS-04", None, 0, 10)).unwrap()),
        [1, 2]
    );
    assert_eq!(
        ids(&db.window("host", &spec("3389", None, 0, 10)).unwrap()),
        [1]
    );
    assert!(
        db.window("host", &spec("NULL", None, 0, 10))
            .unwrap()
            .rows
            .is_empty()
    );
    // `%` and `_` are characters, not wildcards.
    assert_eq!(
        ids(&db.window("host", &spec("%", None, 0, 10)).unwrap()),
        [3]
    );
    assert_eq!(
        ids(&db.window("host", &spec("0%_s", None, 0, 10)).unwrap()),
        [3]
    );
    // The filter and the order apply before the slice.
    let page = db
        .window("host", &spec("ws-", Some(("name", false)), 1, 2))
        .unwrap();
    assert_eq!(ids(&page), [2, 1]);
    // A view reads like a table, and a made-up column is an error, not SQL.
    assert_eq!(
        ids(&db.window("big", &spec("", None, 0, 10)).unwrap()),
        [2, 3]
    );
    assert!(
        db.window("host", &spec("", Some(("name\" DESC; --", true)), 0, 10))
            .is_err()
    );
    assert!(db.window("nope", &spec("", None, 0, 10)).is_err());
}

#[test]
fn rows_are_counted_with_the_same_filter() {
    let (_d, path) = fixture();
    let db = Database::open(&path).unwrap();
    let all = db.count("host", "").unwrap();
    assert_eq!((all.rows, all.exact), (5, true));
    assert_eq!(db.count("host", "ws-").unwrap().rows, 4);
    assert_eq!(db.count("host", "no such").unwrap().rows, 0);
}

#[test]
fn the_schema_has_types_keys_indexes_and_references() {
    let (_d, path) = fixture();
    let db = Database::open(&path).unwrap();
    let host = db.schema("host").unwrap();
    assert!(!host.is_view);
    let cols: Vec<(&str, &str, bool, u32)> = host
        .columns
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                c.declared_type.as_str(),
                c.not_null,
                c.primary_key,
            )
        })
        .collect();
    assert_eq!(
        cols,
        [
            ("id", "INTEGER", false, 1),
            ("name", "TEXT", true, 0),
            ("port", "INTEGER", false, 0),
            ("note", "TEXT", false, 0),
            ("raw", "BLOB", false, 0),
        ]
    );
    let mut indexes: Vec<(String, bool, Vec<String>)> = host
        .indexes
        .iter()
        .map(|i| (i.name.clone(), i.unique, i.columns.clone()))
        .collect();
    indexes.sort();
    assert_eq!(
        indexes,
        [
            ("host_name".to_string(), false, vec!["name".to_string()]),
            (
                "host_port".to_string(),
                true,
                vec!["port".to_string(), "name".to_string()]
            ),
        ]
    );
    assert!(
        host.sql
            .as_deref()
            .unwrap()
            .starts_with("CREATE TABLE host")
    );

    let conn = db.schema("conn").unwrap();
    assert_eq!(conn.foreign_keys.len(), 1);
    assert_eq!(
        (
            conn.foreign_keys[0].from.as_str(),
            conn.foreign_keys[0].table.as_str()
        ),
        ("host_id", "host")
    );
    let view = db.schema("big").unwrap();
    assert!(view.is_view);
    assert_eq!(view.columns.len(), 3);
    assert!(db.schema("nope").is_err());
}

// --- running a query -------------------------------------------------------------------

fn collect(
    db: &Database,
    sql: &str,
    budget: &Budget,
) -> (Vec<String>, Vec<usize>, Vec<Vec<Value>>, Stop) {
    let (mut columns, mut chunks, mut rows) = (Vec::new(), Vec::new(), Vec::new());
    let done = db
        .run(sql, budget, |event| {
            match event {
                Event::Columns(c) => columns = c,
                Event::Rows(r) => {
                    chunks.push(r.len());
                    rows.extend(r);
                }
            }
            ControlFlow::Continue(())
        })
        .unwrap();
    assert_eq!(done.rows as usize, rows.len());
    (columns, chunks, rows, done.stop)
}

#[test]
fn a_query_arrives_in_chunks() {
    let (_d, path) = fixture();
    let db = Database::open(&path).unwrap();
    let budget = Budget {
        chunk_rows: 2,
        ..Budget::default()
    };
    let (columns, chunks, rows, stop) =
        collect(&db, "SELECT id, name FROM host ORDER BY id;", &budget);
    assert_eq!(columns, ["id", "name"]);
    assert_eq!(chunks, [2, 2, 1]);
    assert_eq!(rows.len(), 5);
    assert_eq!(stop, Stop::Finished);
    // An expression has no declared type, and keeps the type it computes.
    let (columns, _, rows, _) = collect(
        &db,
        "SELECT count(*) AS n, sum(bytes) FROM conn",
        &Budget::default(),
    );
    assert_eq!(columns, ["n", "sum(bytes)"]);
    assert_eq!(rows[0], [Value::Integer(3), Value::Integer(5550)]);
}

#[test]
fn a_query_stops_at_its_memory_budget_and_says_so() {
    let (_d, path) = fixture();
    let db = Database::open(&path).unwrap();
    let budget = Budget {
        bytes: 1,
        chunk_rows: 10,
    };
    let (_, _, rows, stop) = collect(&db, "SELECT * FROM host", &budget);
    assert_eq!(stop, Stop::Budget);
    assert_eq!(
        rows.len(),
        1,
        "the row that crossed it is kept, nothing after"
    );

    // A budget that fits does not stop it.
    let (_, _, rows, stop) = collect(&db, "SELECT * FROM host", &Budget::default());
    assert_eq!((rows.len(), stop), (5, Stop::Finished));
}

#[test]
fn the_receiver_can_stop_a_query() {
    let (_d, path) = fixture();
    let db = Database::open(&path).unwrap();
    let mut chunks = 0;
    let done = db
        .run(
            "SELECT * FROM host",
            &Budget {
                chunk_rows: 1,
                ..Budget::default()
            },
            |event| {
                if let Event::Rows(_) = event {
                    chunks += 1;
                    return ControlFlow::Break(());
                }
                ControlFlow::Continue(())
            },
        )
        .unwrap();
    assert_eq!((chunks, done.stop, done.rows), (1, Stop::Stopped, 1));
}

#[test]
fn an_interrupt_stops_a_statement_that_would_not_end() {
    let (_d, path) = fixture();
    let db = Database::open(&path).unwrap();
    let handle = db.interrupt_handle();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        handle.interrupt();
    });
    let done = db
        .run(
            "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c LIMIT 2000000000)
             SELECT count(*) FROM c",
            &Budget::default(),
            |_| ControlFlow::Continue(()),
        )
        .unwrap();
    stopper.join().unwrap();
    assert_eq!(done.stop, Stop::Stopped);
    assert!(done.elapsed < Duration::from_secs(30));
}

#[test]
fn a_query_cannot_write_attach_or_change_settings() {
    let (_d, path) = fixture();
    let before = std::fs::read(&path).unwrap();
    let db = Database::open(&path).unwrap();
    for sql in [
        "UPDATE host SET note = 'x'",
        "DELETE FROM host",
        "INSERT INTO host (name) VALUES ('x')",
        "DROP TABLE host",
        "CREATE TABLE t (x)",
        "ATTACH DATABASE ':memory:' AS m",
        "PRAGMA journal_mode = DELETE",
    ] {
        let err = db
            .run(sql, &Budget::default(), |_| ControlFlow::Continue(()))
            .unwrap_err();
        assert!(err.refused, "{sql}: {err}");
    }
    // Whatever else is not a SELECT never runs either.
    let err = db
        .run("VACUUM", &Budget::default(), |_| ControlFlow::Continue(()))
        .unwrap_err();
    assert!(!err.message.is_empty());
    // And the connection reads again afterwards.
    let (_, _, rows, _) = collect(&db, "SELECT count(*) FROM host", &Budget::default());
    assert_eq!(rows[0][0], Value::Integer(5));
    drop(db);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the file is not touched"
    );
}

#[test]
fn a_mistake_says_what_and_where() {
    let (_d, path) = fixture();
    let db = Database::open(&path).unwrap();
    let sql = "SELECT 1 FORM host";
    let err = db
        .run(sql, &Budget::default(), |_| ControlFlow::Continue(()))
        .unwrap_err();
    assert!(err.message.contains("syntax error"), "{}", err.message);
    assert_eq!(err.offset, sql.find("host"));
    assert!(!err.refused);

    let err = db
        .run("SELECT 1; SELECT 2", &Budget::default(), |_| {
            ControlFlow::Continue(())
        })
        .unwrap_err();
    assert!(err.message.contains("one statement"), "{}", err.message);

    let err = db
        .run("SELECT * FROM nope", &Budget::default(), |_| {
            ControlFlow::Continue(())
        })
        .unwrap_err();
    assert!(err.message.contains("no such table"), "{}", err.message);
}

#[test]
fn ordering_a_result_wraps_the_statement_and_covers_every_row() {
    let (_d, path) = fixture();
    let db = Database::open(&path).unwrap();
    // Two columns called `id`, and a comment on the last line.
    let sql = "SELECT a.id, b.id FROM host a JOIN host b ON a.id <> b.id WHERE a.id < 3 -- pairs\n";
    let (_, _, rows, _) = collect(&db, &ordered_by(sql, 2, false), &Budget::default());
    assert_eq!(rows[0][1], Value::Integer(5));
    assert_eq!(rows.len(), 8);
    let (_, _, rows, _) = collect(
        &db,
        &ordered_by("SELECT id FROM host;", 1, true),
        &Budget::default(),
    );
    assert_eq!(rows[0][0], Value::Integer(1));
}

// --- opening ---------------------------------------------------------------------------

/// A WAL-mode database with rows still in its `-wal`, and the connection that holds it.
fn wal_with_frames(dir: &Path) -> (PathBuf, Connection) {
    let path = dir.join("a.db");
    let w = Connection::open(&path).unwrap();
    w.pragma_update(None, "journal_mode", "WAL").unwrap();
    w.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    w.execute_batch("CREATE TABLE t (x); INSERT INTO t VALUES (1), (2), (3);")
        .unwrap();
    (path, w)
}

fn count_t(db: &Database) -> i64 {
    let (_, _, rows, _) = collect(db, "SELECT count(*) FROM t", &Budget::default());
    match rows[0][0] {
        Value::Integer(n) => n,
        ref v => panic!("{v:?}"),
    }
}

#[test]
fn a_plain_file_opens_read_only() {
    let (_d, path) = fixture();
    assert_eq!(browse::open_mode(&path).unwrap(), OpenMode::ReadOnly);
    assert_eq!(Database::open(&path).unwrap().mode(), OpenMode::ReadOnly);
}

#[test]
fn a_wal_file_with_nothing_to_replay_is_opened_without_touching_its_folder() {
    let dir = tempfile::tempdir().unwrap();
    let (path, w) = wal_with_frames(dir.path());
    drop(w); // the last connection closing checkpoints and removes the sidecars
    assert_eq!(
        listing(dir.path()),
        ["a.db"],
        "the premise: a WAL file alone"
    );
    let before = std::fs::read(&path).unwrap();

    assert_eq!(browse::open_mode(&path).unwrap(), OpenMode::Immutable);
    let db = Database::open(&path).unwrap();
    assert_eq!(db.mode(), OpenMode::Immutable);
    assert_eq!(count_t(&db), 3);
    assert_eq!(db.tables().unwrap().len(), 1);
    drop(db);
    assert_eq!(
        listing(dir.path()),
        ["a.db"],
        "nothing was created beside it"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn a_database_in_use_is_read_with_its_wal_and_nothing_new_appears() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _writer) = wal_with_frames(dir.path());
    let before = listing(dir.path());
    assert!(before.contains(&"a.db-wal".to_string()) && before.contains(&"a.db-shm".to_string()));

    assert_eq!(browse::open_mode(&path).unwrap(), OpenMode::ReadOnly);
    let db = Database::open(&path).unwrap();
    assert_eq!(db.mode(), OpenMode::ReadOnly);
    assert_eq!(count_t(&db), 3, "rows that are only in the -wal are seen");
    drop(db);
    assert_eq!(listing(dir.path()), before);
}

#[test]
fn a_wal_file_copied_without_its_shm_is_read_from_a_copy() {
    let source = tempfile::tempdir().unwrap();
    let (live, _writer) = wal_with_frames(source.path());
    // What taking a file off a machine usually gives: the database and its -wal.
    let evidence = tempfile::tempdir().unwrap();
    let path = evidence.path().join("a.db");
    std::fs::copy(&live, &path).unwrap();
    std::fs::copy(
        source.path().join("a.db-wal"),
        evidence.path().join("a.db-wal"),
    )
    .unwrap();
    assert_eq!(listing(evidence.path()), ["a.db", "a.db-wal"]);
    let (db_before, wal_before) = (
        std::fs::read(&path).unwrap(),
        std::fs::read(evidence.path().join("a.db-wal")).unwrap(),
    );

    assert_eq!(browse::open_mode(&path).unwrap(), OpenMode::Copy);
    let db = Database::open(&path).unwrap();
    assert_eq!(db.mode(), OpenMode::Copy);
    assert_eq!(count_t(&db), 3, "the frames in the -wal are seen");
    drop(db);
    assert_eq!(
        listing(evidence.path()),
        ["a.db", "a.db-wal"],
        "no -shm was made"
    );
    assert_eq!(std::fs::read(&path).unwrap(), db_before);
    assert_eq!(
        std::fs::read(evidence.path().join("a.db-wal")).unwrap(),
        wal_before
    );
}

#[test]
fn a_second_connection_reads_the_same_file_and_the_copy_outlives_the_first() {
    let (_d, path) = fixture();
    let first = Database::open(&path).unwrap();
    let second = first.reopen().unwrap();
    assert_eq!(second.mode(), OpenMode::ReadOnly);
    drop(first);
    assert_eq!(second.count("host", "").unwrap().rows, 5);

    // A copy is shared, and goes when the last connection on it does.
    let source = tempfile::tempdir().unwrap();
    let (live, _writer) = wal_with_frames(source.path());
    let evidence = tempfile::tempdir().unwrap();
    let copied = evidence.path().join("a.db");
    std::fs::copy(&live, &copied).unwrap();
    std::fs::copy(
        source.path().join("a.db-wal"),
        evidence.path().join("a.db-wal"),
    )
    .unwrap();
    let first = Database::open(&copied).unwrap();
    assert_eq!(first.mode(), OpenMode::Copy);
    let second = first.reopen().unwrap();
    drop(first);
    assert_eq!(
        count_t(&second),
        3,
        "the copy is still there for the second"
    );
    assert_eq!(listing(evidence.path()), ["a.db", "a.db-wal"]);
}

#[test]
fn something_that_is_not_a_database_is_an_error_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "id,name\n1,x\n".repeat(50)).unwrap();
    let err = Database::open(&path).err().expect("not a database");
    assert!(err.to_string().contains("notes.txt"), "{err}");
    assert!(Database::open(&dir.path().join("missing.db")).is_err());
}

// --- a delimited file's rows, to query -----------------------------------------------------

fn text(rows: &[&[&str]]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|r| r.iter().map(|c| c.to_string()).collect())
        .collect()
}

#[test]
fn columns_are_typed_only_when_every_cell_reads_that_way_exactly() {
    use browse::{ColumnType, infer_type};
    let t = |cells: &[&str]| infer_type(cells.iter().copied());
    assert_eq!(t(&["1", "20", "-3", ""]), ColumnType::Integer);
    assert_eq!(t(&["1", "2.5", "-0.25", "1e3"]), ColumnType::Real);
    assert_eq!(
        t(&["007", "12"]),
        ColumnType::Text,
        "a leading zero is not a number"
    );
    assert_eq!(t(&["+5"]), ColumnType::Text);
    assert_eq!(t(&[" 5"]), ColumnType::Text);
    assert_eq!(t(&["1,5"]), ColumnType::Text);
    assert_eq!(t(&["nan", "1.0"]), ColumnType::Text);
    assert_eq!(
        t(&["12345678901234567890"]),
        ColumnType::Text,
        "too big for an integer"
    );
    assert_eq!(t(&["1", "x"]), ColumnType::Text);
    assert_eq!(t(&["", ""]), ColumnType::Text, "nothing to go on");
    assert_eq!(t(&["0.5", "00.5"]), ColumnType::Text);
}

#[test]
fn a_sheet_becomes_a_table_that_can_be_queried() {
    let rows = text(&[
        &["ws-042", "3389", "0.5", "007"],
        &["ws-043", "22", "", "010"],
        &["srv-db1", "", "1.25"],                // short
        &["ws-017", "443", "2", "011", "extra"], // long
    ]);
    let columns: Vec<String> = ["host", "port", "ratio", "zip"].map(String::from).to_vec();
    let loaded = browse::load_text_table("logons", &columns, &rows).unwrap();
    assert_eq!(loaded.rows, 4);
    assert_eq!(
        loaded.types,
        [
            browse::ColumnType::Text,
            browse::ColumnType::Integer,
            browse::ColumnType::Real,
            browse::ColumnType::Text
        ]
    );
    let schema = loaded.db.schema("logons").unwrap();
    let types: Vec<&str> = schema
        .columns
        .iter()
        .map(|c| c.declared_type.as_str())
        .collect();
    assert_eq!(types, ["TEXT", "INTEGER", "REAL", "TEXT"]);

    let (_, _, out, _) = collect(
        &loaded.db,
        "SELECT host, port, ratio, zip FROM logons ORDER BY port DESC",
        &Budget::default(),
    );
    assert_eq!(
        out[0],
        [
            Value::Text("ws-042".into()),
            Value::Integer(3389),
            Value::Real(0.5),
            Value::Text("007".into())
        ]
    );
    // An empty cell is NULL in a number column; a short row is padded.
    assert_eq!(out.last().unwrap()[1], Value::Null);
    let (_, _, nulls, _) = collect(
        &loaded.db,
        "SELECT count(*) FROM logons WHERE ratio IS NULL",
        &Budget::default(),
    );
    assert_eq!(nulls[0][0], Value::Integer(1));
    let (_, _, text_empty, _) = collect(
        &loaded.db,
        "SELECT count(*) FROM logons WHERE zip = ''",
        &Budget::default(),
    );
    assert_eq!(
        text_empty[0][0],
        Value::Integer(1),
        "the short row's text cell is empty"
    );

    // It is read-only like any other, and a second connection sees the same table.
    let refused = loaded
        .db
        .run("DELETE FROM logons", &Budget::default(), |_| {
            ControlFlow::Continue(())
        })
        .unwrap_err();
    assert!(refused.refused);
    let second = loaded.db.reopen().unwrap();
    drop(loaded);
    assert_eq!(
        second.count("logons", "").unwrap().rows,
        4,
        "the file outlives the first handle"
    );
}

#[test]
fn repeated_and_odd_headers_all_get_a_column() {
    let columns: Vec<String> = ["name", "Name", "", "a\"b", "name"]
        .map(String::from)
        .to_vec();
    let rows = text(&[&["x", "y", "z", "w", "v"]]);
    let loaded = browse::load_text_table("odd \"table", &columns, &rows).unwrap();
    assert_eq!(loaded.columns, ["name", "Name_2", "", "a\"b", "name_3"]);
    let page = loaded
        .db
        .window("odd \"table", &spec("", None, 0, 10))
        .unwrap();
    assert_eq!(page.columns, loaded.columns);
    assert_eq!(page.rows[0][4], Value::Text("v".into()));
    assert!(browse::load_text_table("t", &[], &rows).is_err());
}

#[test]
fn a_large_sheet_loads_and_answers() {
    let columns: Vec<String> = ["id", "label"].map(String::from).to_vec();
    let rows: Vec<Vec<String>> = (0..50_000)
        .map(|i| vec![i.to_string(), format!("row {i}")])
        .collect();
    let loaded = browse::load_text_table("big", &columns, &rows).unwrap();
    let (_, _, out, _) = collect(
        &loaded.db,
        "SELECT count(*), max(id), min(label) FROM big",
        &Budget::default(),
    );
    assert_eq!(
        out[0],
        [
            Value::Integer(50_000),
            Value::Integer(49_999),
            Value::Text("row 0".into())
        ]
    );
}
