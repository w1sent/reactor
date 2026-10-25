//! What the benchmarks stand on: a synthetic case of any size, and a small timing harness.
//!
//! Compiled only with the `bench` feature (the benches and the GUI's dev-dependencies turn it
//! on), so nothing here ships. The case is made through [`graph::apply`], the one way into the
//! graph, so building it also measures the write path.
//!
//! The shape is that of an incident rather than random noise: entities fall into clusters,
//! most claims relate two entities of the same cluster and a few cross between clusters, a few
//! claims have three participants, some are discussed by a note, some entities carry tags and
//! properties, and a few have nothing said about them at all. The same size gives the same
//! graph, every time.

use std::fmt::Write as _;
use std::io::Write as _;
use std::time::{Duration, Instant};

use crate::Db;
use crate::graph::{self, Batch, Content, Kind, Link, Put, Target};
use crate::manifest::Author;

/// A small deterministic generator: reproducible without a dependency.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(2).wrapping_add(1))
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 11
    }

    /// A number below `n` (`n` > 0).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// True about `percent` times in a hundred.
    pub fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

/// How big a synthetic case is, and how it is put together.
#[derive(Debug, Clone)]
pub struct Shape {
    pub entities: usize,
    /// Binary and three-way claims, for each entity.
    pub claims_per_entity: f32,
    /// Entities that belong together.
    pub cluster: usize,
    /// Of every hundred claim ends, how many land outside the claim's own cluster.
    pub cross_percent: usize,
    /// Of every hundred claims, how many a note is written about (so the claim stays a node).
    pub discussed_percent: usize,
    /// Of every hundred claims, how many have three participants.
    pub three_way_percent: usize,
    /// Notes written about entities, for each hundred entities.
    pub notes_per_hundred: usize,
}

impl Shape {
    /// An incident with this many entities.
    pub fn of(entities: usize) -> Shape {
        Shape {
            entities,
            claims_per_entity: 1.5,
            cluster: 40,
            cross_percent: 8,
            discussed_percent: 3,
            three_way_percent: 3,
            notes_per_hundred: 8,
        }
    }
}

const TAGS: [&str; 10] = [
    "phase.recon",
    "phase.c2",
    "phase.exfil",
    "host.internal",
    "host.dmz",
    "actor.apt",
    "ioc",
    "triage",
    "reviewed",
    "noise",
];

const ENTITY_TYPES: [&str; 6] = [
    "host.name",
    "net.ipv4",
    "account.user",
    "process.name",
    "file.path",
    "net.domain",
];

/// What was made: how many of each, and the ids of the entities.
#[derive(Debug, Clone, Default)]
pub struct Made {
    pub entities: Vec<String>,
    pub claims: usize,
    pub notes: usize,
    pub write: Duration,
}

