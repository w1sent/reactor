//! The context corpus: storing documents, finding passages, and keeping vectors honest.

use reactor_db::corpus::{self, Added, NewDocument, Query};
use reactor_db::{Case, Error};
use serde_json::json;

fn case() -> (tempfile::TempDir, Case) {
    let dir = tempfile::tempdir().unwrap();
    let case = Case::create(dir.path(), None).unwrap();
    (dir, case)
}

fn doc(uri: &str, raw: &str, chunks: &[(&str, &str)]) -> NewDocument {
    NewDocument {
        source_uri: uri.into(),
        source_kind: "file".into(),
        mime: Some("text/markdown".into()),
        title: Some(uri.into()),
        origin: corpus::ORIGIN_EXTERNAL.into(),
        fetched_at: None,
        meta: json!({}),
        raw: raw.as_bytes().to_vec(),
        markdown: raw.to_string(),
        chunker: "test/v1".into(),
        chunks: chunks
            .iter()
            .map(|(h, t)| (h.to_string(), t.to_string()))
            .collect(),
    }
}

fn keywords(case: &Case, text: &str) -> Vec<corpus::Hit> {
    corpus::search(
        case.db(),
        &Query {
            text,
            vector: None,
            limit: 10,
        },
    )
    .unwrap()
}

fn table_rows(case: &Case, table: &str) -> i64 {
    case.db()
        .read(|c| Ok(c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?))
        .unwrap()
}

