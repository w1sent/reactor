//! Reading any SQLite file for the table viewer.

use reactor_db::browse::{self, MAGIC};
use reactor_db::{Case, graph};

fn data() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reactor_data.sqlite");
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch(
        "PRAGMA journal_mode = DELETE;
         CREATE TABLE fw_conn (id INTEGER PRIMARY KEY, src TEXT, bytes BLOB, ratio REAL, note TEXT);
         INSERT INTO fw_conn VALUES (1, 'ws-042', x'0102030405060708090a0b0c0d0e0f101112', 0.5, NULL),
                                    (2, 'ws-043', x'ff', 1.25, 'second');
         CREATE VIEW big AS SELECT * FROM fw_conn WHERE id > 1;
         CREATE TABLE \"odd \"\"name\" (x);
         INSERT INTO \"odd \"\"name\" VALUES ('ok');",
    )
    .unwrap();
    (dir, path)
}

#[test]
fn a_database_is_recognised_by_its_first_bytes() {
    let (_d, path) = data();
    let head = std::fs::read(&path).unwrap();
    assert!(browse::is_sqlite(&head[..100]));
    assert!(!browse::is_sqlite(b"id,name\n1,x\n"));
    assert!(!browse::is_sqlite(&MAGIC[..8]));
}

#[test]
fn tables_and_views_are_listed_with_their_rows() {
    let (_d, path) = data();
    let t = browse::tables(&path).unwrap();
    let names: Vec<(&str, bool, String)> = t
        .iter()
        .map(|t| (t.name.as_str(), t.is_view, t.rows_label()))
        .collect();
    assert_eq!(
        names,
        [
            ("big", true, "view".to_string()),
            ("fw_conn", false, "2 rows".to_string()),
            ("odd \"name", false, "1 row".to_string()),
        ]
    );
}

#[test]
fn rows_come_back_as_text_with_the_column_names_first_and_nothing_is_changed() {
    let (_d, path) = data();
    let before = std::fs::read(&path).unwrap();
    let r = browse::read_table(&path, "fw_conn", 10).unwrap();
    assert!(!r.truncated);
    assert_eq!(r.rows[0], ["id", "src", "bytes", "ratio", "note"]);
    assert_eq!(
        r.rows[1],
        [
            "1",
            "ws-042",
            "x'0102030405060708090a0b0c0d0e0f10…' (18 bytes)",
            "0.5",
            "NULL"
        ]
    );
    assert_eq!(r.rows[2], ["2", "ws-043", "x'ff'", "1.25", "second"]);
    // A view reads like a table; an odd name is quoted, not interpreted.
    assert_eq!(browse::read_table(&path, "big", 10).unwrap().rows.len(), 2);
    assert_eq!(
        browse::read_table(&path, "odd \"name", 10).unwrap().rows[1],
        ["ok"]
    );
    assert!(browse::read_table(&path, "nope", 10).is_err());
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the file is not touched"
    );
}

#[test]
fn a_long_table_is_cut_and_says_so() {
    let (_d, path) = data();
    let r = browse::read_table(&path, "fw_conn", 1).unwrap();
    assert!(r.truncated);
    assert_eq!(r.rows.len(), 2, "the header and one row");
}

#[test]
fn the_cases_own_database_reads_too_with_its_virtual_tables_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let case = Case::create(dir.path(), None).unwrap();
    use graph::{Batch, Put};
    graph::apply(
        case.db(),
        Batch::new(reactor_db::manifest::Author::analyst()).put(Put::entity("host.name", "ws-042")),
    )
    .unwrap();
    // A vector table, as the corpus has once a model is chosen. The keyword index
    // (`chunk_fts`) is already there.
    case.db()
        .write(|tx| {
            tx.execute_batch(
                "CREATE VIRTUAL TABLE chunk_vec_1 USING vec0(chunk_id INTEGER PRIMARY KEY, embedding float[4]);",
            )?;
            Ok(())
        })
        .unwrap();
    let path = case.dir().join("analysis.db");
    let tables = browse::tables(&path).unwrap();
    let names: Vec<&str> = tables.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names.contains(&"element")
            && names.contains(&"chunk_vec_1")
            && names.contains(&"chunk_fts")
    );
    assert!(
        !names
            .iter()
            .any(|n| n.starts_with("chunk_fts_") || n.starts_with("chunk_vec_1_")),
        "{names:?}"
    );
    let r = browse::read_table(&path, "element", 10).unwrap();
    assert_eq!(r.rows.len(), 2);
    assert_eq!(r.rows[1][1], "entity");
    assert_eq!(
        browse::read_table(&path, "chunk_vec_1", 10)
            .unwrap()
            .rows
            .len(),
        1
    );
}
