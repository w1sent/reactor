//! The registry of referenced files (ADR-0045): in place, by relative path and hash.

use reactor_db::artifact::{self, State};
use reactor_db::manifest::Author;
use reactor_db::{Case, Error};

fn case() -> (tempfile::TempDir, Case) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("samples/out")).unwrap();
    std::fs::write(
        dir.path().join("samples/dropper.bin"),
        b"MZ\x90\x00 not really",
    )
    .unwrap();
    std::fs::write(dir.path().join("samples/out/unpacked.bin"), b"payload").unwrap();
    let case = Case::create(dir.path(), None).unwrap();
    (dir, case)
}

fn events(case: &Case) -> i64 {
    case.db()
        .read(|c| Ok(c.query_row("SELECT count(*) FROM event", [], |r| r.get(0))?))
        .unwrap()
}

#[test]
fn a_file_is_registered_where_it_is_with_its_hashes_and_no_copy() {
    let (dir, case) = case();
    let me = Author::analyst();
    let a = artifact::register(
        &case,
        "samples/dropper.bin",
        Some("Sample"),
        None,
        &me,
        None,
    )
    .unwrap();
    assert_eq!(a.path, "samples/dropper.bin");
    assert_eq!(a.kind.as_deref(), Some("sample"));
    assert_eq!(
        (a.sha256.len(), a.sha1.len(), a.md5.len(), a.size),
        (64, 40, 32, 15)
    );
    // Nothing was copied: the blob store is empty and the file is where it was.
    assert!(!case.dir().join("blobs/sha256").exists());
    assert!(dir.path().join("samples/dropper.bin").is_file());

    // An absolute path inside the case is the same file; a child names its parent.
    let abs = dir.path().join("samples/out/unpacked.bin");
    let child = artifact::register(
        &case,
        abs.to_str().unwrap(),
        Some("extracted_file"),
        Some("samples/dropper.bin"),
        &me,
        Some("s1"),
    )
    .unwrap();
    assert_eq!(child.path, "samples/out/unpacked.bin");
    assert_eq!(child.parent.as_deref(), Some("samples/dropper.bin"));
    assert_eq!(artifact::list(&case).unwrap().len(), 2);

    // Registering what is already registered writes nothing.
    let n = events(&case);
    let again = artifact::register(&case, "samples/dropper.bin", None, None, &me, None).unwrap();
    assert_eq!(again, a);
    assert_eq!(events(&case), n);
}

#[test]
fn a_changed_or_missing_file_is_reported_and_reregistering_accepts_it() {
    let (dir, case) = case();
    let me = Author::analyst();
    let before = artifact::register(&case, "samples/dropper.bin", None, None, &me, None).unwrap();
    artifact::register(&case, "samples/out/unpacked.bin", None, None, &me, None).unwrap();
    let states = |case: &Case| -> Vec<State> {
        artifact::verify(case)
            .unwrap()
            .into_iter()
            .map(|(_, s)| s)
            .collect()
    };
    assert_eq!(states(&case), [State::Unchanged, State::Unchanged]);

    std::fs::write(dir.path().join("samples/dropper.bin"), b"patched").unwrap();
    std::fs::remove_file(dir.path().join("samples/out/unpacked.bin")).unwrap();
    assert_eq!(states(&case), [State::Changed, State::Missing]);

    let after = artifact::register(&case, "samples/dropper.bin", None, None, &me, None).unwrap();
    assert_eq!(after.id, before.id, "the same entry");
    assert_ne!(after.sha256, before.sha256);
    assert_eq!(states(&case)[0], State::Unchanged);
}

#[test]
fn a_case_refers_only_to_files_inside_it() {
    let (dir, case) = case();
    let me = Author::analyst();
    let outside = tempfile::NamedTempFile::new().unwrap();
    let refuse = |path: &str| {
        let e = artifact::register(&case, path, None, None, &me, None).unwrap_err();
        assert!(matches!(e, Error::Invalid(_)), "{path}: {e}");
    };
    refuse(outside.path().to_str().unwrap());
    // Out through `..`, to a file that does exist next to the case's folder.
    let sibling = dir
        .path()
        .parent()
        .unwrap()
        .join("reactor-test-sibling.txt");
    std::fs::write(&sibling, "x").unwrap();
    refuse("samples/../../reactor-test-sibling.txt");
    std::fs::remove_file(&sibling).unwrap();
    refuse(".reactor/analysis.db");
    refuse("samples");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        refuse("link");
    }
    assert!(artifact::register(&case, "samples/nope.bin", None, None, &me, None).is_err());
    assert!(
        artifact::register(
            &case,
            "samples/out/unpacked.bin",
            None,
            Some("samples/dropper.bin"),
            &me,
            None
        )
        .unwrap_err()
        .to_string()
        .contains("not registered yet")
    );
    assert!(artifact::list(&case).unwrap().is_empty());
    let _ = dir;
}
