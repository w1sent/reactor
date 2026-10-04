//! The case directory, the unsaved state, sessions, blobs and the case manifest.

use reactor_db::manifest::{self, Author, Field};
use reactor_db::sessions::{self, Row};
use reactor_db::{Case, case};

fn row(id: u64, kind: &str, body: &str) -> Row {
    Row {
        id,
        parent: id.checked_sub(1),
        ts: 1_000 + id,
        kind: kind.into(),
        body: body.into(),
        blob_sha256: None,
    }
}

fn session(case: &Case, id: &str) {
    sessions::create(
        case.db(),
        id,
        1,
        row(0, "session", r#"{"id":0,"type":"session"}"#),
    )
    .unwrap();
}

#[test]
fn creating_a_case_makes_the_folder_one_and_it_reopens() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!Case::is_case(dir.path()));
    let c = Case::create(dir.path(), Some("Incident 17")).unwrap();
    assert!(c.is_saved());
    assert!(Case::is_case(dir.path()));
    assert!(dir.path().join(".reactor/analysis.db").is_file());
    let id = c.meta().id.clone();
    drop(c);

    let c = Case::open(dir.path()).unwrap();
    assert_eq!(c.meta().id, id);
    assert_eq!(c.meta().title.as_deref(), Some("Incident 17"));
    assert!(Case::create(dir.path(), None).is_err(), "not twice");
}

#[test]
fn a_reactor_directory_without_case_json_is_not_a_case() {
    // ADR-0003's project-scoped state.json lives there too.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".reactor")).unwrap();
    std::fs::write(dir.path().join(".reactor/state.json"), "{}").unwrap();
    assert!(!Case::is_case(dir.path()));
    assert!(Case::open(dir.path()).is_err());
}

#[test]
fn opening_a_plain_folder_leaves_nothing_in_it_and_saving_moves_everything() {
    let (work, config) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let c = Case::open_any(work.path(), config.path()).unwrap();
    assert!(!c.is_saved());
    assert!(c.dir().starts_with(config.path()));
    assert_eq!(std::fs::read_dir(work.path()).unwrap().count(), 0);

    session(&c, "s1");
    sessions::append(
        c.db(),
        "s1",
        row(1, "user", r#"{"id":1,"type":"user","text":"hi"}"#),
    )
    .unwrap();
    let (sha, _) = c.blobs().put(b"whole output").unwrap();
    std::fs::write(c.session_dir("s1").unwrap().join("activation.json"), "{}").unwrap();
    manifest::set(
        c.db(),
        Field::Goal,
        Some("find the C2"),
        &Author::analyst(),
        None,
    )
    .unwrap();
    let id = c.meta().id.clone();
    drop(c);

    // The same folder gets the same unsaved state back.
    assert!(Case::has_unsaved(work.path(), config.path()));
    let c = Case::open_any(work.path(), config.path()).unwrap();
    assert_eq!(c.meta().id, id);

    let c = c.save().unwrap();
    assert!(c.is_saved());
    assert!(Case::is_case(work.path()));
    assert_eq!(c.meta().id, id);
    assert_eq!(sessions::load(c.db(), "s1").unwrap().len(), 2);
    assert_eq!(c.blobs().read(&sha).unwrap(), b"whole output");
    assert!(
        c.session_dir("s1")
            .unwrap()
            .join("activation.json")
            .is_file()
    );
    assert_eq!(
        manifest::current(c.db()).unwrap().goal.as_deref(),
        Some("find the C2")
    );
    // Nothing is left behind in the config directory.
    assert!(!Case::has_unsaved(work.path(), config.path()));
    assert_eq!(
        std::fs::read_dir(config.path().join("unsaved"))
            .unwrap()
            .count(),
        0
    );
    assert!(c.save().is_err(), "a saved case is not saved again");
}

#[test]
fn saving_keeps_what_was_already_in_the_reactor_directory() {
    let (work, config) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::create_dir(work.path().join(".reactor")).unwrap();
    std::fs::write(work.path().join(".reactor/state.json"), "{\"x\":1}").unwrap();
    let c = Case::open_any(work.path(), config.path()).unwrap();
    assert!(!c.is_saved());
    c.save().unwrap();
    assert_eq!(
        std::fs::read_to_string(work.path().join(".reactor/state.json")).unwrap(),
        "{\"x\":1}"
    );
}

#[test]
fn a_case_in_use_is_not_moved_and_an_unsaved_state_can_be_discarded() {
    let (work, config) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let c = Case::open_any(work.path(), config.path()).unwrap();
    let other = c.clone();
    assert!(c.save().is_err());
    other.discard().unwrap();
    assert!(!Case::has_unsaved(work.path(), config.path()));

    let saved = Case::create(work.path(), None).unwrap();
    assert!(
        saved.discard().is_err(),
        "only an unsaved state is discarded"
    );
}

#[test]
fn nothing_in_the_case_directory_names_where_it_is() {
    let (work, config) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let c = Case::open_any(work.path(), config.path()).unwrap();
    session(&c, "s1");
    let dir = c.dir().to_path_buf();
    drop(c);
    let needle = work.path().canonicalize().unwrap();
    let needle = needle.to_string_lossy();
    for name in ["case.json", "analysis.db"] {
        let bytes = std::fs::read(dir.join(name)).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(needle.as_ref()),
            "{name} stores the workdir's absolute path"
        );
    }
}

