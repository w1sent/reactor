use reactor_db::{selfcheck, sqlite};

#[test]
fn this_build_has_fts5_sqlite_vec_and_json() {
    let report = selfcheck::run().expect("self-check");
    assert!(report.fts5 && report.json);
    assert!(
        report.vec_version.starts_with('v'),
        "{}",
        report.vec_version
    );
}

#[test]
fn a_file_database_is_wal_and_a_read_connection_cannot_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("analysis.db");
    let rw = sqlite::open_rw(&path).unwrap();
    let mode: String = rw
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    rw.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1);")
        .unwrap();

    let ro = sqlite::open_ro(&path).unwrap();
    let n: i64 = ro
        .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    assert!(ro.execute("INSERT INTO t VALUES (2)", []).is_err());
    // The extension is there on every connection, not only the first.
    let v: String = ro
        .query_row("SELECT vec_version()", [], |r| r.get(0))
        .unwrap();
    assert!(!v.is_empty());
}
