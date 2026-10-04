//! Making vectors, and searching the corpus with them
//! ([ADR-0046](../../../docs/adr/0046-the-analysis-database-is-sqlite-inside-the-case.md)).
//!
//! The user chooses an embedding model (`embedding.model` in settings, as `provider/name`):
//! one of rig's provider clients, or an Ollama server for an offline machine. There is no
//! default and no model in the process. [`Embedder`] is the one thing the rest needs from
//! it, and rig stays on this side of it.
//!
//! [`embed_pending`] is the background worker: it takes the passages a model has no vector
//! for, in small batches, and stores what comes back. It can be stopped at any batch and
//! picked up again; passages are stored with the fact that they are, in one transaction.

use std::sync::Arc;

use reactor_db::Case;
use reactor_db::corpus::{self, Hit, Model, Progress, Query};
use rig_core::client::{EmbeddingsClient, ProviderClient};
use rig_core::embeddings::EmbeddingModel;
use rig_core::providers::{gemini, ollama, openai};
use rig_core::vector_store::request::Filter;
use rig_core::vector_store::{VectorSearchRequest, VectorStoreError, VectorStoreIndex};
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::tools::BoxFut;

/// Passages per request to the provider, and per transaction.
const BATCH: usize = 32;

/// Something that turns text into vectors.
pub trait Embedder: Send + Sync {
    /// `provider/name`, as the user chose it: what the case records.
    fn name(&self) -> String;
    /// One vector per text, in order. All of one length.
    fn embed<'a>(&'a self, texts: Vec<String>) -> BoxFut<'a, Result<Vec<Vec<f32>>>>;
}

/// Where the current embedder is kept, shared between the window and the agent, so that
/// choosing a model takes effect in a session that is already running.
#[derive(Clone, Default)]
pub struct EmbedderSlot(Arc<std::sync::RwLock<Option<Arc<dyn Embedder>>>>);

impl EmbedderSlot {
    pub fn get(&self) -> Option<Arc<dyn Embedder>> {
        self.0.read().unwrap().clone()
    }

    pub fn set(&self, embedder: Option<Arc<dyn Embedder>>) {
        *self.0.write().unwrap() = embedder;
    }
}

/// An embedding model of a rig provider.
pub struct RigEmbedder<M: EmbeddingModel> {
    model: M,
    spec: String,
}

impl<M: EmbeddingModel> RigEmbedder<M> {
    pub fn new(model: M, spec: &str) -> Self {
        RigEmbedder {
            model,
            spec: spec.to_string(),
        }
    }
}

impl<M: EmbeddingModel + 'static> Embedder for RigEmbedder<M> {
    fn name(&self) -> String {
        self.spec.clone()
    }

    fn embed<'a>(&'a self, texts: Vec<String>) -> BoxFut<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move {
            let mut out = Vec::with_capacity(texts.len());
            for group in texts.chunks(M::MAX_DOCUMENTS.clamp(1, 64)) {
                let got = self
                    .model
                    .embed_texts(group.to_vec())
                    .await
                    .map_err(|e| Error::Model(format!("{}: {e}", self.spec)))?;
                if got.len() != group.len() {
                    return Err(Error::Model(format!(
                        "{}: asked for {} vectors and got {}",
                        self.spec,
                        group.len(),
                        got.len()
                    )));
                }
                out.extend(
                    got.into_iter()
                        .map(|e| e.vec.into_iter().map(|x| x as f32).collect::<Vec<f32>>()),
                );
            }
            Ok(out)
        })
    }
}

/// The providers that serve embeddings, by name. (Anthropic has none.)
pub const EMBEDDING_PROVIDERS: [&str; 3] = ["openai", "gemini", "ollama"];

/// `provider/name` → an embedder. Credentials come from the provider's usual environment
/// variable, as for the chat model.
pub fn from_spec(spec: &str) -> Result<Arc<dyn Embedder>> {
    let (provider, model) = spec.split_once('/').ok_or_else(|| {
        Error::Model(format!(
            "an embedding model is provider/name, e.g. ollama/nomic-embed-text (providers: {})",
            EMBEDDING_PROVIDERS.join(", ")
        ))
    })?;
    let fail = |e: rig_core::client::ProviderClientError| Error::Model(format!("{provider}: {e}"));
    Ok(match provider {
        "openai" => Arc::new(RigEmbedder::new(
            openai::Client::from_env()
                .map_err(fail)?
                .embedding_model(model),
            spec,
        )),
        "gemini" => Arc::new(RigEmbedder::new(
            gemini::Client::from_env()
                .map_err(fail)?
                .embedding_model(model),
            spec,
        )),
        "ollama" => Arc::new(RigEmbedder::new(
            ollama::Client::from_env()
                .map_err(fail)?
                .embedding_model(model),
            spec,
        )),
        other => {
            return Err(Error::Model(format!(
                "`{other}` does not serve embeddings here (providers: {})",
                EMBEDDING_PROVIDERS.join(", ")
            )));
        }
    })
}

