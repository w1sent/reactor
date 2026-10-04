//! Measuring how well the documents are searched
//! ([ADR-0046](../../../docs/adr/0046-the-analysis-database-is-sqlite-inside-the-case.md)).
//!
//! Whether a search finds what a person means depends on the model and on the documents, and
//! no benchmark says how it will do on *these* ones. So the owner writes a few questions with
//! where each answer is, and this runs them in each way a search can be made — by keyword, by
//! meaning, both — and says how often the answer was first, how often it was among the top
//! few, and how far down it was on average. For meaning it also reads the similarities of the
//! answers that were right and of those that were not, and suggests a threshold that keeps
//! the first and drops the second.
//!
//! The scoring is plain functions of ranks and numbers, tested on their own.

use std::collections::BTreeMap;

use reactor_db::Case;
use reactor_db::corpus::Hit;
use serde::{Deserialize, Serialize};

use crate::embed::{self, Embedder, Mode};
use crate::error::Result;

/// Where an answer is. Everything given must hold, ignoring case: a part of the source's name,
/// of the section's heading path, of the passage's text. A bare string is a part of the source.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(from = "ExpectRepr")]
pub struct Expect {
    pub source: Option<String>,
    pub section: Option<String>,
    pub text: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ExpectRepr {
    Source(String),
    Parts {
        source: Option<String>,
        section: Option<String>,
        text: Option<String>,
    },
}

impl From<ExpectRepr> for Expect {
    fn from(r: ExpectRepr) -> Expect {
        match r {
            ExpectRepr::Source(s) => Expect {
                source: Some(s),
                ..Default::default()
            },
            ExpectRepr::Parts {
                source,
                section,
                text,
            } => Expect {
                source,
                section,
                text,
            },
        }
    }
}

impl Expect {
    pub fn matches(&self, hit: &Hit) -> bool {
        let has = |haystack: &str, needle: &Option<String>| {
            needle
                .as_ref()
                .is_none_or(|n| haystack.to_lowercase().contains(&n.to_lowercase()))
        };
        // Something with nothing to match on matches nothing, rather than everything.
        (self.source.is_some() || self.section.is_some() || self.text.is_some())
            && has(&hit.source_uri, &self.source)
            && has(&hit.heading_path, &self.section)
            && has(&hit.text, &self.text)
    }
}

/// One question and where its answer is: any of these counts.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Question {
    pub query: String,
    pub expect: Vec<Expect>,
}

/// How a way of searching did over all the questions.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModeScore {
    pub mode: String,
    pub questions: usize,
    /// Questions whose answer was the first result.
    pub first: usize,
    /// Questions whose answer was among the first `k`.
    pub within_k: usize,
    /// The mean of 1/rank of the first answer, 0 for a miss: 1 is always first.
    pub mrr: f64,
    /// Per question, in order: where the first answer was (1 is first), if it was found.
    pub ranks: Vec<Option<usize>>,
}

/// Score a list of ranks: `None` is a miss.
pub fn score(mode: &str, ranks: Vec<Option<usize>>, k: usize) -> ModeScore {
    let n = ranks.len();
    let first = ranks.iter().filter(|r| **r == Some(1)).count();
    let within_k = ranks.iter().filter(|r| r.is_some_and(|r| r <= k)).count();
    let mrr = if n == 0 {
        0.0
    } else {
        ranks
            .iter()
            .map(|r| r.map_or(0.0, |r| 1.0 / r as f64))
            .sum::<f64>()
            / n as f64
    };
    ModeScore {
        mode: mode.to_string(),
        questions: n,
        first,
        within_k,
        mrr,
        ranks,
    }
}

/// What the similarities of right and wrong answers say about a threshold.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Calibration {
    pub right: usize,
    pub wrong: usize,
    /// The lowest similarity among the right answers, and the highest among the wrong.
    pub lowest_right: f32,
    pub highest_wrong: f32,
    /// The threshold that gives the best balance of keeping right answers and dropping wrong.
    pub best: f32,
    /// The highest threshold that still keeps nine in ten of the right answers.
    pub keeps_nine_in_ten: f32,
}

