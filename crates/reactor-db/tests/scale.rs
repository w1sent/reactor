//! How search behaves at size. Not part of the normal run: it builds a synthetic corpus and
//! takes minutes.
//!
//! ```text
//! REACTOR_SCALE_CHUNKS=100000 cargo test --release -p reactor-db --test scale -- --ignored --nocapture
//! REACTOR_SCALE_CHUNKS=1000000 ...
//! ```
//!
//! The corpus is random words from a skewed vocabulary and random unit vectors of
//! `REACTOR_SCALE_DIM` dimensions (default 384, the size of the small embedding models).
//! That is a fair test of *cost* — rows scanned, bytes read — and says nothing about the
//! quality of results, which a labelled query set measures instead.

use std::time::Instant;

use reactor_db::Case;
use reactor_db::corpus::{self, Query};

/// A small deterministic generator: reproducible without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 11
    }

    fn unit(&mut self) -> f32 {
        (self.next() % 2_000_001) as f32 / 1_000_000.0 - 1.0
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() as f64 - 1.0) * p).round() as usize]
}

fn bench(label: &str, n: usize, mut f: impl FnMut(usize)) {
    let mut ms: Vec<f64> = (0..n)
        .map(|i| {
            let t = Instant::now();
            f(i);
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    ms.sort_by(f64::total_cmp);
    println!(
        "{label:<34} n={n:<3} p50 {:>8.1} ms   p95 {:>8.1} ms   max {:>8.1} ms",
        percentile(&ms, 0.5),
        percentile(&ms, 0.95),
        ms[ms.len() - 1]
    );
}

#[test]
#[ignore]
fn search_latency_at_size() {
    let chunks: usize = std::env::var("REACTOR_SCALE_CHUNKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000);
    let dim: usize = std::env::var("REACTOR_SCALE_DIM")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(384);
    let root = std::env::var("REACTOR_SCALE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("reactor-scale"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let case = Case::create(&root, None).unwrap();
    let db = case.db();

    // -- build -----------------------------------------------------------------------------
    let started = Instant::now();
    let vocabulary: Vec<String> = (0..20_000).map(|i| format!("w{i:05}x")).collect();
    let per_doc = 1_000;
    let docs = chunks.div_ceil(per_doc);
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO blob(sha256, size, created_at) VALUES (?1, 0, '2026-01-01T00:00:00Z')",
            ["0".repeat(64)],
        )?;
        let mut doc = tx.prepare(
            "INSERT INTO document(id, source_uri, source_kind, content_sha256, origin, added_at)
             VALUES (?1, ?2, 'file', ?3, 'external', '2026-01-01T00:00:00Z')",
        )?;
        for d in 0..docs {
            doc.execute(rusqlite::params![
                format!("d{d}"),
                format!("doc{d}.md"),
                "0".repeat(64)
            ])?;
        }
        Ok(())
    })
    .unwrap();
    let mut rng = Rng(42);
    let batch = 20_000;
    let mut made = 0usize;
    while made < chunks {
        let upto = (made + batch).min(chunks);
        let rows: Vec<(usize, String)> = (made..upto)
            .map(|i| {
                // Skewed: low-numbered words are common, high ones rare.
                let mut words: Vec<&str> = (0..60)
                    .map(|_| {
                        let r = (rng.next() % 1000) as f64 / 1000.0;
                        vocabulary[((r * r * r) * vocabulary.len() as f64) as usize].as_str()
                    })
                    .collect();
                let needle = format!("needle{}", i / 10_000);
                if i % 10_000 == 7 {
                    words.push(&needle);
                }
                (i, words.join(" "))
            })
            .collect();
        db.write(move |tx| {
            let mut ins = tx.prepare(
                "INSERT INTO chunk(id, document_id, ord, heading_path, text, text_sha256, chunker)
                 VALUES (?1, ?2, ?3, '', ?4, '', 'scale')",
            )?;
            for (i, text) in &rows {
                ins.execute(rusqlite::params![
                    *i as i64 + 1,
                    format!("d{}", i / 1_000),
                    (i % 1_000) as i64,
                    text
                ])?;
            }
            Ok(())
        })
        .unwrap();
        made = upto;
    }
    println!(
        "{chunks} passages stored in {:.1}s",
        started.elapsed().as_secs_f64()
    );

    let t = Instant::now();
    let model = corpus::register_model(db, "scale/test", "", dim).unwrap();
    let mut made = 0usize;
    while made < chunks {
        let upto = (made + 10_000).min(chunks);
        let vectors: Vec<(usize, Vec<u8>)> = (made..upto)
            .map(|i| {
                let v: Vec<f32> = (0..dim).map(|_| rng.unit()).collect();
                let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
                (i, v.iter().flat_map(|x| (x / norm).to_le_bytes()).collect())
            })
            .collect();
        let id = model.id;
        db.write(move |tx| {
            let mut v = tx.prepare(&format!("INSERT INTO chunk_vec_{id}(chunk_id, embedding) VALUES (?1, ?2)"))?;
            let mut s = tx.prepare(
                "INSERT INTO embedding_status(chunk_id, model_id, embedded_at) VALUES (?1, ?2, '2026-01-01T00:00:00Z')",
            )?;
            for (i, blob) in &vectors {
                v.execute(rusqlite::params![*i as i64 + 1, blob])?;
                s.execute(rusqlite::params![*i as i64 + 1, id])?;
            }
            Ok(())
        })
        .unwrap();
        made = upto;
    }
    println!(
        "{chunks} vectors of {dim} dimensions stored in {:.1}s",
        t.elapsed().as_secs_f64()
    );
    assert!(corpus::progress(db, model.id).unwrap().complete());
    let size = |name: &str| {
        std::fs::metadata(case.dir().join(name))
            .map(|m| m.len())
            .unwrap_or(0)
    };
    println!(
        "analysis.db {:.0} MiB (+ wal {:.0} MiB)",
        size("analysis.db") as f64 / 1048576.0,
        size("analysis.db-wal") as f64 / 1048576.0
    );

    // -- measure ---------------------------------------------------------------------------
    let queries = if chunks > 200_000 { 10 } else { 40 };
    let words = [
        "w00003x", "w00010x", "w00100x", "w01000x", "w05000x", "w10000x",
    ];
    let query_vectors: Vec<Vec<f32>> = (0..queries)
        .map(|_| {
            let v: Vec<f32> = (0..dim).map(|_| rng.unit()).collect();
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
            v.iter().map(|x| x / norm).collect()
        })
        .collect();
    println!("--- {chunks} passages, {dim} dimensions ---");
    bench("keyword, a rare identifier", queries, |i| {
        let q = format!("needle{}", i % (chunks / 10_000).max(1));
        let hits = corpus::search(
            db,
            &Query {
                text: &q,
                vector: None,
                limit: 8,
            },
        )
        .unwrap();
        assert!(!hits.is_empty(), "{q}");
    });
    bench("keyword, a question of 6 words", queries, |i| {
        let q = (0..6)
            .map(|k| words[(i + k) % words.len()])
            .collect::<Vec<_>>()
            .join(" ");
        let hits = corpus::search(
            db,
            &Query {
                text: &q,
                vector: None,
                limit: 8,
            },
        )
        .unwrap();
        assert_eq!(hits.len(), 8);
    });
    bench("meaning only (KNN, top 40)", queries, |i| {
        let hits = corpus::search(
            db,
            &Query {
                text: "",
                vector: Some((&model, &query_vectors[i])),
                limit: 8,
            },
        )
        .unwrap();
        assert_eq!(hits.len(), 8);
    });
    bench("hybrid (what docs_search does)", queries, |i| {
        let q = format!(
            "{} needle{}",
            words[i % words.len()],
            i % (chunks / 10_000).max(1)
        );
        let hits = corpus::search(
            db,
            &Query {
                text: &q,
                vector: Some((&model, &query_vectors[i])),
                limit: 8,
            },
        )
        .unwrap();
        assert!(!hits.is_empty());
    });
}