/// Fill the case's graph with a synthetic incident of this shape.
pub fn populate(db: &Db, shape: &Shape) -> crate::Result<Made> {
    let started = Instant::now();
    let mut rng = Rng::new(shape.entities as u64 + 7);
    let author = || Author::analyst();
    let mut made = Made::default();

    // Entities, a thousand to a transaction.
    for chunk in (0..shape.entities).collect::<Vec<_>>().chunks(1000) {
        let mut batch = Batch::new(author());
        for &i in chunk {
            let kind = ENTITY_TYPES[i % ENTITY_TYPES.len()];
            let mut put = Put::entity(kind, &format!("{kind}-{i:07}"));
            if rng.chance(30) {
                put.content
                    .tags
                    .push(TAGS[rng.below(TAGS.len())].to_string());
            }
            if rng.chance(8) {
                put.content
                    .tags
                    .push(TAGS[rng.below(TAGS.len())].to_string());
            }
            if rng.chance(15) {
                put.content
                    .props
                    .insert("size_bytes".into(), (rng.below(1 << 20) as u64).into());
            }
            if rng.chance(10) {
                put.content.confidence = Some("likely".into());
            }
            batch = batch.put(put);
        }
        made.entities.extend(graph::apply(db, batch)?.ids);
    }

    let n = made.entities.len();
    if n < 2 {
        made.write = started.elapsed();
        return Ok(made);
    }
    let cluster = shape.cluster.max(2);
    // An entity of the same cluster as `of`, or (now and then) of any.
    let near = |rng: &mut Rng, of: usize| -> usize {
        if rng.chance(shape.cross_percent) {
            return rng.below(n);
        }
        let base = of / cluster * cluster;
        (base + rng.below(cluster)).min(n - 1)
    };
    let sources = 12;
    let claims = (shape.entities as f32 * shape.claims_per_entity) as usize;
    // Leave a few entities with nothing said about them.
    let used = (n * 97 / 100).max(2);
    let mut claim_ids: Vec<String> = Vec::new();
    let mut c = 0;
    while c < claims {
        let mut batch = Batch::new(author());
        let mut kinds = Vec::new();
        for _ in 0..500.min(claims - c) {
            let a = rng.below(used);
            let b = loop {
                let b = near(&mut rng, a).min(used - 1);
                if b != a {
                    break b;
                }
            };
            let src = Link::source(&format!("artifact:log-{}.csv", rng.below(sources)))
                .at(&format!("row={}", c), None);
            let to = |i: usize| Target::Id(made.entities[i].clone());
            let (kind, type_, links) = if rng.chance(shape.three_way_percent) {
                let d = near(&mut rng, a).min(used - 1);
                (
                    Kind::Event,
                    "process.start",
                    vec![
                        Link::new("parent", to(a)),
                        Link::new("child", to(b)),
                        Link::new("actor", to(d)),
                        src,
                    ],
                )
            } else {
                match rng.below(4) {
                    0 => (
                        Kind::Event,
                        "net.connection",
                        vec![Link::new("src", to(a)), Link::new("dst", to(b)), src],
                    ),
                    1 => (
                        Kind::Event,
                        "auth.logon",
                        vec![Link::new("actor", to(a)), Link::new("target", to(b)), src],
                    ),
                    2 => (
                        Kind::Observation,
                        "net.resolution",
                        vec![Link::new("query", to(a)), Link::new("answer", to(b)), src],
                    ),
                    _ => (
                        Kind::Observation,
                        "file.access",
                        vec![Link::new("actor", to(a)), Link::new("target", to(b)), src],
                    ),
                }
            };
            kinds.push(kind);
            batch = batch.put(Put {
                links,
                ..Put::new(kind, type_)
            });
            c += 1;
        }
        let t = Instant::now();
        claim_ids.extend(graph::apply(db, batch)?.ids);
        if std::env::var_os("REACTOR_BENCH_TRACE").is_some() {
            eprintln!(
                "claims batch ending at {c}: {:.1} ms",
                t.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
    made.claims = claim_ids.len();

    // Notes: about entities, and about claims (which makes those claims nodes).
    let discussed = claim_ids.len() * shape.discussed_percent / 100;
    let about_entities = shape.entities * shape.notes_per_hundred / 100;
    let mut batch = Batch::new(Author::agent("bench"));
    let mut in_batch = 0;
    for k in 0..(discussed + about_entities) {
        let target = if k < discussed {
            claim_ids[rng.below(claim_ids.len())].clone()
        } else {
            made.entities[rng.below(used)].clone()
        };
        let mut text = String::new();
        let _ = write!(
            text,
            "Observation {k}: the activity here fits the pattern seen elsewhere in the case."
        );
        if rng.chance(30) {
            text.push_str(" Check the surrounding events before drawing a conclusion.\n\n- first point\n- second point");
        }
        let hypothesis = rng.chance(15);
        let mut put = Put {
            content: Content::value(text),
            links: vec![Link::new("about", Target::Id(target))],
            ..Put::new(
                Kind::Annotation,
                if hypothesis {
                    "analysis.hypothesis"
                } else {
                    "analysis.note"
                },
            )
        };
        if rng.chance(10) {
            put.content
                .tags
                .push(TAGS[rng.below(TAGS.len())].to_string());
        }
        batch = batch.put(put);
        made.notes += 1;
        in_batch += 1;
        if in_batch == 500 {
            graph::apply(
                db,
                std::mem::replace(&mut batch, Batch::new(Author::agent("bench"))),
            )?;
            in_batch = 0;
        }
    }
    if in_batch > 0 {
        graph::apply(db, batch)?;
    }
    made.write = started.elapsed();
    Ok(made)
}

// -- timing ------------------------------------------------------------------------------------

/// One measurement.
#[derive(Debug, Clone)]
pub struct Row {
    pub suite: String,
    pub name: String,
    pub size: usize,
    pub samples: usize,
    pub median_ms: f64,
    pub min_ms: f64,
    pub p95_ms: f64,
}

/// A suite of timings: prints a line per measurement, and appends them as tab-separated rows
/// to the file named by `REACTOR_BENCH_OUT`, for comparing one run with another
/// (`scripts/bench-compare.sh`).
///
/// * `REACTOR_BENCH_SIZES` — sizes to run, e.g. `200,1000,5000` (each suite has its own default).
/// * `REACTOR_BENCH_BUDGET_MS` — how long to keep sampling one measurement (default 1500). A
///   measurement slower than that is taken once, after one warm-up.
/// * an argument that is not a flag — only the measurements whose name contains it.
pub struct Bench {
    suite: String,
    filter: Option<String>,
    budget: Duration,
    out: Option<std::fs::File>,
    pub rows: Vec<Row>,
}

impl Bench {
    pub fn new(suite: &str) -> Bench {
        let filter = std::env::args().skip(1).find(|a| !a.starts_with('-'));
        let budget = std::env::var("REACTOR_BENCH_BUDGET_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1500);
        let out = std::env::var("REACTOR_BENCH_OUT").ok().and_then(|p| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
        });
        Bench {
            suite: suite.to_string(),
            filter,
            budget: Duration::from_millis(budget),
            out,
            rows: Vec::new(),
        }
    }

    /// Run every measurement, whatever arguments the process was given (a test binary is
    /// handed its own filter).
    pub fn all(mut self) -> Bench {
        self.filter = None;
        self
    }

    /// The sizes to run: those asked for, else `default`.
    pub fn sizes(default: &[usize]) -> Vec<usize> {
        match std::env::var("REACTOR_BENCH_SIZES") {
            Ok(v) => v.split(',').filter_map(|s| s.trim().parse().ok()).collect(),
            Err(_) => default.to_vec(),
        }
    }

    pub fn wants(&self, name: &str) -> bool {
        self.filter.as_deref().is_none_or(|f| name.contains(f))
    }

    /// Time `f`: once to warm up, then as many times as the budget allows (3 to 40).
    pub fn run<T>(&mut self, name: &str, size: usize, mut f: impl FnMut() -> T) {
        if !self.wants(name) {
            return;
        }
        std::hint::black_box(f());
        let mut ms: Vec<f64> = Vec::new();
        let began = Instant::now();
        loop {
            let t = Instant::now();
            std::hint::black_box(f());
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
            // A slow measurement is taken once; a fast one as often as the budget allows.
            let slow = ms[0] > self.budget.as_secs_f64() * 1000.0;
            if ms.len() >= 40 || slow || (ms.len() >= 3 && began.elapsed() >= self.budget) {
                break;
            }
        }
        ms.sort_by(f64::total_cmp);
        let at = |p: f64| ms[((ms.len() as f64 - 1.0) * p).round() as usize];
        self.record(Row {
            suite: self.suite.clone(),
            name: name.to_string(),
            size,
            samples: ms.len(),
            median_ms: at(0.5),
            min_ms: ms[0],
            p95_ms: at(0.95),
        });
    }

    /// Record a measurement taken some other way (a one-off, like building the case).
    pub fn record(&mut self, row: Row) {
        println!(
            "{:<10} {:<40} n={:<7} {:>10.3} ms median  {:>10.3} min  {:>10.3} p95  ({} samples)",
            row.suite, row.name, row.size, row.median_ms, row.min_ms, row.p95_ms, row.samples
        );
        if let Some(out) = &mut self.out {
            let _ = writeln!(
                out,
                "{}\t{}\t{}\t{:.4}\t{:.4}",
                row.suite, row.name, row.size, row.median_ms, row.min_ms
            );
        }
        self.rows.push(row);
    }

    /// A single timing, as a row.
    pub fn once(&mut self, name: &str, size: usize, took: Duration) {
        let ms = took.as_secs_f64() * 1000.0;
        if self.wants(name) {
            self.record(Row {
                suite: self.suite.clone(),
                name: name.to_string(),
                size,
                samples: 1,
                median_ms: ms,
                min_ms: ms,
                p95_ms: ms,
            });
        }
    }
}
