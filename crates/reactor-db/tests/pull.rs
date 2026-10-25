//! Pulling rows of an external into the graph (ADR-0047).

use reactor_db::graph::pull::{self, Report, Request};
use reactor_db::graph::{Kind, query};
use reactor_db::manifest::Author;
use reactor_db::{Case, Error};

fn case() -> (tempfile::TempDir, Case) {
    let dir = tempfile::tempdir().unwrap();
    let data = rusqlite::Connection::open(dir.path().join("reactor_data.sqlite")).unwrap();
    data.execute_batch(
        "CREATE TABLE vpn_auth (record_id INTEGER PRIMARY KEY, ts TEXT, src_ip TEXT,
                                username TEXT, hostname TEXT, ok INTEGER);
         INSERT INTO vpn_auth VALUES
           (1204, '2026-10-01T03:12:07Z', '198.51.100.7', 'jdoe', 'WS-042', 1),
           (1205, '2026-10-01 03:14:00',  '198.51.100.7', 'JDOE', 'ws-042', 0),
           (1206, 'last tuesday',         '203.0.113.9',  'asmith', NULL, 1);",
    )
    .unwrap();
    drop(data);
    std::fs::create_dir(dir.path().join("data")).unwrap();
    std::fs::write(
        dir.path().join("data/pdns.csv"),
        "domain,ip,first_seen,last_seen\ncdn-sync.example,198.51.100.7,2026-09-28,2026-10-02\nOther.Example.,203.0.113.9,2026-09-01,\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("data/edr.jsonl"),
        "{\"ts\":1791117296,\"host\":\"ws-042\",\"proc\":{\"name\":\"powershell.exe\",\"pid\":4312}}\n\n{\"ts\":1791117300,\"host\":\"ws-042\",\"proc\":{\"name\":\"rundll32.exe\",\"pid\":4400}}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("data/report.json"),
        r#"{"meta":{"tool":"x"},"hits":[{"sha256":"AA11","name":"a.dll"},{"sha256":"bb22","name":"b.dll"}]}"#,
    )
    .unwrap();
    let case = Case::create(dir.path(), None).unwrap();
    (dir, case)
}

fn run(case: &Case, source: &str, query: &str) -> reactor_db::Result<Report> {
    pull::pull(case, request(source, query))
}

fn request(source: &str, query: &str) -> Request {
    Request {
        source: source.into(),
        query: query.into(),
        author: Author::agent("model"),
        on_behalf_of: Some(Author::analyst()),
        session: Some("s1".into()),
        entry: Some(7),
        max_rows: 0,
        preview: false,
    }
}

const LOGONS: &str = r#"SELECT ts AS "@time", record_id AS "@row", 'auth.logon' AS "@type",
       src_ip AS "net.ipv4@src", username AS "account.user@actor",
       hostname AS "host.name@target", ok
  FROM src ORDER BY record_id"#;