#[test]
fn the_log_is_append_only_dense_and_exports_as_json_lines() {
    let dir = tempfile::tempdir().unwrap();
    let c = Case::create(dir.path(), None).unwrap();
    session(&c, "s1");
    sessions::append(
        c.db(),
        "s1",
        row(1, "user", r#"{"id":1,"type":"user","text":"look"}"#),
    )
    .unwrap();
    assert!(
        sessions::append(c.db(), "s1", row(5, "user", "{}")).is_err(),
        "ids are dense"
    );
    assert!(
        sessions::create(c.db(), "s1", 1, row(0, "session", "{}")).is_err(),
        "a session is created once"
    );
    for sql in [
        "UPDATE entry SET body = '{}' WHERE id = 1",
        "DELETE FROM entry WHERE id = 1",
    ] {
        let err = c
            .db()
            .write(move |tx| Ok(tx.execute(sql, [])?))
            .unwrap_err();
        assert!(err.to_string().contains("append-only"), "{err}");
    }

    let mut out = Vec::new();
    assert_eq!(sessions::export_jsonl(c.db(), "s1", &mut out).unwrap(), 2);
    let text = String::from_utf8(out).unwrap();
    assert_eq!(text.lines().count(), 2);
    assert!(text.lines().nth(1).unwrap().contains("\"look\""));

    let listed = sessions::list(c.db()).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].entries, listed[0].first_prompt.as_deref()),
        (2, Some("look"))
    );
}

#[test]
fn a_failed_write_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let c = Case::create(dir.path(), None).unwrap();
    session(&c, "s1");
    let r: reactor_db::Result<()> = c.db().write(|tx| {
        tx.execute(
            "INSERT INTO entry(session_id, id, ts, type, body) VALUES ('s1', 1, 0, 'user', '{}')",
            [],
        )?;
        Err(reactor_db::Error::Sqlite("no".into()))
    });
    assert!(r.is_err());
    assert_eq!(sessions::load(c.db(), "s1").unwrap().len(), 1);
}

#[test]
fn a_session_is_one_frontends_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    let c = Case::create(dir.path(), None).unwrap();
    let held = c.lock_session("s1").unwrap();
    assert!(c.lock_session("s1").is_err());
    c.lock_session("s2").unwrap();
    drop(held);
    c.lock_session("s1").unwrap();
}

