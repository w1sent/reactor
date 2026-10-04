//! Handing a case over as one encrypted archive, and unpacking it (ADR-0045).

use std::io::Write;

use reactor_db::export::{self, DEFAULT_PASSWORD};
use reactor_db::graph::{self, Batch, Kind, Link, Put, query};
use reactor_db::manifest::{self, Author, Field};
use reactor_db::sessions::{self, Row};
use reactor_db::{Case, Error, artifact, corpus};

/// A case with a bit of everything: evidence, a session, a goal, a graph, a registered
/// file, a document with a vector, and the agent's data file.
fn busy_case() -> (tempfile::TempDir, Case) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("samples")).unwrap();
    std::fs::write(
        dir.path().join("samples/dropper.bin"),
        b"MZ\x90\x00 pretend malware",
    )
    .unwrap();
    std::fs::write(dir.path().join("notes.txt"), "analyst notes\n").unwrap();
    let data = rusqlite::Connection::open(dir.path().join("reactor_data.sqlite")).unwrap();
    data.execute_batch(
        "PRAGMA journal_mode = WAL; CREATE TABLE fw(src, dst); INSERT INTO fw VALUES ('a','b');",
    )
    .unwrap();
    // Left open with its write-ahead log in place, as a script might leave it.
    std::mem::forget(data);
    let case = Case::create(dir.path(), Some("Incident 17")).unwrap();
    sessions::create(
        case.db(),
        "s1",
        1,
        Row {
            id: 0,
            parent: None,
            ts: 1,
            kind: "session".into(),
            body: r#"{"id":0}"#.into(),
            blob_sha256: None,
        },
    )
    .unwrap();
    manifest::set(
        case.db(),
        Field::Goal,
        Some("find the C2"),
        &Author::analyst(),
        None,
    )
    .unwrap();
    artifact::register(
        &case,
        "samples/dropper.bin",
        Some("sample"),
        None,
        &Author::analyst(),
        None,
    )
    .unwrap();
    let put = Put {
        links: vec![
            Link::entity("src", "host.name", "ws-042"),
            Link::entity("dst", "net.ipv4", "198.51.100.7"),
            Link::source("samples/dropper.bin").at("addr=0x401000", None),
        ],
        ..Put::new(Kind::Event, "net.connection")
    };
    graph::apply(case.db(), Batch::new(Author::analyst()).put(put)).unwrap();
    corpus::add_document(
        &case,
        corpus::NewDocument {
            source_uri: "wiki/emotet.md".into(),
            source_kind: "file".into(),
            mime: None,
            title: Some("Emotet".into()),
            origin: "external".into(),
            fetched_at: None,
            meta: serde_json::json!({}),
            raw: b"# Emotet".to_vec(),
            markdown: "# Emotet".into(),
            chunker: "t".into(),
            chunks: vec![("Emotet".into(), "a banking trojan".into())],
        },
    )
    .unwrap();
    let m = corpus::register_model(case.db(), "ollama/t", "", 3).unwrap();
    let id = corpus::pending(case.db(), m.id, 1).unwrap()[0].0;
    corpus::store_vectors(case.db(), &m, vec![(id, vec![1., 0., 0.])]).unwrap();
    (dir, case)
}

fn names(archive: &std::path::Path) -> Vec<String> {
    let zip = zip::ZipArchive::new(std::fs::File::open(archive).unwrap()).unwrap();
    zip.file_names().map(str::to_string).collect()
}

fn ids(case: &Case) -> (Vec<String>, Vec<String>) {
    let (elements, edges) = query::snapshot(case.db(), true).unwrap();
    let mut e: Vec<String> = elements
        .iter()
        .map(|e| format!("{}@{}", e.id, e.ver))
        .collect();
    let mut g: Vec<String> = edges.iter().map(|g| g.id.clone()).collect();
    e.sort();
    g.sort();
    (e, g)
}