/// A deterministic stand-in for tests: a word-hashing embedding, so that texts that share
/// words are near each other. Counts the requests it was asked.
pub struct ScriptedEmbedder {
    name: String,
    dim: usize,
    pub calls: std::sync::atomic::AtomicUsize,
    /// Fail every request after this many (`None`: never).
    pub fail_after: Option<usize>,
}

impl ScriptedEmbedder {
    pub fn new(name: &str, dim: usize) -> Self {
        ScriptedEmbedder {
            name: name.to_string(),
            dim,
            calls: Default::default(),
            fail_after: None,
        }
    }

    pub fn vector(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; self.dim];
        for word in text.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
            if word.is_empty() {
                continue;
            }
            let h = word.bytes().fold(14695981039346656037u64, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(1099511628211)
            });
            v[(h % self.dim as u64) as usize] += 1.0;
        }
        if v.iter().all(|x| *x == 0.0) {
            v[0] = 1.0;
        }
        v
    }
}

impl Embedder for ScriptedEmbedder {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn embed<'a>(&'a self, texts: Vec<String>) -> BoxFut<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.fail_after.is_some_and(|k| n >= k) {
                return Err(Error::Model(format!("{}: the provider is down", self.name)));
            }
            Ok(texts.iter().map(|t| self.vector(t)).collect())
        })
    }
}

/// What [`embed_pending`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Embedded {
    pub model: Option<Model>,
    pub progress: Progress,
    /// Passages embedded in this run.
    pub now: usize,
    /// It stopped because it was told to.
    pub cancelled: bool,
}

/// The model's registry entry: found by name, or made by asking the model what size its
/// vectors are — the only reliable way, since a provider's model list does not say.
async fn model_for(case: &Case, embedder: &dyn Embedder) -> Result<Model> {
    let name = embedder.name();
    if let Some(m) = corpus::models(case.db())?
        .into_iter()
        .find(|m| m.name == name && m.revision.is_empty())
    {
        return Ok(m);
    }
    let probe = embedder.embed(vec!["dimension probe".to_string()]).await?;
    let dim = probe.first().map_or(0, Vec::len);
    Ok(corpus::register_model(case.db(), &name, "", dim)?)
}

/// Make the vectors the case's passages are missing for this model. `on_progress` is told
/// after every batch. Returns early, with what was done, when `cancel` is set; and does
/// nothing if another embedding run holds the case.
pub async fn embed_pending(
    case: &Case,
    embedder: &dyn Embedder,
    cancel: &CancellationToken,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<Embedded> {
    let Some(_lock) = case.lock_embedding()? else {
        return Err(Error::Store(
            "another embedding run is already going on this case".into(),
        ));
    };
    if !corpus::has_documents(case.db())? {
        return Ok(Embedded {
            model: None,
            progress: Progress {
                embedded: 0,
                total: 0,
            },
            now: 0,
            cancelled: false,
        });
    }
    let model = model_for(case, embedder).await?;
    let mut now = 0;
    loop {
        if cancel.is_cancelled() {
            return Ok(Embedded {
                progress: corpus::progress(case.db(), model.id)?,
                model: Some(model),
                now,
                cancelled: true,
            });
        }
        let batch = corpus::pending(case.db(), model.id, BATCH)?;
        if batch.is_empty() {
            break;
        }
        let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
        let vectors = embedder.embed(texts).await?;
        if vectors.len() != batch.len() {
            return Err(Error::Model(format!(
                "{}: asked for {} vectors and got {}",
                embedder.name(),
                batch.len(),
                vectors.len()
            )));
        }
        let rows: Vec<(i64, Vec<f32>)> = batch.iter().map(|(id, _)| *id).zip(vectors).collect();
        let stored = corpus::store_vectors(case.db(), &model, rows)?;
        now += stored;
        on_progress(corpus::progress(case.db(), model.id)?);
        // Every passage was handed out and none stored: they are gone, not pending.
        if stored == 0 {
            break;
        }
    }
    Ok(Embedded {
        progress: corpus::progress(case.db(), model.id)?,
        model: Some(model),
        now,
        cancelled: false,
    })
}

/// How a search ran: the passages, and whether meaning played a part.
pub struct Found {
    pub hits: Vec<Hit>,
    /// `None`: keywords only, and why.
    pub meaning: std::result::Result<Model, String>,
    /// The similarity a passage needed to count as a match by meaning (0: none was asked).
    pub min_similarity: f32,
}

/// Which rankings a search uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Both, fused; by keyword alone where meaning is not available.
    #[default]
    Both,
    /// The words only: exact identifiers.
    Keyword,
    /// The meaning only: an error where it is not available.
    Meaning,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_lowercase().as_str() {
            "both" | "hybrid" => Some(Mode::Both),
            "keyword" | "keywords" => Some(Mode::Keyword),
            "semantic" | "meaning" => Some(Mode::Meaning),
            _ => None,
        }
    }
}