#[test]
fn blobs_are_named_by_content_and_fsck_reconciles() {
    let dir = tempfile::tempdir().unwrap();
    let c = Case::create(dir.path(), None).unwrap();
    let (a, size) = c.blobs().put(b"same bytes").unwrap();
    let (b, _) = c.blobs().put(b"same bytes").unwrap();
    assert_eq!((a.as_str(), size), (b.as_str(), 10));
    assert_eq!(a.len(), 64);

    let spill = dir.path().join("spill.txt");
    std::fs::write(&spill, b"spilled").unwrap();
    let (s, _) = c.blobs().adopt(&spill).unwrap();
    assert!(!spill.exists(), "the original is removed");
    assert_eq!(c.blobs().read(&s).unwrap(), b"spilled");
    assert_eq!(c.blobs().fsck().unwrap(), Default::default());

    // A file without a row (a crash between the two) is adopted; a row without its
    // file is reported; a file that is not what its name says is left alone.
    let orphan = c.blobs().path(&"0".repeat(64));
    std::fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    std::fs::write(&orphan, b"not zeros").unwrap();
    let real = {
        use sha2::Digest;
        let d = sha2::Sha256::digest(b"crashed here");
        d.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    let real_path = c.blobs().path(&real);
    std::fs::create_dir_all(real_path.parent().unwrap()).unwrap();
    std::fs::write(&real_path, b"crashed here").unwrap();
    std::fs::remove_file(c.blobs().path(&s)).unwrap();

    let report = c.blobs().fsck().unwrap();
    assert_eq!(report.missing, [s]);
    assert_eq!(report.adopted, std::slice::from_ref(&real));
    assert_eq!(report.corrupt, [orphan]);
    assert_eq!(c.blobs().fsck().unwrap().adopted, Vec::<String>::new());
}

#[test]
fn the_goal_and_guidelines_are_versioned_and_belong_to_the_case() {
    let dir = tempfile::tempdir().unwrap();
    let c = Case::create(dir.path(), None).unwrap();
    assert_eq!(manifest::current(c.db()).unwrap(), Default::default());

    let me = Author::analyst();
    assert!(manifest::set(c.db(), Field::Goal, Some("one"), &me, Some("s1")).unwrap());
    assert!(!manifest::set(c.db(), Field::Goal, Some("one"), &me, None).unwrap());
    assert!(manifest::set(c.db(), Field::Goal, Some("two"), &Author::agent("m"), None).unwrap());
    assert!(manifest::set(c.db(), Field::Guidelines, Some("be brief"), &me, None).unwrap());
    let now = manifest::current(c.db()).unwrap();
    assert_eq!(
        (now.goal.as_deref(), now.guidelines.as_deref()),
        (Some("two"), Some("be brief"))
    );

    assert!(manifest::set(c.db(), Field::Goal, None, &me, None).unwrap());
    assert_eq!(manifest::current(c.db()).unwrap().goal, None);
    let versions: i64 = c
        .db()
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT count(*) FROM case_manifest WHERE field = 'goal'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(versions, 3, "history is kept, clearing included");
}

#[test]
fn a_backup_is_a_whole_database_in_one_file() {
    let dir = tempfile::tempdir().unwrap();
    let c = Case::create(dir.path(), None).unwrap();
    session(&c, "s1");
    let dest = dir.path().join("copy.db");
    c.db().backup_to(&dest).unwrap();
    assert!(c.db().backup_to(&dest).is_err(), "never overwrites");
    let copy = reactor_db::Db::open(&dest).unwrap();
    assert_eq!(sessions::load(&copy, "s1").unwrap().len(), 1);
}

#[test]
fn the_machine_remembers_its_cases_most_recent_first() {
    let (a, b, config) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    Case::create(a.path(), Some("A")).unwrap();
    Case::create(b.path(), None).unwrap();
    case::remember(config.path(), a.path()).unwrap();
    case::remember(config.path(), b.path()).unwrap();
    case::remember(config.path(), a.path()).unwrap();
    let known = case::known(config.path());
    assert_eq!(known.len(), 2);
    assert_eq!(known[0].meta.as_ref().unwrap().title.as_deref(), Some("A"));
    drop(b);
    assert!(
        case::known(config.path())[1].meta.is_none(),
        "a vanished case is shown as gone"
    );
}