/// Read a threshold off the similarities of the results that were right and of those that
/// were not. `None` without both kinds: there is nothing to tell them apart by.
pub fn calibrate(right: &[f32], wrong: &[f32]) -> Option<Calibration> {
    if right.is_empty() || wrong.is_empty() {
        return None;
    }
    let mut candidates: Vec<f32> = right.iter().chain(wrong).copied().collect();
    candidates.sort_by(f32::total_cmp);
    candidates.dedup();
    let mut best = (f64::MIN, candidates[0]);
    let mut nine = None;
    for &t in &candidates {
        let kept_right = right.iter().filter(|s| **s >= t).count() as f64;
        let kept_wrong = wrong.iter().filter(|s| **s >= t).count() as f64;
        let precision = if kept_right + kept_wrong == 0.0 {
            0.0
        } else {
            kept_right / (kept_right + kept_wrong)
        };
        let recall = kept_right / right.len() as f64;
        let f1 = if precision + recall == 0.0 {
            0.0
        } else {
            2.0 * precision * recall / (precision + recall)
        };
        // The first best wins a tie: the lower threshold, which loses less.
        if f1 > best.0 {
            best = (f1, t);
        }
        if recall >= 0.9 {
            nine = Some(t);
        }
    }
    let down = |v: f32| (v * 100.0).floor() / 100.0;
    Some(Calibration {
        right: right.len(),
        wrong: wrong.len(),
        lowest_right: right.iter().copied().fold(f32::MAX, f32::min),
        highest_wrong: wrong.iter().copied().fold(f32::MIN, f32::max),
        best: down(best.1),
        keeps_nine_in_ten: down(nine.unwrap_or(candidates[0])),
    })
}

/// The whole result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub k: usize,
    pub modes: Vec<ModeScore>,
    pub calibration: Option<Calibration>,
    /// Why meaning was not tried, if it was not.
    pub meaning_skipped: Option<String>,
}

/// Run the questions in each way the case can be searched now and score them. Meaning is tried
/// where the vectors are complete for the chosen model, and skipped, with the reason, where
/// they are not.
pub async fn run(
    case: &Case,
    embedder: Option<&dyn Embedder>,
    questions: &[Question],
    k: usize,
) -> Result<Report> {
    let mut modes = Vec::new();
    let mut calibration = None;
    let mut skipped = None;
    let ways: [(&str, Mode); 3] = [
        ("keyword", Mode::Keyword),
        ("meaning", Mode::Meaning),
        ("both", Mode::Both),
    ];
    for (name, mode) in ways {
        if mode != Mode::Keyword && !embed::meaning_ready(case, embedder) {
            skipped.get_or_insert_with(|| {
                "meaning was not tried: no embedding model is chosen, or it has not made vectors for every passage".to_string()
            });
            continue;
        }
        let mut ranks = Vec::with_capacity(questions.len());
        let (mut right, mut wrong) = (Vec::new(), Vec::new());
        for q in questions {
            // No threshold here: the similarities are what is being read.
            let found = embed::search_by(case, embedder, &q.query, k, mode, 0.0).await?;
            let rank = found
                .hits
                .iter()
                .position(|h| q.expect.iter().any(|e| e.matches(h)))
                .map(|i| i + 1);
            ranks.push(rank);
            if mode == Mode::Meaning {
                for h in &found.hits {
                    if let Some(sim) = h.similarity {
                        if q.expect.iter().any(|e| e.matches(h)) {
                            right.push(sim);
                        } else {
                            wrong.push(sim);
                        }
                    }
                }
            }
        }
        if mode == Mode::Meaning {
            calibration = calibrate(&right, &wrong);
        }
        modes.push(score(name, ranks, k));
    }
    Ok(Report {
        k,
        modes,
        calibration,
        meaning_skipped: skipped,
    })
}