#[test]
fn rows_of_a_table_become_sourced_claims_and_a_second_pull_changes_nothing() {
    let (_d, case) = case();
    let db = case.db();
    let r = run(&case, "artifact:reactor_data.sqlite#table=vpn_auth", LOGONS).unwrap();
    assert_eq!((r.rows, r.claims.len()), (3, 3));
    assert_eq!(r.source, "artifact:reactor_data.sqlite#table=vpn_auth");
    assert!(r.applied.event_seq.is_some() && r.skipped.is_empty() && r.overlaps.is_empty());

    let first = query::get(db, &r.claims[0]).unwrap().unwrap();
    assert_eq!(
        (first.kind, first.type_.as_str()),
        (Kind::Event, "auth.logon")
    );
    assert_eq!(first.author, "agent:reactor@model");
    assert_eq!(
        first.content.time.occurred_at.as_deref(),
        Some("2026-10-01T03:12:07.000Z")
    );
    assert_eq!(first.content.time.tz_certainty.as_deref(), Some("stated"));
    assert_eq!(
        first.content.time.raw.as_deref(),
        Some("2026-10-01T03:12:07Z")
    );
    assert_eq!(first.content.props["ok"], 1, "a plain column is a property");
    // No zone in the source: UTC is assumed, and it says so. Not a time at all: the
    // raw text is kept and nothing is guessed.
    let second = query::get(db, &r.claims[1]).unwrap().unwrap();
    assert_eq!(second.content.time.tz_certainty.as_deref(), Some("assumed"));
    let third = query::get(db, &r.claims[2]).unwrap().unwrap();
    assert_eq!(third.content.time.occurred_at, None);
    assert_eq!(third.content.time.raw.as_deref(), Some("last tuesday"));
    assert_eq!(third.content.time.tz_certainty.as_deref(), Some("unknown"));

    // The source edge says which row, and what it held.
    let source = query::edges_from(db, &r.claims[0])
        .unwrap()
        .into_iter()
        .find(|e| e.role == "source")
        .unwrap();
    assert_eq!(source.locator.as_deref(), Some("row=1204"));
    assert!(source.fingerprint.is_some());
    // A NULL is no entity: the third logon has no target.
    assert_eq!(query::edges_from(db, &r.claims[2]).unwrap().len(), 3);

    // Two rows named the same address and the same host: one node each.
    let ip = query::entity_id(db, "net.ipv4", "198.51.100.7", None).unwrap();
    let host = query::entity_id(db, "host.name", "ws-042", None).unwrap();
    assert_eq!(query::edges_to(db, &ip).unwrap().len(), 2);
    assert_eq!(query::edges_to(db, &host).unwrap().len(), 2);

    // The pull is itself an event with its query, sourced to the external.
    let pulls = query::find(
        db,
        &query::Filter {
            type_prefix: Some("analysis.pull".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(pulls.len(), 1);
    assert_eq!(pulls[0].content.props["rows"], 3);
    assert!(
        pulls[0].content.props["query"]
            .as_str()
            .unwrap()
            .contains("auth.logon")
    );

    let again = run(&case, "artifact:reactor_data.sqlite#table=vpn_auth", LOGONS).unwrap();
    assert_eq!(again.claims, r.claims);
    assert_eq!(again.applied.event_seq, None, "nothing was written");
    assert_eq!((again.applied.created, again.applied.versioned), (0, 0));
}

#[test]
fn a_csv_a_jsonl_and_a_json_file_are_pulled_the_same_way() {
    let (_d, case) = case();
    let db = case.db();
    let r = run(
        &case,
        "data/pdns.csv",
        r#"SELECT 'net.resolution' AS "@type", _row AS "@row", first_seen AS "@valid_from",
                  last_seen AS "@valid_to", domain AS "net.domain@query", ip AS "net.ipv4@answer"
             FROM src"#,
    )
    .unwrap();
    assert_eq!((r.source.as_str(), r.rows), ("artifact:data/pdns.csv", 2));
    let obs = query::get(db, &r.claims[1]).unwrap().unwrap();
    assert_eq!(
        obs.kind,
        Kind::Observation,
        "no @time: it holds, it did not happen"
    );
    assert_eq!(
        obs.content.time.valid_from.as_deref(),
        Some("2026-09-01T00:00:00.000Z")
    );
    assert_eq!(obs.content.time.valid_to, None, "an open end");
    assert_eq!(obs.content.time.precision.as_deref(), Some("day"));
    let domain = query::entity_id(db, "net.domain", "other.example", None).unwrap();
    assert_eq!(query::edges_to(db, &domain).unwrap().len(), 1);
    assert_eq!(query::relations(db, None).unwrap().len(), 2);

    // JSONL: top-level keys are columns, nested values are JSON, blank lines are skipped.
    let r = run(
        &case,
        "artifact:data/edr.jsonl",
        r#"SELECT 'process.start' AS "@type", ts AS "@time", _row AS "@row",
                  host AS "host.name@target", json_extract(proc, '$.name') AS "process.name@child",
                  json_extract(proc, '$.pid') AS pid
             FROM src"#,
    )
    .unwrap();
    assert_eq!(r.rows, 2);
    let start = query::get(db, &r.claims[0]).unwrap().unwrap();
    assert_eq!(
        start.content.time.occurred_at.as_deref(),
        Some("2026-10-04T12:34:56.000Z")
    );
    assert_eq!(start.content.props["pid"], 4312);

    // JSON: the array a pointer selects. Without @row, a row is located by its content.
    let r = run(
        &case,
        "artifact:data/report.json#/hits",
        r#"SELECT 'file.seen' AS "@type", sha256 AS "file.hash.sha256@subject",
                  name AS "file.name@object" FROM src"#,
    )
    .unwrap();
    assert_eq!(
        (r.source.as_str(), r.rows),
        ("artifact:data/report.json#/hits", 2)
    );
    let hash = query::entity_id(db, "sha256", "aa11", None).unwrap();
    assert_eq!(
        query::get(db, &hash)
            .unwrap()
            .unwrap()
            .content
            .value
            .as_deref(),
        Some("aa11")
    );
    let source = query::edges_from(db, &r.claims[0])
        .unwrap()
        .into_iter()
        .find(|e| e.role == "source")
        .unwrap();
    assert!(source.locator.unwrap().starts_with("row=sha="));
}

#[test]
fn a_pull_query_can_only_read() {
    let (dir, case) = case();
    let table = "artifact:reactor_data.sqlite#table=vpn_auth";
    for bad in [
        "DELETE FROM src",
        "UPDATE data.vpn_auth SET username = 'x'",
        "DROP TABLE data.vpn_auth",
        "CREATE TABLE t(x)",
        "ATTACH DATABASE '/tmp/x.db' AS other",
        "PRAGMA journal_mode = DELETE",
        r#"SELECT load_extension('x') AS "@type""#,
    ] {
        let e = run(&case, table, bad).unwrap_err();
        assert!(matches!(e, Error::Invalid(_)), "{bad}: {e}");
    }
    // The table is as it was.
    let data = rusqlite::Connection::open(dir.path().join("reactor_data.sqlite")).unwrap();
    let n: i64 = data
        .query_row("SELECT count(*) FROM vpn_auth", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 3);

    let e = run(&case, table, "SELECT username FROM src").unwrap_err();
    assert!(e.to_string().contains("@type"), "{e}");
    let e = pull::pull(
        &case,
        Request {
            max_rows: 2,
            ..request(table, LOGONS)
        },
    )
    .unwrap_err();
    assert!(e.to_string().contains("more than 2 rows"), "{e}");
    assert!(run(&case, "artifact:reactor_data.sqlite#table=nope", LOGONS).is_err());
    assert!(run(&case, "artifact:../outside.csv", LOGONS).is_err());
    assert!(run(&case, "notes.txt", LOGONS).is_err());
    // None of that left anything in the graph.
    assert!(
        query::find(case.db(), &Default::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_preview_writes_nothing_and_an_overlapping_pull_is_pointed_out() {
    let (_d, case) = case();
    let table = "artifact:reactor_data.sqlite#table=vpn_auth";
    let preview = pull::pull(
        &case,
        Request {
            preview: true,
            ..request(table, LOGONS)
        },
    )
    .unwrap();
    assert_eq!(preview.rows, 3);
    assert!(preview.claims.is_empty());
    assert!(
        preview.sample[0].contains("event auth.logon")
            && preview.sample[0].contains("src=net.ipv4:198.51.100.7"),
        "{:?}",
        preview.sample
    );
    assert!(
        query::find(case.db(), &Default::default())
            .unwrap()
            .is_empty()
    );

    run(&case, table, LOGONS).unwrap();
    // A whole-file attach, and an aggregate over the same rows.
    let aggregate = r#"SELECT 'auth.logon.summary' AS "@type", src_ip AS "@row",
                              src_ip AS "net.ipv4@src", count(*) AS logons
                         FROM data.vpn_auth GROUP BY src_ip"#;
    let r = run(&case, table, aggregate).unwrap();
    assert_eq!(r.rows, 2);
    assert_eq!(r.overlaps.len(), 1, "the earlier pull of the same table");
    assert!(r.overlaps[0].contains("auth.logon"));
}

#[test]
fn a_source_that_changed_since_it_was_pulled_is_reported() {
    let (dir, case) = case();
    run(
        &case,
        "artifact:data/pdns.csv",
        r#"SELECT 'net.resolution' AS "@type", domain AS "net.domain@query" FROM src"#,
    )
    .unwrap();
    assert!(pull::changed_sources(&case).unwrap().is_empty());
    std::fs::write(
        dir.path().join("data/pdns.csv"),
        "domain,ip\nnew.example,1.2.3.4\n",
    )
    .unwrap();
    assert_eq!(
        pull::changed_sources(&case).unwrap(),
        ["artifact:data/pdns.csv"]
    );
}
