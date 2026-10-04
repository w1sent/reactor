//! What other processes do to a case: a writer killed mid-append, and two windows on one
//! case at once (ADR-0046). The children are this test binary, re-run on the ignored
//! helper below.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use reactor_db::Case;
use reactor_db::sessions::{self, Row};

const DIR: &str = "REACTOR_DB_TEST_CASE";
const SESSION: &str = "REACTOR_DB_TEST_SESSION";
const COUNT: &str = "REACTOR_DB_TEST_COUNT";

fn row(id: u64) -> Row {
    Row {
        id,
        parent: id.checked_sub(1),
        ts: id,
        kind: if id == 0 { "session" } else { "user" }.into(),
        body: format!(
            r#"{{"id":{id},"type":"user","text":"{}"}}"#,
            "x".repeat(200)
        ),
        blob_sha256: None,
    }
}

/// Not a test: the child process. Appends `COUNT` entries (or forever) to `SESSION`.
#[test]
#[ignore]
fn child_appends() {
    let Ok(dir) = std::env::var(DIR) else { return };
    let session = std::env::var(SESSION).unwrap();
    let count: Option<u64> = std::env::var(COUNT).ok().and_then(|c| c.parse().ok());
    let case = Case::open(dir.as_ref()).unwrap();
    let _lock = case.lock_session(&session).unwrap();
    sessions::create(case.db(), &session, 1, row(0)).unwrap();
    let mut id = 1;
    while count.is_none_or(|c| id <= c) {
        sessions::append(case.db(), &session, row(id)).unwrap();
        id += 1;
    }
}

fn spawn(dir: &std::path::Path, session: &str, count: Option<u64>) -> Child {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "child_appends", "--ignored", "--test-threads=1"])
        .env(DIR, dir)
        .env(SESSION, session)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(c) = count {
        cmd.env(COUNT, c.to_string());
    }
    cmd.spawn().unwrap()
}

fn dense(case: &Case, session: &str) -> u64 {
    let rows = sessions::load(case.db(), session).unwrap();
    for (i, r) in rows.iter().enumerate() {
        assert_eq!(r.id, i as u64, "ids are dense");
        assert!(
            serde_json::from_str::<serde_json::Value>(&r.body).is_ok(),
            "entry #{i} is whole"
        );
    }
    rows.len() as u64
}

#[test]
fn a_writer_killed_mid_append_leaves_a_session_that_reopens_and_continues() {
    let dir = tempfile::tempdir().unwrap();
    let case = Case::create(dir.path(), None).unwrap();
    let mut child = spawn(dir.path(), "s1", None);

    // Let it get going, then kill it without warning.
    let start = Instant::now();
    while sessions::load(case.db(), "s1")
        .map(|r| r.len())
        .unwrap_or(0)
        < 50
    {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "the child never wrote"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    child.kill().unwrap();
    child.wait().unwrap();

    let n = dense(&case, "s1");
    assert!(n >= 50);
    // Its lock died with it, and the next append lands on a clean log.
    let _lock = case.lock_session("s1").unwrap();
    sessions::append(case.db(), "s1", row(n)).unwrap();
    assert_eq!(dense(&case, "s1"), n + 1);
    assert_eq!(case.blobs().fsck().unwrap(), Default::default());

    let ok: String = case
        .db()
        .read(|conn| Ok(conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(ok, "ok");
}

#[test]
fn two_processes_write_two_sessions_of_one_case_without_lock_errors() {
    let dir = tempfile::tempdir().unwrap();
    let case = Case::create(dir.path(), None).unwrap();
    let mut a = spawn(dir.path(), "a", Some(300));
    let mut b = spawn(dir.path(), "b", Some(300));
    assert!(a.wait().unwrap().success(), "process a failed");
    assert!(b.wait().unwrap().success(), "process b failed");
    assert_eq!(dense(&case, "a"), 301);
    assert_eq!(dense(&case, "b"), 301);
    assert_eq!(sessions::list(case.db()).unwrap().len(), 2);
}