/// The report as text a person reads.
pub fn render(report: &Report, questions: &[Question]) -> String {
    let mut out = Vec::new();
    for m in &report.modes {
        out.push(format!(
            "{:<8} first {}/{}  top {} {}/{}  mean reciprocal rank {:.3}",
            m.mode, m.first, m.questions, report.k, m.within_k, m.questions, m.mrr
        ));
    }
    if let Some(why) = &report.meaning_skipped {
        out.push(why.clone());
    }
    if let Some(c) = &report.calibration {
        out.push(format!(
            "meaning similarity: right answers from {:.2} up (lowest), wrong ones up to {:.2} (highest), over {} right and {} wrong results",
            c.lowest_right, c.highest_wrong, c.right, c.wrong
        ));
        out.push(format!(
            "  threshold with the best balance: {:.2}; the highest that keeps nine in ten right answers: {:.2}",
            c.best, c.keeps_nine_in_ten
        ));
    }
    // The questions that were missed, by the way that did best on them.
    let mut missed: BTreeMap<usize, Vec<&str>> = BTreeMap::new();
    for m in &report.modes {
        for (i, r) in m.ranks.iter().enumerate() {
            if r.is_none() {
                missed.entry(i).or_default().push(m.mode.as_str());
            }
        }
    }
    for (i, modes) in missed {
        out.push(format!(
            "not found ({}): {}",
            modes.join(", "),
            questions[i].query
        ));
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_are_scored_as_first_within_k_and_mean_reciprocal_rank() {
        let s = score("x", vec![Some(1), Some(2), None, Some(4)], 3);
        assert_eq!((s.first, s.within_k, s.questions), (1, 2, 4));
        assert!((s.mrr - (1.0 + 0.5 + 0.0 + 0.25) / 4.0).abs() < 1e-9);
        let none = score("x", Vec::new(), 3);
        assert_eq!((none.first, none.mrr), (0, 0.0));
    }

    #[test]
    fn a_threshold_is_read_off_where_the_right_answers_end_and_the_wrong_begin() {
        // Right answers 0.60–0.80; wrong ones mostly below 0.50, one at 0.62.
        let right = [0.80, 0.75, 0.70, 0.65, 0.60];
        let wrong = [0.50, 0.45, 0.40, 0.35, 0.62];
        let c = calibrate(&right, &wrong).unwrap();
        assert_eq!((c.right, c.wrong), (5, 5));
        assert_eq!((c.lowest_right, c.highest_wrong), (0.60, 0.62));
        // Keeping everything over 0.60 drops four of five wrong ones and loses no right one.
        assert_eq!(c.best, 0.60);
        assert!(c.keeps_nine_in_ten >= 0.60 && c.keeps_nine_in_ten <= 0.65);
        // Without both kinds there is nothing to tell them apart by.
        assert!(calibrate(&right, &[]).is_none());
        assert!(calibrate(&[], &wrong).is_none());
    }

    #[test]
    fn an_expectation_is_parts_that_must_all_hold_and_a_bare_string_is_a_source() {
        let q: Vec<Question> = serde_json::from_str(
            r#"[
              {"query": "q1", "expect": ["adr/0012"]},
              {"query": "q2", "expect": [{"source": "wiki", "section": "Persistence"}, "other.md"]}
            ]"#,
        )
        .unwrap();
        assert_eq!(q[0].expect[0].source.as_deref(), Some("adr/0012"));
        assert_eq!(q[1].expect.len(), 2);
        assert_eq!(q[1].expect[0].section.as_deref(), Some("Persistence"));
        let hit = |source: &str, section: &str, text: &str| Hit {
            chunk_id: 1,
            document_id: String::new(),
            source_uri: source.into(),
            title: None,
            heading_path: section.into(),
            text: text.into(),
            origin: "external".into(),
            source_kind: "file".into(),
            mime: None,
            meta: serde_json::Value::Null,
            score: 0.0,
            keyword_rank: None,
            meaning_rank: None,
            similarity: None,
        };
        assert!(q[0].expect[0].matches(&hit("docs/ADR/0012-x.md", "", "")));
        assert!(q[1].expect[0].matches(&hit("wiki/emotet.md", "Emotet > persistence", "")));
        assert!(!q[1].expect[0].matches(&hit("wiki/emotet.md", "Emotet > Network", "")));
        assert!(
            !Expect::default().matches(&hit("a", "b", "c")),
            "nothing to match on matches nothing"
        );
    }
}