#[test]
fn an_exported_case_unpacks_into_the_same_case() {
    let (dir, case) = busy_case();
    let out = tempfile::tempdir().unwrap();
    let archive = out.path().join("incident-17.zip");

    let report = export::export(&case, &archive, DEFAULT_PASSWORD).unwrap();
    assert_eq!(report.files, 6, "{report:?}"); // case.json, the database, one blob, the sample, the notes, the data file
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    let inside = names(&archive);
    for must in [
        ".reactor/case.json",
        ".reactor/analysis.db",
        "reactor_data.sqlite",
        "samples/dropper.bin",
        "notes.txt",
    ] {
        assert!(
            inside.contains(&must.to_string()),
            "{must} missing from {inside:?}"
        );
    }
    // What is derived, or only means something while the case is open, is not archived.
    let derived: Vec<&String> = inside
        .iter()
        .filter(|n| {
            n.contains("/run/")
                || n.ends_with("-wal")
                || n.ends_with("-shm")
                || n.ends_with("-journal")
        })
        .collect();
    assert!(derived.is_empty(), "archived: {derived:?}");
    // Nothing in it is readable without the password.
    let raw = std::fs::read(&archive).unwrap();
    assert!(
        !raw.windows(11).any(|w| w == b"pretend mal"),
        "plaintext in the archive"
    );
    assert!(!raw.windows(13).any(|w| w == b"analyst notes"));

    let (before_elements, before_edges) = ids(&case);
    let target = out.path().join("unpacked");
    let (back, imported) = export::import(&archive, &target, DEFAULT_PASSWORD).unwrap();
    assert_eq!(imported.files, report.files);
    assert!(imported.missing_blobs.is_empty());

    // The graph is the same graph: the same ids, the same versions, the same edges.
    assert_eq!(ids(&back), (before_elements, before_edges));
    assert_eq!(back.meta().id, case.meta().id);
    assert_eq!(back.meta().title.as_deref(), Some("Incident 17"));
    assert_eq!(sessions::load(back.db(), "s1").unwrap().len(), 1);
    assert_eq!(
        manifest::current(back.db()).unwrap().goal.as_deref(),
        Some("find the C2")
    );
    assert_eq!(artifact::list(&back).unwrap().len(), 1);
    assert_eq!(corpus::counts(back.db()).unwrap(), (1, 1));
    let model = corpus::usable_model(back.db(), None).unwrap().unwrap();
    assert_eq!(model.name, "ollama/t", "the vectors came with it");
    // The evidence is byte for byte what it was, and so is the agent's data.
    assert_eq!(
        std::fs::read(target.join("samples/dropper.bin")).unwrap(),
        std::fs::read(dir.path().join("samples/dropper.bin")).unwrap()
    );
    let data = rusqlite::Connection::open(target.join("reactor_data.sqlite")).unwrap();
    assert_eq!(
        data.query_row("SELECT dst FROM fw", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "b"
    );
    assert_eq!(
        artifact::verify(&back)
            .unwrap()
            .iter()
            .filter(|(_, s)| *s != artifact::State::Unchanged)
            .count(),
        0
    );
}

#[cfg(unix)]
#[test]
fn nothing_comes_out_executable_and_a_link_is_not_followed() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (dir, case) = busy_case();
    let sample = dir.path().join("samples/dropper.bin");
    std::fs::set_permissions(&sample, std::fs::Permissions::from_mode(0o755)).unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(outside.path(), "secret").unwrap();
    symlink(outside.path(), dir.path().join("link-out")).unwrap();

    let out = tempfile::tempdir().unwrap();
    let archive = out.path().join("c.zip");
    let report = export::export(&case, &archive, "my own password").unwrap();
    assert_eq!(report.skipped.len(), 1, "{:?}", report.skipped);
    assert!(
        report.skipped[0].starts_with("link-out"),
        "{:?}",
        report.skipped
    );
    assert!(!names(&archive).contains(&"link-out".to_string()));

    let (_, _) = export::import(&archive, &out.path().join("u"), "my own password").unwrap();
    let mode = std::fs::metadata(out.path().join("u/samples/dropper.bin"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0, "mode {mode:o}");
}

#[test]
fn the_default_password_is_the_samples_convention_and_a_wrong_one_is_refused_clearly() {
    let (_d, case) = busy_case();
    let out = tempfile::tempdir().unwrap();
    let own = out.path().join("own.zip");
    export::export(&case, &own, "correct horse").unwrap();
    for wrong in [DEFAULT_PASSWORD, "correct hors", "x"] {
        let err = export::import(&own, &out.path().join("nope"), wrong).unwrap_err();
        assert!(err.to_string().contains("wrong password"), "{wrong}: {err}");
        assert!(
            !out.path().join("nope").exists(),
            "a failed import leaves nothing"
        );
    }
    assert!(export::import(&own, &out.path().join("yes"), "correct horse").is_ok());
    assert_eq!(DEFAULT_PASSWORD, "infected");
}

#[test]
fn an_export_never_overwrites_and_is_not_made_of_an_unsaved_state_or_with_no_password() {
    let (dir, case) = busy_case();
    let out = tempfile::tempdir().unwrap();
    let archive = out.path().join("c.zip");
    export::export(&case, &archive, DEFAULT_PASSWORD).unwrap();
    assert!(matches!(
        export::export(&case, &archive, DEFAULT_PASSWORD),
        Err(Error::Io(_))
    ));
    assert!(matches!(
        export::export(&case, &out.path().join("d.zip"), ""),
        Err(Error::Invalid(_))
    ));
    assert!(!out.path().join("d.zip").exists());

    let config = tempfile::tempdir().unwrap();
    let plain = tempfile::tempdir().unwrap();
    let unsaved = Case::open_any(plain.path(), config.path()).unwrap();
    let err = export::export(&unsaved, &out.path().join("e.zip"), "x").unwrap_err();
    assert!(err.to_string().contains("unsaved"), "{err}");

    // An archive written inside the case is not part of itself.
    let inside = dir.path().join("handover.zip");
    export::export(&case, &inside, DEFAULT_PASSWORD).unwrap();
    assert!(!names(&inside).contains(&"handover.zip".to_string()));
}

#[test]
fn an_archive_cannot_write_outside_the_folder_it_is_unpacked_into() {
    let out = tempfile::tempdir().unwrap();
    for evil in ["../evil.txt", "/tmp/evil.txt", ".reactor/../../evil.txt"] {
        let archive = out.path().join("evil.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file(".reactor/case.json", opts).unwrap();
        zip.write_all(b"{}").unwrap();
        zip.start_file(evil, opts).unwrap();
        zip.write_all(b"pwned").unwrap();
        zip.finish().unwrap();
        let err = export::import(&archive, &out.path().join("target"), "x").unwrap_err();
        assert!(matches!(err, Error::Invalid(_)), "{evil}: {err}");
        assert!(err.to_string().contains("outside the case"), "{err}");
        assert!(!out.path().join("evil.txt").exists());
        assert!(!out.path().join("target").exists());
        let _ = std::fs::remove_file(&archive);
    }
}

#[test]
fn only_an_exported_case_into_an_empty_folder_is_unpacked() {
    let (_d, case) = busy_case();
    let out = tempfile::tempdir().unwrap();
    let archive = out.path().join("c.zip");
    export::export(&case, &archive, DEFAULT_PASSWORD).unwrap();
    // Not empty: refused, and what was there is untouched.
    let busy = out.path().join("busy");
    std::fs::create_dir(&busy).unwrap();
    std::fs::write(busy.join("mine.txt"), "keep").unwrap();
    assert!(export::import(&archive, &busy, DEFAULT_PASSWORD).is_err());
    assert_eq!(
        std::fs::read_to_string(busy.join("mine.txt")).unwrap(),
        "keep"
    );
    // An existing empty folder is fine, and survives a failure empty.
    let empty = out.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    assert!(export::import(&archive, &empty, "wrong").is_err());
    assert!(empty.is_dir() && std::fs::read_dir(&empty).unwrap().count() == 0);
    // Some other zip, and not a zip at all.
    let other = out.path().join("other.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&other).unwrap());
    zip.start_file("readme.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"hi").unwrap();
    zip.finish().unwrap();
    let err = export::import(&other, &out.path().join("x"), "x").unwrap_err();
    assert!(err.to_string().contains("not an exported case"), "{err}");
    std::fs::write(out.path().join("junk.zip"), b"not a zip").unwrap();
    assert!(export::import(&out.path().join("junk.zip"), &out.path().join("y"), "x").is_err());
    assert!(export::import(&out.path().join("missing.zip"), &out.path().join("z"), "x").is_err());
}