#[test]
fn a_document_is_stored_whole_and_as_passages_and_the_same_one_again_changes_nothing() {
    let (_d, case) = case();
    let d = doc(
        "notes/emotet.md",
        "# Emotet\n\nbody",
        &[
            ("Emotet", "Emotet is a banking trojan."),
            ("Emotet > Delivery", "Delivered by spam."),
        ],
    );
    let Added::Stored {
        id,
        chunks,
        replaced,
    } = corpus::add_document(&case, d.clone()).unwrap()
    else {
        panic!("not stored")
    };
    assert_eq!((chunks, replaced), (2, 0));
    assert_eq!(corpus::counts(case.db()).unwrap(), (1, 2));
    // The bytes and the Markdown are blobs, so the case holds what it was built on.
    let info = &corpus::documents(case.db()).unwrap()[0];
    assert_eq!(
        (info.id.as_str(), info.chunks, info.origin.as_str()),
        (id.as_str(), 2, "external")
    );
    assert_eq!(case.blobs().fsck().unwrap(), Default::default());

    let events = table_rows(&case, "event");
    assert_eq!(
        corpus::add_document(&case, d).unwrap(),
        Added::Unchanged { id }
    );
    assert_eq!(table_rows(&case, "event"), events, "nothing was written");
    assert_eq!(corpus::counts(case.db()).unwrap(), (1, 2));

    assert!(matches!(
        corpus::add_document(&case, doc("x.md", "x", &[])),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn a_changed_source_replaces_what_was_there_down_to_the_index() {
    let (_d, case) = case();
    corpus::add_document(
        &case,
        doc("a.md", "v1", &[("", "old unique zebrafish text")]),
    )
    .unwrap();
    assert_eq!(keywords(&case, "zebrafish").len(), 1);
    let again =
        corpus::add_document(&case, doc("a.md", "v2", &[("", "new unique narwhal text")])).unwrap();
    assert!(matches!(again, Added::Stored { replaced: 1, .. }));
    assert_eq!(corpus::counts(case.db()).unwrap(), (1, 1));
    assert!(
        keywords(&case, "zebrafish").is_empty(),
        "the old passage left the keyword index"
    );
    assert_eq!(keywords(&case, "narwhal").len(), 1);
    // Another source with the same content is its own document.
    corpus::add_document(&case, doc("b.md", "v2", &[("", "new unique narwhal text")])).unwrap();
    assert_eq!(corpus::counts(case.db()).unwrap(), (2, 2));
}

#[test]
fn identifiers_are_found_by_keyword_and_a_question_by_its_words() {
    let (_d, case) = case();
    corpus::add_document(
        &case,
        doc(
            "re.md",
            "x",
            &[
                (
                    "Config",
                    "The function decrypt_config at 0x401000 unpacks the C2 list.",
                ),
                ("Network", "Beacons go to c2.example.net every 47 seconds."),
                ("Other", "Unrelated text about gardening and compost."),
            ],
        ),
    )
    .unwrap();
    // An underscore and a hex address are not syntax.
    assert_eq!(keywords(&case, "decrypt_config")[0].heading_path, "Config");
    assert_eq!(keywords(&case, "0x401000")[0].heading_path, "Config");
    assert_eq!(keywords(&case, "c2.example.net")[0].heading_path, "Network");
    // A sentence is ranked by how many of its words a passage has.
    let hits = keywords(
        &case,
        "how often does it beacon to the c2 domain every seconds?",
    );
    assert_eq!(hits[0].heading_path, "Network");
    // Nothing in a query is read as FTS5 syntax.
    for q in ["AND OR NOT", "\"unbalanced", "a:b NEAR(c)", "*", "(", "--"] {
        let _ = keywords(&case, q);
    }
    assert!(keywords(&case, "   ").is_empty());
    assert!(keywords(&case, "nonexistentword").is_empty());
    let h = &keywords(&case, "gardening")[0];
    assert_eq!(
        (h.source_uri.as_str(), h.origin.as_str(), h.keyword_rank),
        ("re.md", "external", Some(1))
    );
    assert_eq!(h.meaning_rank, None);
}

#[test]
fn vectors_belong_to_a_model_and_a_passage_without_one_is_pending() {
    let (_d, case) = case();
    let db = case.db();
    corpus::add_document(
        &case,
        doc("a.md", "x", &[("", "alpha"), ("", "beta"), ("", "gamma")]),
    )
    .unwrap();
    let m = corpus::register_model(db, "ollama/test", "", 4).unwrap();
    assert_eq!(
        corpus::register_model(db, "ollama/test", "", 4).unwrap(),
        m,
        "registering again is finding"
    );
    let e = corpus::register_model(db, "ollama/test", "", 8).unwrap_err();
    assert!(e.to_string().contains("another revision"), "{e}");
    assert!(corpus::register_model(db, "x", "", 0).is_err());

    let pending = corpus::pending(db, m.id, 10).unwrap();
    assert_eq!(
        pending.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(),
        ["alpha", "beta", "gamma"]
    );
    assert_eq!(corpus::progress(db, m.id).unwrap().embedded, 0);
    assert!(
        corpus::usable_model(db, None).unwrap().is_none(),
        "nothing embedded yet: keywords only"
    );

    // A wrong dimension is refused whole.
    assert!(corpus::store_vectors(db, &m, vec![(pending[0].0, vec![1.0; 3])]).is_err());
    assert_eq!(corpus::progress(db, m.id).unwrap().embedded, 0);

    corpus::store_vectors(
        db,
        &m,
        vec![
            (pending[0].0, vec![1., 0., 0., 0.]),
            (pending[1].0, vec![0., 1., 0., 0.]),
        ],
    )
    .unwrap();
    assert_eq!(corpus::pending(db, m.id, 10).unwrap().len(), 1);
    assert!(
        corpus::usable_model(db, None).unwrap().is_none(),
        "one passage is still missing"
    );
    // Storing again replaces, and a passage that is gone is skipped.
    let n = corpus::store_vectors(
        db,
        &m,
        vec![
            (pending[0].0, vec![0.9, 0.1, 0., 0.]),
            (99_999, vec![0.; 4]),
            (pending[2].0, vec![0., 0., 1., 0.]),
        ],
    )
    .unwrap();
    assert_eq!(n, 2);
    let p = corpus::progress(db, m.id).unwrap();
    assert!(p.complete() && p.total == 3);
    assert_eq!(corpus::usable_model(db, None).unwrap().unwrap().id, m.id);
}

#[test]
fn a_passage_is_found_by_meaning_and_the_two_rankings_are_fused() {
    let (_d, case) = case();
    let db = case.db();
    corpus::add_document(
        &case,
        doc(
            "a.md",
            "x",
            &[
                ("Persistence", "It survives reboots by writing a run key."),
                ("Network", "It talks to its server over HTTPS."),
                ("Noise", "decrypt_config is mentioned here in passing."),
            ],
        ),
    )
    .unwrap();
    let m = corpus::register_model(db, "ollama/test", "", 3).unwrap();
    let ids = corpus::pending(db, m.id, 10).unwrap();
    // Three directions: persistence, network, noise.
    corpus::store_vectors(
        db,
        &m,
        vec![
            (ids[0].0, vec![1., 0., 0.]),
            (ids[1].0, vec![0., 1., 0.]),
            (ids[2].0, vec![0., 0., 1.]),
        ],
    )
    .unwrap();

    // By meaning alone: a query vector near the first passage, with words that match none.
    let q = |text: &'static str, v: [f32; 3]| {
        corpus::search(
            db,
            &Query {
                text,
                vector: Some((&m, &v)),
                limit: 3,
            },
        )
        .unwrap()
    };
    let hits = q("qwxz vbnm", [0.95, 0.05, 0.0]);
    assert_eq!(hits[0].heading_path, "Persistence");
    assert_eq!(hits[0].keyword_rank, None);
    assert_eq!(hits[0].meaning_rank, Some(1));

    // A word the meaning does not know and a meaning the word does not know: both found,
    // and what both agree on beats what only one put first.
    let hits = q("decrypt_config", [0.0, 0.0, 1.0]);
    assert_eq!(hits[0].heading_path, "Noise");
    assert!(hits[0].keyword_rank.is_some() && hits[0].meaning_rank.is_some());
    let fused = hits[0].score;
    let only_meaning = q("zzzunknown", [0.0, 0.0, 1.0])[0].score;
    assert!(fused > only_meaning);
    // A wrong-sized query vector is refused, not silently misread.
    let bad = corpus::search(
        db,
        &Query {
            text: "x",
            vector: Some((&m, &[1.0, 0.0])),
            limit: 3,
        },
    );
    assert!(bad.is_err());
    // Without a vector it is keywords only.
    assert_eq!(
        corpus::search(
            db,
            &Query {
                text: "run key",
                vector: None,
                limit: 3
            }
        )
        .unwrap()[0]
            .heading_path,
        "Persistence"
    );
}

#[test]
fn changing_the_model_keeps_search_on_the_old_vectors_until_the_new_set_is_complete() {
    let (_d, case) = case();
    let db = case.db();
    corpus::add_document(&case, doc("a.md", "x", &[("", "one"), ("", "two")])).unwrap();
    let old = corpus::register_model(db, "ollama/old", "", 2).unwrap();
    let ids = corpus::pending(db, old.id, 10).unwrap();
    corpus::store_vectors(
        db,
        &old,
        ids.iter().map(|(i, _)| (*i, vec![1., 0.])).collect(),
    )
    .unwrap();
    assert_eq!(
        corpus::usable_model(db, Some("ollama/old"))
            .unwrap()
            .unwrap()
            .id,
        old.id
    );

    let new = corpus::register_model(db, "ollama/new", "", 3).unwrap();
    // Chosen, and not yet embedded: the old one is still what search uses.
    assert_eq!(
        corpus::usable_model(db, Some("ollama/new"))
            .unwrap()
            .unwrap()
            .id,
        old.id
    );
    corpus::store_vectors(db, &new, vec![(ids[0].0, vec![1., 0., 0.])]).unwrap();
    assert_eq!(
        corpus::usable_model(db, Some("ollama/new"))
            .unwrap()
            .unwrap()
            .id,
        old.id,
        "half is not complete"
    );
    corpus::store_vectors(db, &new, vec![(ids[1].0, vec![0., 1., 0.])]).unwrap();
    assert_eq!(
        corpus::usable_model(db, Some("ollama/new"))
            .unwrap()
            .unwrap()
            .id,
        new.id
    );
    // Asking for the old one while the new is complete and newer still works.
    assert_eq!(
        corpus::usable_model(db, Some("ollama/old"))
            .unwrap()
            .unwrap()
            .id,
        old.id
    );

    // A new document makes both incomplete: search falls back to keywords until it is embedded.
    corpus::add_document(&case, doc("b.md", "y", &[("", "three")])).unwrap();
    assert!(
        corpus::usable_model(db, Some("ollama/new"))
            .unwrap()
            .is_none()
    );

    corpus::forget_model(db, old.id).unwrap();
    assert_eq!(corpus::models(db).unwrap().len(), 1);
    assert!(corpus::progress(db, old.id).unwrap().embedded == 0);
}

#[test]
fn removing_a_document_removes_its_vectors_in_every_model_with_it() {
    let (_d, case) = case();
    let db = case.db();
    corpus::add_document(
        &case,
        doc(
            "a.md",
            "x",
            &[("", "keep this passage"), ("", "drop this passage")],
        ),
    )
    .unwrap();
    corpus::add_document(&case, doc("b.md", "y", &[("", "dropping that one too")])).unwrap();
    let docs = corpus::documents(db).unwrap();
    let b = docs
        .iter()
        .find(|d| d.source_uri == "b.md")
        .unwrap()
        .id
        .clone();
    for (name, dim) in [("ollama/a", 2usize), ("ollama/b", 3)] {
        let m = corpus::register_model(db, name, "", dim).unwrap();
        let rows: Vec<_> = corpus::pending(db, m.id, 10)
            .unwrap()
            .into_iter()
            .map(|(i, _)| (i, vec![1.0; dim]))
            .collect();
        corpus::store_vectors(db, &m, rows).unwrap();
    }
    assert_eq!(
        (
            table_rows(&case, "chunk_vec_1"),
            table_rows(&case, "chunk_vec_2")
        ),
        (3, 3)
    );

    corpus::remove_document(db, &b).unwrap();
    assert_eq!(corpus::counts(db).unwrap(), (1, 2));
    assert_eq!(
        (
            table_rows(&case, "chunk_vec_1"),
            table_rows(&case, "chunk_vec_2")
        ),
        (2, 2)
    );
    assert_eq!(table_rows(&case, "embedding_status"), 4);
    assert!(keywords(&case, "dropping").is_empty());
    assert!(corpus::remove_document(db, &b).is_err());
}

#[test]
fn the_derived_indexes_are_rebuilt_from_the_passages() {
    let (_d, case) = case();
    let db = case.db();
    corpus::add_document(&case, doc("a.md", "x", &[("H", "findable passage text")])).unwrap();
    let m = corpus::register_model(db, "ollama/test", "", 2).unwrap();
    let id = corpus::pending(db, m.id, 1).unwrap()[0].0;
    corpus::store_vectors(db, &m, vec![(id, vec![1., 0.])]).unwrap();
    // Damage the keyword index behind the corpus's back, then rebuild.
    db.write(|tx| {
        tx.execute("INSERT INTO chunk_fts(chunk_fts) VALUES ('delete-all')", [])?;
        Ok(())
    })
    .unwrap();
    assert!(keywords(&case, "findable").is_empty());
    corpus::rebuild_indexes(db).unwrap();
    assert_eq!(keywords(&case, "findable").len(), 1);
    // Vectors are dropped to be made again; the passages are untouched.
    assert_eq!(table_rows(&case, "chunk_vec_1"), 0);
    assert_eq!(corpus::progress(db, m.id).unwrap().embedded, 0);
    assert_eq!(corpus::counts(db).unwrap(), (1, 1));
}

#[test]
fn the_passages_around_a_hit_come_in_reading_order_within_its_own_document() {
    let (_d, case) = case();
    corpus::add_document(
        &case,
        doc(
            "a.md",
            "a",
            &[
                ("A", "one"),
                ("A > B", "two"),
                ("A > C", "three"),
                ("D", "four"),
            ],
        ),
    )
    .unwrap();
    corpus::add_document(&case, doc("b.md", "b", &[("X", "elsewhere two")])).unwrap();
    let hit = &keywords(&case, "three")[0];
    let around = corpus::surrounding(case.db(), hit.chunk_id, 1, 1).unwrap();
    let texts: Vec<&str> = around.iter().map(|p| p.text.as_str()).collect();
    assert_eq!(texts, ["two", "three", "four"]);
    assert_eq!(around[1].chunk_id, hit.chunk_id);
    // At the start of a document there is nothing before it, and no other document leaks in.
    let first = &keywords(&case, "one")[0];
    let around = corpus::surrounding(case.db(), first.chunk_id, 2, 0).unwrap();
    assert_eq!(around.len(), 1);
}

#[test]
fn a_passage_handed_over_as_text_is_found_again_by_its_document_and_section() {
    let (_d, case) = case();
    corpus::add_document(
        &case,
        doc(
            "a.md",
            "a",
            &[("A", "one"), ("A > B", "two"), ("A > B", "three")],
        ),
    )
    .unwrap();
    let found = corpus::hits_at(case.db(), "a.md", "A > B").unwrap();
    let texts: Vec<&str> = found.iter().map(|h| h.text.as_str()).collect();
    assert_eq!(texts, ["two", "three"]);
    assert!(found[0].keyword_rank.is_none() && found[0].chunk_id > 0);
    assert!(corpus::hits_at(case.db(), "a.md", "Z").unwrap().is_empty());
    assert!(corpus::hits_at(case.db(), "b.md", "A").unwrap().is_empty());
}

#[test]
fn a_heading_or_fence_opener_left_alone_by_the_splitter_is_stored_with_what_follows() {
    assert!(corpus::is_hollow("## Consequences"));
    assert!(corpus::is_hollow("### DDL\n\n```sql"));
    assert!(corpus::is_hollow("---"));
    assert!(corpus::is_hollow("  \n"));
    assert!(!corpus::is_hollow("## Consequences\n\nThe tests need pi."));
    assert!(!corpus::is_hollow("Emotet is a banking trojan."));
    assert!(
        !corpus::is_hollow("see issue # 5"),
        "a # inside a line is text"
    );
    assert!(!corpus::is_hollow("#hashtag"));

    let (_d, case) = case();
    corpus::add_document(
        &case,
        doc(
            "adr.md",
            "x",
            &[
                ("Intro", "Why we did it."),
                ("Consequences", "## Consequences"),
                (
                    "Consequences",
                    "- The tests need a pi.\n- The build is slower.",
                ),
                ("DDL", "### DDL"),
                ("DDL", "```sql"),
                ("DDL", "CREATE TABLE t(x);\n```"),
                ("End", "## Later"),
            ],
        ),
    )
    .unwrap();
    let texts: Vec<String> = case
        .db()
        .read(|c| {
            let mut s = c.prepare("SELECT text FROM chunk ORDER BY ord")?;
            Ok(s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?)
        })
        .unwrap();
    assert_eq!(texts.len(), 3, "{texts:?}");
    assert!(texts[1].starts_with("## Consequences\n\n- The tests need a pi."));
    assert!(texts[2].starts_with("### DDL\n\n```sql\n\nCREATE TABLE t(x);"));
    // The one at the end went to the one before it.
    assert!(texts[2].ends_with("## Later"));
    assert!(texts.iter().all(|t| !corpus::is_hollow(t)));
    // A document of nothing else is kept as it is.
    corpus::add_document(&case, doc("h.md", "h", &[("T", "# Title")])).unwrap();
}

#[test]
fn a_passage_with_no_words_is_never_a_result_even_if_it_was_stored_before_they_were_joined() {
    let (_d, case) = case();
    let db = case.db();
    corpus::add_document(
        &case,
        doc(
            "a.md",
            "x",
            &[("Real", "The loader writes a run key to persist.")],
        ),
    )
    .unwrap();
    // An old case: a heading-only passage put in directly, as the splitter used to leave it.
    db.write(|tx| {
        let id: String = tx.query_row("SELECT id FROM document", [], |r| r.get(0))?;
        tx.execute(
            "INSERT INTO chunk(document_id, ord, heading_path, text, text_sha256, chunker)
             VALUES (?1, 9, 'Consequences', '## Consequences', 'h', 'old')",
            [id],
        )?;
        Ok(())
    })
    .unwrap();
    let m = corpus::register_model(db, "ollama/test", "", 2).unwrap();
    let pending = corpus::pending(db, m.id, 10).unwrap();
    assert_eq!(pending.len(), 2);
    // The empty heading sits right on the query; the real passage is further off.
    let rows = pending
        .iter()
        .map(|(id, text)| {
            (
                *id,
                if corpus::is_hollow(text) {
                    vec![1., 0.]
                } else {
                    vec![0.6, 0.8]
                },
            )
        })
        .collect();
    corpus::store_vectors(db, &m, rows).unwrap();
    let q = [1.0f32, 0.0];
    let hits = corpus::search_ranked(
        db,
        &Query {
            text: "Consequences",
            vector: Some((&m, &q)),
            limit: 5,
        },
        false,
    )
    .unwrap();
    let texts: Vec<&str> = hits.iter().map(|h| h.text.as_str()).collect();
    assert_eq!(texts, ["The loader writes a run key to persist."]);
    // Nor by its word.
    assert!(keywords(&case, "Consequences").is_empty());
}

#[test]
fn importing_again_repairs_a_document_stored_with_empty_passages() {
    let (_d, case) = case();
    let d = doc("a.md", "x", &[("A", "Body words here.")]);
    corpus::add_document(&case, d.clone()).unwrap();
    assert!(matches!(
        corpus::add_document(&case, d.clone()).unwrap(),
        corpus::Added::Unchanged { .. }
    ));
    // As an older version stored it: an empty heading passage beside the real one.
    case.db()
        .write(|tx| {
            let id: String = tx.query_row("SELECT id FROM document", [], |r| r.get(0))?;
            tx.execute(
                "INSERT INTO chunk(document_id, ord, heading_path, text, text_sha256, chunker)
                 VALUES (?1, 5, 'B', '## B', 'h', 'old')",
                [id],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        corpus::add_document(&case, d.clone()).unwrap(),
        corpus::Added::Stored { replaced: 1, .. }
    ));
    let (docs, chunks) = corpus::counts(case.db()).unwrap();
    assert_eq!((docs, chunks), (1, 1));
    assert!(matches!(
        corpus::add_document(&case, d).unwrap(),
        corpus::Added::Unchanged { .. }
    ));
}

#[test]
fn clearing_a_models_vectors_makes_every_passage_pending_for_it_and_only_it() {
    let (_d, case) = case();
    let db = case.db();
    corpus::add_document(
        &case,
        doc(
            "a.md",
            "x",
            &[("A", "one two three four"), ("B", "five six seven")],
        ),
    )
    .unwrap();
    let (a, b) = (
        corpus::register_model(db, "ollama/a", "", 2).unwrap(),
        corpus::register_model(db, "ollama/b", "", 2).unwrap(),
    );
    for m in [&a, &b] {
        let rows = corpus::pending(db, m.id, 10)
            .unwrap()
            .into_iter()
            .map(|(id, _)| (id, vec![1., 0.]))
            .collect();
        corpus::store_vectors(db, m, rows).unwrap();
    }
    assert!(corpus::progress(db, a.id).unwrap().complete());
    corpus::clear_vectors(db, a.id).unwrap();
    assert_eq!(corpus::progress(db, a.id).unwrap().embedded, 0);
    assert_eq!(corpus::pending(db, a.id, 10).unwrap().len(), 2);
    // The model is still registered, and the other one is untouched.
    assert!(corpus::models(db).unwrap().iter().any(|m| m.id == a.id));
    assert!(corpus::progress(db, b.id).unwrap().complete());
    // It can be filled again.
    let rows = corpus::pending(db, a.id, 10)
        .unwrap()
        .into_iter()
        .map(|(id, _)| (id, vec![0., 1.]))
        .collect();
    assert_eq!(corpus::store_vectors(db, &a, rows).unwrap(), 2);
}

#[test]
fn a_hit_says_how_close_in_meaning_it_is_and_a_threshold_drops_what_is_too_far() {
    let (_d, case) = case();
    let db = case.db();
    corpus::add_document(
        &case,
        doc(
            "a.md",
            "x",
            &[
                ("A", "The loader survives reboots by a run key."),
                ("B", "It talks to its server over HTTPS."),
                ("C", "A note about something else entirely here."),
            ],
        ),
    )
    .unwrap();
    let m = corpus::register_model(db, "ollama/test", "", 2).unwrap();
    let ids = corpus::pending(db, m.id, 10).unwrap();
    // Unit vectors at 0, 60 and 90 degrees from the query: similarities 1.0, 0.5, 0.0.
    corpus::store_vectors(
        db,
        &m,
        vec![
            (ids[0].0, vec![1., 0.]),
            (ids[1].0, vec![0.5, 0.866_025_4]),
            (ids[2].0, vec![0., 1.]),
        ],
    )
    .unwrap();
    let q = [1.0f32, 0.0];
    let run = |min_similarity: f32| {
        corpus::search_with(
            db,
            &Query {
                text: "zzz",
                vector: Some((&m, &q)),
                limit: 10,
            },
            &corpus::Options {
                keywords: false,
                min_similarity,
            },
        )
        .unwrap()
    };
    let all = run(0.0);
    let sims: Vec<f32> = all.iter().map(|h| h.similarity.unwrap()).collect();
    assert_eq!(all.len(), 3);
    assert!(
        (sims[0] - 1.0).abs() < 1e-4 && (sims[1] - 0.5).abs() < 1e-4 && sims[2].abs() < 1e-4,
        "{sims:?}"
    );
    assert!(
        sims.windows(2).all(|w| w[0] >= w[1]),
        "larger is closer, and closest first"
    );
    // A threshold drops what is under it, and keeps what is at or over.
    assert_eq!(run(0.5).len(), 2);
    assert_eq!(run(0.9).len(), 1);
    assert!(run(1.01).is_empty());
    // A keyword-only search has no similarity to report.
    assert!(keywords(&case, "loader")[0].similarity.is_none());
}
