//! Filter views kept by name (`graph::views`).

use reactor_db::Case;
use reactor_db::graph::views::{self, NAME_MAX, SavedView, ViewLayer};

fn case() -> (tempfile::TempDir, Case) {
    let dir = tempfile::tempdir().unwrap();
    let case = Case::create(dir.path(), None).unwrap();
    (dir, case)
}

fn view(name: &str, filter: &str) -> SavedView {
    SavedView {
        name: name.into(),
        layers: vec![
            ViewLayer {
                filter: filter.into(),
                mode: "only".into(),
                on: true,
            },
            ViewLayer {
                filter: "type:process.".into(),
                mode: "dim".into(),
                on: false,
            },
        ],
        size: Some("l".into()),
        detail: None,
    }
}

#[test]
fn a_view_is_kept_by_name_replaced_by_the_same_name_and_forgotten() {
    let (_d, case) = case();
    let db = case.db();
    assert!(views::list(db).unwrap().is_empty());
    views::save(db, &view("C2 only", "tag:phase.c2")).unwrap();
    views::save(db, &view("agent guesses", "by:agent conf:<likely")).unwrap();
    let all = views::list(db).unwrap();
    assert_eq!(
        all.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
        ["agent guesses", "C2 only"],
        "by name, case aside"
    );
    // Exactly what was saved comes back: layers in order, off ones too.
    assert_eq!(all[1], view("C2 only", "tag:phase.c2"));
    // The same name, in another case, is the same view: it is replaced.
    views::save(db, &view("c2 only", "tag:phase.exfil")).unwrap();
    let all = views::list(db).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].name, "c2 only");
    assert_eq!(all[1].layers[0].filter, "tag:phase.exfil");
    views::delete(db, "AGENT GUESSES").unwrap();
    views::delete(db, "never was").unwrap();
    assert_eq!(views::list(db).unwrap().len(), 1);
}

#[test]
fn a_view_needs_a_name_and_only_things_that_exist() {
    let (_d, case) = case();
    let db = case.db();
    assert!(views::save(db, &view("   ", "x")).is_err());
    assert!(views::save(db, &view(&"n".repeat(NAME_MAX + 1), "x")).is_err());
    assert!(views::save(db, &view(&"n".repeat(NAME_MAX), "x")).is_ok());
    let mut odd = view("odd", "x");
    odd.size = Some("huge".into());
    assert!(views::save(db, &odd).is_err());
    let mut odd = view("odd", "x");
    odd.detail = Some("everything".into());
    assert!(views::save(db, &odd).is_err());
    // A name is kept trimmed; a view with no layers is a size and a level and nothing else.
    let kept = views::save(
        db,
        &SavedView {
            name: "  just big  ".into(),
            layers: Vec::new(),
            size: Some("xl".into()),
            detail: Some("full".into()),
        },
    )
    .unwrap();
    assert_eq!(kept.name, "just big");
    assert!(views::list(db).unwrap().iter().any(|v| *v == kept));
}

#[test]
fn views_are_there_when_the_case_is_opened_again() {
    let dir = tempfile::tempdir().unwrap();
    {
        let case = Case::create(dir.path(), None).unwrap();
        views::save(case.db(), &view("kept", "kind:note")).unwrap();
    }
    let case = Case::open(dir.path()).unwrap();
    assert_eq!(views::list(case.db()).unwrap()[0].name, "kept");
}
