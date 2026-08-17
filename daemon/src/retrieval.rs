//! Local retrieval projection over the canonical iroh learning store.
//!
//! Embeddings are deliberately not written to iroh: every peer derives this in-memory index
//! from its local replica. That keeps model-specific vectors out of synchronized state.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use serde::Serialize;

use crate::learnings::Learning;

const LEXICAL_WEIGHT: f32 = 0.45;
const SEMANTIC_WEIGHT: f32 = 0.55;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum RetrievalMode {
    Lexical,
    Semantic,
    #[default]
    Hybrid,
}

impl RetrievalMode {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "lexical" => Ok(Self::Lexical),
            "semantic" => Ok(Self::Semantic),
            "hybrid" | "" => Ok(Self::Hybrid),
            _ => Err(anyhow!("mode must be lexical, semantic, or hybrid")),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RetrievalResult {
    pub id: String,
    pub title: String,
    pub excerpt: String,
    pub tags: Vec<String>,
    pub author: String,
    pub created: u64,
    pub score: f32,
    pub lexical_score: f32,
    pub semantic_score: Option<f32>,
    pub match_type: &'static str,
}

struct IndexState {
    model: Option<TextEmbedding>,
    vectors: HashMap<String, Vec<f32>>,
    indexed_fingerprints: HashMap<String, String>,
}

impl Default for IndexState {
    fn default() -> Self {
        Self {
            model: None,
            vectors: HashMap::new(),
            indexed_fingerprints: HashMap::new(),
        }
    }
}

/// Cheaply cloneable local semantic index. ONNX inference runs on a blocking worker thread.
#[derive(Clone, Default)]
pub struct RetrievalIndex(Arc<Mutex<IndexState>>);

impl RetrievalIndex {
    pub async fn search(
        &self,
        learnings: Vec<Learning>,
        query: String,
        mode: RetrievalMode,
        top_k: usize,
        required_tags: Vec<String>,
        excerpt_chars: usize,
        min_score: f32,
    ) -> Result<Vec<RetrievalResult>> {
        let state = self.0.clone();
        tokio::task::spawn_blocking(move || {
            search_blocking(
                state,
                learnings,
                query,
                mode,
                top_k,
                required_tags,
                excerpt_chars,
                min_score,
            )
        })
        .await
        .context("retrieval worker stopped")?
    }
}

fn search_blocking(
    state: Arc<Mutex<IndexState>>,
    learnings: Vec<Learning>,
    query: String,
    mode: RetrievalMode,
    top_k: usize,
    required_tags: Vec<String>,
    excerpt_chars: usize,
    min_score: f32,
) -> Result<Vec<RetrievalResult>> {
    let required_tags: Vec<String> = required_tags
        .into_iter()
        .map(|t| t.to_lowercase())
        .collect();
    let candidates: Vec<Learning> = learnings
        .into_iter()
        .filter(|l| {
            required_tags
                .iter()
                .all(|wanted| l.tags.iter().any(|tag| tag.to_lowercase() == *wanted))
        })
        .collect();

    let lexical: Vec<f32> = candidates
        .iter()
        .map(|l| lexical_score(l, &query))
        .collect();
    let semantic = if mode == RetrievalMode::Lexical {
        None
    } else {
        Some(semantic_scores(&state, &candidates, &query)?)
    };

    let mut results: Vec<RetrievalResult> = candidates
        .into_iter()
        .enumerate()
        .filter_map(|(i, learning)| {
            let lexical_score = lexical[i];
            let semantic_score = semantic.as_ref().map(|scores| scores[i]);
            let score = match mode {
                RetrievalMode::Lexical => lexical_score,
                RetrievalMode::Semantic => semantic_score.unwrap_or(0.0),
                RetrievalMode::Hybrid => {
                    LEXICAL_WEIGHT * lexical_score + SEMANTIC_WEIGHT * semantic_score.unwrap_or(0.0)
                }
            };
            (score >= min_score).then(|| RetrievalResult {
                excerpt: excerpt(&learning.body, excerpt_chars),
                id: learning.id,
                title: learning.title,
                tags: learning.tags,
                author: learning.author,
                created: learning.created,
                score,
                lexical_score,
                semantic_score,
                match_type: match mode {
                    RetrievalMode::Lexical => "lexical",
                    RetrievalMode::Semantic => "semantic",
                    RetrievalMode::Hybrid => "hybrid",
                },
            })
        })
        .collect();
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
    results.truncate(top_k);
    Ok(results)
}

fn semantic_scores(
    state: &Arc<Mutex<IndexState>>,
    learnings: &[Learning],
    query: &str,
) -> Result<Vec<f32>> {
    let mut state = state
        .lock()
        .map_err(|_| anyhow!("semantic index lock poisoned"))?;
    if state.model.is_none() {
        let options = TextInitOptions::new(EmbeddingModel::AllMiniLML6V2Q)
            .with_show_download_progress(false)
            .with_intra_threads(2);
        state.model =
            Some(TextEmbedding::try_new(options).context("loading local embedding model")?);
    }

    // Incrementally embed only records whose searchable text is new or changed. The learning ID
    // excludes tags, so the projection uses its own fingerprint rather than trusting ID alone.
    let changed: Vec<(&Learning, String)> = learnings
        .iter()
        .filter_map(|learning| {
            let text = embedding_text(learning);
            let fingerprint = blake3::hash(text.as_bytes()).to_hex().to_string();
            (state.indexed_fingerprints.get(&learning.id) != Some(&fingerprint))
                .then_some((learning, fingerprint))
        })
        .collect();
    if !changed.is_empty() {
        let passages: Vec<String> = changed
            .iter()
            .map(|(learning, _)| embedding_text(learning))
            .collect();
        let vectors = state
            .model
            .as_mut()
            .expect("model initialized")
            .embed(passages, None)?;
        for ((learning, fingerprint), vector) in changed.into_iter().zip(vectors) {
            state.vectors.insert(learning.id.clone(), vector);
            state
                .indexed_fingerprints
                .insert(learning.id.clone(), fingerprint);
        }
    }

    let query_vector = state
        .model
        .as_mut()
        .expect("model initialized")
        .embed(vec![format!("query: {query}")], None)?
        .pop()
        .ok_or_else(|| anyhow!("embedding model returned no query vector"))?;

    Ok(learnings
        .iter()
        .map(|l| {
            state
                .vectors
                .get(&l.id)
                .map(|v| cosine(&query_vector, v).max(0.0))
                .unwrap_or(0.0)
        })
        .collect())
}

fn embedding_text(learning: &Learning) -> String {
    format!(
        "passage: {}\n{}\nTags: {}",
        learning.title,
        learning.body,
        learning.tags.join(", ")
    )
}

/// Bounded lexical relevance: exact phrase plus coverage of distinct query terms.
fn lexical_score(learning: &Learning, query: &str) -> f32 {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return 0.0;
    }
    let title = learning.title.to_lowercase();
    let body = learning.body.to_lowercase();
    let tags = learning.tags.join(" ").to_lowercase();
    let terms: HashSet<&str> = query.split_whitespace().collect();
    let matched = terms
        .iter()
        .filter(|term| title.contains(**term) || body.contains(**term) || tags.contains(**term))
        .count();
    let coverage = matched as f32 / terms.len().max(1) as f32;
    let phrase = if title.contains(&query) {
        1.0
    } else if tags.contains(&query) {
        0.9
    } else if body.contains(&query) {
        0.8
    } else {
        0.0
    };
    (0.65 * phrase + 0.35 * coverage).min(1.0)
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let a_norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let b_norm: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if a_norm == 0.0 || b_norm == 0.0 {
        0.0
    } else {
        dot / (a_norm * b_norm)
    }
}

fn excerpt(body: &str, max_chars: usize) -> String {
    let mut chars = body.chars();
    let text: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{}…", text.trim_end())
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn learning(title: &str, body: &str, tags: &[&str]) -> Learning {
        Learning::new(
            title.into(),
            body.into(),
            tags.iter().map(|s| s.to_string()).collect(),
            "test".into(),
        )
    }

    #[test]
    fn lexical_ranking_prefers_title_phrase() {
        let title = learning("Retry temporary API failures", "Use backoff.", &["api"]);
        let body = learning(
            "Networking",
            "Retry temporary API failures with backoff.",
            &["ops"],
        );
        assert!(
            lexical_score(&title, "temporary API failures")
                > lexical_score(&body, "temporary API failures")
        );
    }

    #[test]
    fn excerpts_are_unicode_safe_and_bounded() {
        assert_eq!(excerpt("hello world", 5), "hello…");
        assert_eq!(excerpt("éclair", 1), "é…");
    }
}
