use std::sync::Arc;

use crate::domain::llm::EmbeddingProvider;
use crate::domain::rag::{KnowledgeChunk, RAGRepository};
use crate::shared::error::AppError;

/// Retrieval service implementing hybrid search.
///
/// Strategy:
/// 1. Always runs a FULLTEXT keyword search.
/// 2. If an `EmbeddingProvider` is configured, also generates a query
///    embedding and computes cosine similarity against stored chunk
///    embeddings.
/// 3. Merges results with a hybrid score: `0.6 * vec_score + 0.4 * keyword_score`.
/// 4. If the embedding step fails, falls back to FULLTEXT-only results.
pub struct RetrievalService {
    repo: Arc<dyn RAGRepository>,
    embedding: Option<Arc<dyn EmbeddingProvider>>,
}

impl RetrievalService {
    pub fn new(repo: Arc<dyn RAGRepository>, embedding: Option<Arc<dyn EmbeddingProvider>>) -> Self {
        Self { repo, embedding }
    }

    /// Retrieve the top-k relevant chunks for `query` scoped to `user_id`.
    ///
    /// `user_id` is reserved for future per-user filtering (e.g. ACL on
    /// documents); the current implementation ignores it and returns
    /// globally matching chunks.
    pub async fn retrieve(
        &self,
        query: &str,
        _user_id: u64,
        top_k: u64,
    ) -> Result<Vec<(KnowledgeChunk, f64)>, AppError> {
        // ── 1. Keyword search (always runs) ──
        let keyword_results = self.repo.search_by_keyword(query, top_k).await?;

        let embedding_provider = match self.embedding {
            Some(ref p) => p,
            None => return Ok(keyword_results),
        };

        // ── 2. Vector search ──
        let vec_results = match self.vector_search(embedding_provider, query, top_k).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(?e, "vector search failed — falling back to keyword only");
                return Ok(keyword_results);
            }
        };

        // ── 3. Hybrid merge ──
        Ok(Self::hybrid_merge(keyword_results, vec_results, top_k))
    }

    // ── private helpers ──

    async fn vector_search(
        &self,
        provider: &Arc<dyn EmbeddingProvider>,
        query: &str,
        top_k: u64,
    ) -> Result<Vec<(KnowledgeChunk, f64)>, AppError> {
        let query_embedding = provider
            .embed(&[query.to_string()])
            .await
            .map_err(|e| AppError::Internal(format!("embedding failed: {e}")))?;

        let q_vec = match query_embedding.first() {
            Some(v) => v,
            None => return Ok(Vec::new()),
        };

        let chunks_with_embs = self.repo.list_chunks_with_embeddings().await?;

        let mut scored: Vec<(KnowledgeChunk, f64)> = chunks_with_embs
            .into_iter()
            .filter_map(|(chunk, emb)| {
                let stored_vec: Vec<f32> =
                    serde_json::from_value(emb.embedding_json).ok()?;
                let sim = cosine_similarity(q_vec, &stored_vec);
                Some((chunk, sim))
            })
            .collect();

        // sort descending by similarity
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k as usize);
        Ok(scored)
    }

    fn hybrid_merge(
        keyword: Vec<(KnowledgeChunk, f64)>,
        vector: Vec<(KnowledgeChunk, f64)>,
        top_k: u64,
    ) -> Vec<(KnowledgeChunk, f64)> {
        use std::collections::BTreeMap;

        // Capture full chunk data by id before consuming the vectors for scoring.
        let chunks_by_id: BTreeMap<u64, KnowledgeChunk> = keyword
            .iter()
            .chain(vector.iter())
            .map(|(chunk, _)| (chunk.chunk_id, chunk.clone()))
            .collect();

        // Normalize keyword scores to [0, 1].
        let kw_max = keyword
            .iter()
            .map(|(_, s)| *s)
            .fold(f64::NEG_INFINITY, f64::max);
        let kw_map: BTreeMap<u64, f64> = keyword
            .into_iter()
            .map(|(chunk, score)| {
                let norm = if kw_max > 0.0 { score / kw_max } else { 0.0 };
                (chunk.chunk_id, norm)
            })
            .collect();

        // Normalize vector scores to [0, 1].
        let vec_max = vector
            .iter()
            .map(|(_, s)| *s)
            .fold(f64::NEG_INFINITY, f64::max);
        let vec_map: BTreeMap<u64, f64> = vector
            .into_iter()
            .map(|(chunk, score)| {
                let norm = if vec_max > 0.0 { score / vec_max } else { 0.0 };
                (chunk.chunk_id, norm)
            })
            .collect();

        // Collect all chunk IDs present in either set.
        let mut all_ids: Vec<u64> = kw_map
            .keys()
            .chain(vec_map.keys())
            .copied()
            .collect();
        all_ids.sort();
        all_ids.dedup();

        // Hybrid score: 0.6 * vec + 0.4 * keyword
        let mut scored: Vec<(u64, f64)> = all_ids
            .into_iter()
            .map(|id| {
                let kw_score = kw_map.get(&id).copied().unwrap_or(0.0);
                let vec_score = vec_map.get(&id).copied().unwrap_or(0.0);
                (id, 0.6 * vec_score + 0.4 * kw_score)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k as usize);

        // Reconstruct (chunk, score) pairs using the saved chunk data.
        scored
            .into_iter()
            .filter_map(|(id, score)| {
                chunks_by_id.get(&id).map(|chunk| (chunk.clone(), score))
            })
            .collect()
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)) as f64
}