/// Whether a search by meaning can be made now: a model is chosen and has a vector for
/// every passage. What decides whether the agent is offered the choice.
pub fn meaning_ready(case: &Case, embedder: Option<&dyn Embedder>) -> bool {
    let Some(embedder) = embedder else {
        return false;
    };
    let want = embedder.name();
    matches!(
        corpus::usable_model(case.db(), Some(&want)),
        Ok(Some(m)) if m.name == want
    )
}

/// Search the corpus. With an embedder whose vectors are complete, the query is embedded and
/// both rankings are fused; otherwise it is by keyword, and `meaning` says why. A query is
/// never embedded by a model other than the one that made the stored vectors.
pub async fn search(
    case: &Case,
    embedder: Option<&dyn Embedder>,
    text: &str,
    limit: usize,
) -> Result<Found> {
    search_by(case, embedder, text, limit, Mode::Both, 0.0).await
}

/// [`search`] with the rankings chosen. `Keyword` never calls the model; `Meaning` is
/// refused, with the reason, where the vectors are not there.
pub async fn search_by(
    case: &Case,
    embedder: Option<&dyn Embedder>,
    text: &str,
    limit: usize,
    mode: Mode,
    min_similarity: f32,
) -> Result<Found> {
    if mode == Mode::Keyword {
        let hits = corpus::search(
            case.db(),
            &Query {
                text,
                vector: None,
                limit,
            },
        )?;
        return Ok(Found {
            hits,
            meaning: Err("keyword search was asked for".into()),
            min_similarity,
        });
    }
    let Some(embedder) = embedder else {
        if mode == Mode::Meaning {
            return Err(Error::Model(
                "no embedding model is chosen (set `embedding.model`)".into(),
            ));
        }
        let hits = corpus::search(
            case.db(),
            &Query {
                text,
                vector: None,
                limit,
            },
        )?;
        return Ok(Found {
            hits,
            meaning: Err("no embedding model is chosen (set `embedding.model`)".into()),
            min_similarity,
        });
    };
    let want = embedder.name();
    let usable = corpus::usable_model(case.db(), Some(&want))?;
    let (model, why) = match usable {
        Some(m) if m.name == want => (Some(m), String::new()),
        Some(m) => (
            None,
            format!(
                "{want} has not finished embedding the passages; the vectors that are complete are {}'s, which is not the model that would embed the query",
                m.name
            ),
        ),
        None => (None, format!("{want} has not embedded the passages yet")),
    };
    let Some(model) = model else {
        if mode == Mode::Meaning {
            return Err(Error::Model(why));
        }
        let hits = corpus::search(
            case.db(),
            &Query {
                text,
                vector: None,
                limit,
            },
        )?;
        return Ok(Found {
            hits,
            meaning: Err(why),
            min_similarity,
        });
    };
    let mut vectors = embedder.embed(vec![text.to_string()]).await?;
    let vector = vectors.pop().unwrap_or_default();
    let hits = corpus::search_with(
        case.db(),
        &Query {
            text,
            vector: Some((&model, &vector)),
            limit,
        },
        &corpus::Options {
            keywords: mode == Mode::Both,
            min_similarity,
        },
    )?;
    Ok(Found {
        hits,
        meaning: Ok(model),
        min_similarity,
    })
}

// -- why a passage matched by meaning ---------------------------------------------------------

/// A sentence of a passage and how close its meaning is to a query's (cosine, −1 to 1).
#[derive(Debug, Clone, PartialEq)]
pub struct Sentence {
    /// Where it is in the passage, in bytes.
    pub range: std::ops::Range<usize>,
    pub score: f32,
}

/// At most this many sentences of a passage are looked at.
const MAX_SENTENCES: usize = 64;

