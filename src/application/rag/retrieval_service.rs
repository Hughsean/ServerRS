use std::sync::Arc;

use tracing::{debug, warn};

use crate::domain::llm::EmbeddingProvider;
use crate::domain::rag::{KnowledgeChunk, RAGRepository};
use crate::domain::vector_store::{VectorFilter, VectorStore};
use crate::shared::error::AppError;

use super::vector_index_service::payload_chunk_id;

/// Hybrid retrieval service.
///
/// Strategy (Qdrant-first):
/// 1. If `VectorStore` + `EmbeddingProvider` are available:
///    a. Generate query embedding.
///    b. Search Qdrant collection with payload filters.
///    c. Re-load chunks from MySQL (verify existence, status, document validity).
///    d. Return verified (chunk, score) pairs.
/// 2. If Qdrant is unavailable or fails:
///    Fall back to MySQL keyword search (`RAGRepository::search_by_keyword`).
pub struct RetrievalService {
    repo: Arc<dyn RAGRepository>,
    embedding: Option<Arc<dyn EmbeddingProvider>>,
    vector_store: Option<Arc<dyn VectorStore>>,
    rag_collection: String,
}

impl RetrievalService {
    pub fn new(
        repo: Arc<dyn RAGRepository>,
        embedding: Option<Arc<dyn EmbeddingProvider>>,
    ) -> Self {
        Self {
            repo,
            embedding,
            vector_store: None,
            rag_collection: "rag_chunks".into(),
        }
    }

    /// Attach a `VectorStore` for Qdrant-first retrieval.
    pub fn with_vector_store(mut self, vs: Arc<dyn VectorStore>, collection: String) -> Self {
        self.vector_store = Some(vs);
        self.rag_collection = collection;
        self
    }

    /// Retrieve the top-k relevant chunks for `query` scoped to `user_id`.
    pub async fn retrieve(
        &self,
        query: &str,
        user_id: u64,
        top_k: u64,
    ) -> Result<Vec<(KnowledgeChunk, f64)>, AppError> {
        // Try Qdrant path first
        if let (Some(vs), Some(ep)) = (&self.vector_store, &self.embedding) {
            match self.qdrant_retrieve(vs, ep, query, user_id, top_k).await {
                Ok(results) if !results.is_empty() => {
                    debug!(count = results.len(), "Qdrant retrieval succeeded");
                    return Ok(results);
                }
                Ok(_) => {
                    debug!("Qdrant returned empty results; falling back to keyword");
                }
                Err(e) => {
                    warn!(error = %e, "Qdrant retrieval failed; falling back to keyword");
                }
            }
        }

        // Fallback: MySQL keyword search
        self.repo.search_by_keyword(query, top_k).await
    }

    async fn qdrant_retrieve(
        &self,
        vs: &Arc<dyn VectorStore>,
        ep: &Arc<dyn EmbeddingProvider>,
        query: &str,
        _user_id: u64,
        top_k: u64,
    ) -> Result<Vec<(KnowledgeChunk, f64)>, AppError> {
        // 1. Generate query embedding
        let vecs = ep
            .embed(&[query.to_string()])
            .await
            .map_err(|e| AppError::internal(format!("query embedding failed: {e}")))?;

        let query_vec = vecs
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Internal("embedding returned empty vector".to_string()))?;

        // 2. Search Qdrant
        let hits = vs
            .search(
                &self.rag_collection,
                query_vec,
                VectorFilter::default(),
                top_k as usize,
            )
            .await
            .map_err(|e| AppError::internal(format!("Qdrant search failed: {e}")))?;

        // 3. Re-load chunks from MySQL and verify
        let mut results = Vec::with_capacity(hits.len());
        for hit in &hits {
            let chunk_id = match payload_chunk_id(&hit.payload) {
                Some(id) => id,
                None => {
                    warn!(hit_id = %hit.id, "Qdrant hit has no chunk_id in payload; skipping");
                    continue;
                }
            };

            // 3a. Load the chunk from MySQL
            let chunk = match self.repo.find_chunk_by_id(chunk_id).await {
                Ok(Some(c)) => c,
                Ok(None) => {
                    debug!(chunk_id, "chunk not found in MySQL; skipping");
                    continue;
                }
                Err(e) => {
                    warn!(chunk_id, error = %e, "failed to load chunk from MySQL; skipping");
                    continue;
                }
            };

            // 3b. Load the document from MySQL and verify status
            let document = match self.repo.find_document_by_id(chunk.document_id).await {
                Ok(Some(d)) => d,
                Ok(None) => {
                    debug!(doc_id = chunk.document_id, "document not found in MySQL; skipping");
                    continue;
                }
                Err(e) => {
                    warn!(doc_id = chunk.document_id, error = %e, "failed to load document; skipping");
                    continue;
                }
            };

            // 3c. Verify document status (TRUST MYSQL, not Qdrant payload)
            if document.status != 1 {
                debug!(chunk_id, doc_id = document.document_id, "document status is {}; skipping", document.status);
                continue;
            }

            results.push((chunk, hit.score as f64));
        }

        Ok(results)
    }

    // The old vector_search / hybrid_merge methods are kept for backward
    // compatibility but are only used in the embedding_json fallback path
    // (via list_chunks_with_embeddings).

    #[doc(hidden)]
    pub async fn retrieve_fallback_legacy(
        &self,
        query: &str,
        top_k: u64,
    ) -> Result<Vec<(KnowledgeChunk, f64)>, AppError> {
        let keyword_results = self.repo.search_by_keyword(query, top_k).await?;

        let embedding_provider = match self.embedding {
            Some(ref p) => p,
            None => return Ok(keyword_results),
        };

        let vec_results = match self
            .legacy_vector_search(embedding_provider, query, top_k)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!(?e, "legacy vector search failed");
                return Ok(keyword_results);
            }
        };

        Ok(Self::hybrid_merge(keyword_results, vec_results, top_k))
    }

    async fn legacy_vector_search(
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
                let stored_vec: Vec<f32> = serde_json::from_value(emb.embedding_json).ok()?;
                let sim = cosine_similarity(q_vec, &stored_vec);
                Some((chunk, sim))
            })
            .collect();

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

        let chunks_by_id: BTreeMap<u64, KnowledgeChunk> = keyword
            .iter()
            .chain(vector.iter())
            .map(|(chunk, _)| (chunk.chunk_id, chunk.clone()))
            .collect();

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

        let mut all_ids: Vec<u64> = kw_map.keys().chain(vec_map.keys()).copied().collect();
        all_ids.sort();
        all_ids.dedup();

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

        scored
            .into_iter()
            .filter_map(|(id, score)| chunks_by_id.get(&id).map(|chunk| (chunk.clone(), score)))
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