/// The sentences of `text` as byte ranges: split after `.`, `!` or `?` that is followed by
/// space, and at line breaks; pieces under 25 characters join the next one, so that a
/// heading or an abbreviation is not a sentence of its own.
pub fn sentences(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut out: Vec<std::ops::Range<usize>> = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let end = i + c.len_utf8();
        let stop = c == '\n'
            || (matches!(c, '.' | '!' | '?')
                && chars.peek().is_none_or(|(_, n)| n.is_whitespace()));
        if stop {
            let piece = &text[start..end];
            if piece.trim().chars().count() >= 25 || end == text.len() {
                if !piece.trim().is_empty() {
                    let lead = piece.len() - piece.trim_start().len();
                    let trail = piece.len() - piece.trim_end().len();
                    out.push(start + lead..end - trail);
                }
                start = end;
            }
        }
    }
    let rest = &text[start..];
    if !rest.trim().is_empty() {
        let lead = rest.len() - rest.trim_start().len();
        let trail = rest.len() - rest.trim_end().len();
        out.push(start + lead..text.len() - trail);
    }
    out.truncate(MAX_SENTENCES);
    out
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let (na, nb) = (
        a.iter().map(|x| x * x).sum::<f32>().sqrt(),
        b.iter().map(|x| x * x).sum::<f32>().sqrt(),
    );
    if na == 0. || nb == 0. {
        0.
    } else {
        dot / (na * nb)
    }
}

/// Which sentences of `passage` are closest in meaning to `query`, best first. One request
/// to the embedding model, with the query and every sentence in it: the passage's text goes
/// to the provider again, so this is asked for, not done on its own. It must be the model
/// whose vectors found the passage, or the "meaning" is another model's.
pub async fn explain(embedder: &dyn Embedder, query: &str, passage: &str) -> Result<Vec<Sentence>> {
    let ranges = sentences(passage);
    if ranges.is_empty() {
        return Ok(Vec::new());
    }
    let mut texts = vec![query.to_string()];
    texts.extend(ranges.iter().map(|r| passage[r.clone()].to_string()));
    let mut vectors = embedder.embed(texts).await?;
    if vectors.len() != ranges.len() + 1 {
        return Err(Error::Model(format!(
            "{}: asked for {} vectors and got {}",
            embedder.name(),
            ranges.len() + 1,
            vectors.len()
        )));
    }
    let q = vectors.remove(0);
    let mut out: Vec<Sentence> = ranges
        .into_iter()
        .zip(vectors)
        .map(|(range, v)| Sentence {
            range,
            score: cosine(&q, &v),
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.range.start.cmp(&b.range.start))
    });
    Ok(out)
}

// -- rig's VectorStoreIndex ------------------------------------------------------------------

/// The corpus as a rig index: it works anywhere rig expects one, including as dynamic
/// context on an agent. What comes back is the passage with its heading path, source and
/// origin label. The request's filter is read against a passage's `source_kind`, `origin`,
/// `mime`, `source_uri`, `title` and `meta.*`.
pub struct ContextIndex {
    pub case: Case,
    pub embedder: Option<Arc<dyn Embedder>>,
}

/// Whether a passage satisfies a rig filter.
fn matches(filter: &Filter<Value>, hit: &Value) -> bool {
    let field = |key: &str| {
        key.split('.')
            .try_fold(hit, |v, part| v.get(part))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let number = |v: &Value| v.as_f64();
    match filter {
        Filter::Eq(k, v) => field(k) == *v,
        Filter::Gt(k, v) => number(&field(k)).zip(number(v)).is_some_and(|(a, b)| a > b),
        Filter::Lt(k, v) => number(&field(k)).zip(number(v)).is_some_and(|(a, b)| a < b),
        Filter::And(a, b) => matches(a, hit) && matches(b, hit),
        Filter::Or(a, b) => matches(a, hit) || matches(b, hit),
    }
}

impl VectorStoreIndex for ContextIndex {
    type Filter = Filter<Value>;

    async fn top_n<T: for<'a> Deserialize<'a> + Send>(
        &self,
        req: VectorSearchRequest<Self::Filter>,
    ) -> std::result::Result<Vec<(f64, String, T)>, VectorStoreError> {
        let store = |e: Error| VectorStoreError::datastore(std::io::Error::other(e.to_string()));
        // Cut deeper than asked when filtering, so what the filter leaves can still fill it.
        let asked = req.samples() as usize;
        let depth = if req.filter().is_some() {
            asked * 4
        } else {
            asked
        };
        let found = search(&self.case, self.embedder.as_deref(), req.query(), depth)
            .await
            .map_err(store)?;
        let mut out = Vec::new();
        for hit in found.hits {
            let json = serde_json::to_value(&hit)?;
            if req.filter().as_ref().is_some_and(|f| !matches(f, &json)) {
                continue;
            }
            if req.threshold().is_some_and(|t| hit.score < t) {
                continue;
            }
            out.push((
                hit.score,
                hit.chunk_id.to_string(),
                serde_json::from_value(json)?,
            ));
            if out.len() >= asked {
                break;
            }
        }
        Ok(out)
    }

    async fn top_n_ids(
        &self,
        req: VectorSearchRequest<Self::Filter>,
    ) -> std::result::Result<Vec<(f64, String)>, VectorStoreError> {
        Ok(self
            .top_n::<Value>(req)
            .await?
            .into_iter()
            .map(|(score, id, _)| (score, id))
            .collect())
    }
}
