use std::sync::Arc;

use tracing::{debug, warn};

use crate::domain::llm::EmbeddingProvider;
use crate::domain::rag::{KnowledgeChunk, KnowledgeDocument, RAGRepository};
use crate::domain::vector_store::{VectorFilter, VectorStore};
use crate::shared::error::AppError;

use super::vector_index_service::payload_chunk_id;

/// Visibility-based read permission check.
///
/// Trusts MySQL data only — Qdrant payloads must NOT substitute this.
fn can_read_document(document: &KnowledgeDocument, user_id: u64) -> bool {
    if document.status != 1 {
        return false;
    }
    if document.deleted_at.is_some() {
        return false;
    }
    match document.visibility.as_str() {
        "public" => true,
        "private" => document.owner_user_id == Some(user_id),
        "internal" | "admin_only" => false,
        _ => false, // unknown visibility → deny
    }
}

/// Hybrid retrieval service.
///
/// Strategy (Qdrant-first):
/// 1. Qdrant vector search  →  MySQL second validation  →  permission filter.
/// 2. Fallback: MySQL keyword search  →  same MySQL + permission validation.
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

    pub fn with_vector_store(mut self, vs: Arc<dyn VectorStore>, collection: String) -> Self {
        self.vector_store = Some(vs);
        self.rag_collection = collection;
        self
    }

    /// Retrieve top-k relevant chunks, scoped to `user_id`.
    ///
    /// Qdrant is preferred; keyword is a fallback with the same validation.
    pub async fn retrieve(
        &self,
        query: &str,
        user_id: u64,
        top_k: u64,
    ) -> Result<Vec<(KnowledgeChunk, f64)>, AppError> {
        // ── Qdrant path ─────────────────────────────────────────
        if let (Some(vs), Some(ep)) = (&self.vector_store, &self.embedding) {
            match self.qdrant_retrieve(vs, ep, query, user_id, top_k).await {
                Ok(results) if !results.is_empty() => {
                    debug!(count = results.len(), "Qdrant retrieval succeeded");
                    return Ok(results);
                }
                Ok(_) => debug!("Qdrant returned empty; fallback to keyword"),
                Err(e) => warn!(error = %e, "Qdrant retrieval failed; fallback to keyword"),
            }
        }

        // ── Fallback: MySQL keyword  →  same validation ────────
        self.keyword_retrieve_with_validation(query, user_id, top_k)
            .await
    }

    // ── Qdrant path ────────────────────────────────────────────────

    async fn qdrant_retrieve(
        &self,
        vs: &Arc<dyn VectorStore>,
        ep: &Arc<dyn EmbeddingProvider>,
        query: &str,
        user_id: u64,
        top_k: u64,
    ) -> Result<Vec<(KnowledgeChunk, f64)>, AppError> {
        let vecs = ep
            .embed(&[query.to_string()])
            .await
            .map_err(|e| AppError::internal(format!("query embedding failed: {e}")))?;
        let query_vec = vecs
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Internal("embedding returned empty vector".to_string()))?;

        let hits = vs
            .search(
                &self.rag_collection,
                query_vec,
                VectorFilter::default(),
                top_k as usize,
            )
            .await
            .map_err(|e| AppError::internal(format!("Qdrant search failed: {e}")))?;

        self.validate_hits(&hits, user_id).await
    }

    // ── Keyword fallback with identical validation ─────────────────

    async fn keyword_retrieve_with_validation(
        &self,
        query: &str,
        user_id: u64,
        top_k: u64,
    ) -> Result<Vec<(KnowledgeChunk, f64)>, AppError> {
        let raw = self.repo.search_by_keyword(query, top_k).await?;
        let mut results = Vec::with_capacity(raw.len());
        for (chunk, score) in raw {
            // Validate chunk status
            if chunk.status != 1 {
                debug!(
                    chunk_id = chunk.chunk_id,
                    "keyword chunk disabled; skipping"
                );
                continue;
            }
            // Load & validate document
            match self.repo.find_document_by_id(chunk.document_id).await {
                Ok(Some(doc)) => {
                    if can_read_document(&doc, user_id) {
                        results.push((chunk, score));
                    } else {
                        debug!(chunk_id = chunk.chunk_id, doc_id = doc.document_id,
                               visibility = %doc.visibility, "keyword chunk denied by visibility");
                    }
                }
                Ok(None) => {
                    debug!(
                        chunk_id = chunk.chunk_id,
                        "keyword chunk document missing; skipping"
                    );
                }
                Err(e) => {
                    warn!(chunk_id = chunk.chunk_id, error = %e, "keyword chunk doc lookup failed; skipping");
                }
            }
        }
        Ok(results)
    }

    // ── Shared MySQL validation ────────────────────────────────────

    async fn validate_hits(
        &self,
        hits: &[crate::domain::vector_store::VectorSearchHit],
        user_id: u64,
    ) -> Result<Vec<(KnowledgeChunk, f64)>, AppError> {
        let mut results = Vec::with_capacity(hits.len());
        for hit in hits {
            let chunk_id = match payload_chunk_id(&hit.payload) {
                Some(id) => id,
                None => {
                    warn!(hit_id = %hit.id, "Qdrant hit missing chunk_id; skipping");
                    continue;
                }
            };

            // 1. Load chunk from MySQL
            let chunk = match self.repo.find_chunk_by_id(chunk_id).await {
                Ok(Some(c)) => c,
                Ok(None) => {
                    debug!(chunk_id, "chunk not found in MySQL; skipping");
                    continue;
                }
                Err(e) => {
                    warn!(chunk_id, error = %e, "chunk lookup failed; skipping");
                    continue;
                }
            };

            // 2. Chunk status
            if chunk.status != 1 {
                debug!(
                    chunk_id,
                    "chunk disabled (status={}); skipping", chunk.status
                );
                continue;
            }

            // 3. Load document
            let document = match self.repo.find_document_by_id(chunk.document_id).await {
                Ok(Some(d)) => d,
                Ok(None) => {
                    debug!(doc_id = chunk.document_id, "document missing; skipping");
                    continue;
                }
                Err(e) => {
                    warn!(doc_id = chunk.document_id, error = %e, "document lookup failed; skipping");
                    continue;
                }
            };

            // 4. Permission + lifecycle check
            if !can_read_document(&document, user_id) {
                debug!(chunk_id, doc_id = document.document_id,
                       visibility = %document.visibility,
                       "permission denied");
                continue;
            }

            results.push((chunk, hit.score as f64));
        }
        Ok(results)
    }

    // ── Legacy helpers (embedding_json fallback only) ──────────────

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
        let q_vec = query_embedding.first().cloned().unwrap_or_default();
        if q_vec.is_empty() {
            return Ok(Vec::new());
        }
        let chunks_with_embs = self.repo.list_chunks_with_embeddings().await?;
        let mut scored: Vec<(KnowledgeChunk, f64)> = chunks_with_embs
            .into_iter()
            .filter_map(|(chunk, emb)| {
                let stored_vec: Vec<f32> = serde_json::from_value(emb.embedding_json).ok()?;
                Some((chunk, cosine_similarity(&q_vec, &stored_vec)))
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
            .map(|(c, _)| (c.chunk_id, c.clone()))
            .collect();
        let kw_max = keyword
            .iter()
            .map(|(_, s)| *s)
            .fold(f64::NEG_INFINITY, f64::max);
        let kw_map: BTreeMap<u64, f64> = keyword
            .into_iter()
            .map(|(c, s)| (c.chunk_id, if kw_max > 0.0 { s / kw_max } else { 0.0 }))
            .collect();
        let vec_max = vector
            .iter()
            .map(|(_, s)| *s)
            .fold(f64::NEG_INFINITY, f64::max);
        let vec_map: BTreeMap<u64, f64> = vector
            .into_iter()
            .map(|(c, s)| (c.chunk_id, if vec_max > 0.0 { s / vec_max } else { 0.0 }))
            .collect();
        let mut all_ids: Vec<u64> = kw_map.keys().chain(vec_map.keys()).copied().collect();
        all_ids.sort();
        all_ids.dedup();
        let mut scored: Vec<(u64, f64)> = all_ids
            .into_iter()
            .map(|id| {
                let kw = kw_map.get(&id).copied().unwrap_or(0.0);
                let vec = vec_map.get(&id).copied().unwrap_or(0.0);
                (id, 0.6 * vec + 0.4 * kw)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k as usize);
        scored
            .into_iter()
            .filter_map(|(id, score)| chunks_by_id.get(&id).map(|c| (c.clone(), score)))
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

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::rag::{NewChunk, NewDocument, NewEmbedding};
    use async_trait::async_trait;
    use chrono::Utc;

    // ── can_read_document unit tests ────────────────────────────

    fn make_doc(
        status: i8,
        deleted: bool,
        visibility: &str,
        owner: Option<u64>,
    ) -> KnowledgeDocument {
        KnowledgeDocument {
            document_id: 1,
            source_type: "test".into(),
            source_id: None,
            owner_user_id: owner,
            visibility: visibility.into(),
            title: None,
            content_hash: "hash".into(),
            source_version: None,
            source_updated_at: None,
            metadata: None,
            status,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: if deleted { Some(Utc::now()) } else { None },
        }
    }

    #[test]
    fn test_can_read_public() {
        let doc = make_doc(1, false, "public", None);
        assert!(can_read_document(&doc, 42));
    }

    #[test]
    fn test_cannot_read_disabled_document() {
        let doc = make_doc(0, false, "public", None);
        assert!(!can_read_document(&doc, 42));
    }

    #[test]
    fn test_cannot_read_deleted_document() {
        let doc = make_doc(1, true, "public", None);
        assert!(!can_read_document(&doc, 42));
    }

    #[test]
    fn test_private_owner_can_read() {
        let doc = make_doc(1, false, "private", Some(42));
        assert!(can_read_document(&doc, 42));
    }

    #[test]
    fn test_private_non_owner_cannot_read() {
        let doc = make_doc(1, false, "private", Some(42));
        assert!(!can_read_document(&doc, 99));
    }

    #[test]
    fn test_internal_denied() {
        let doc = make_doc(1, false, "internal", None);
        assert!(!can_read_document(&doc, 42));
    }

    #[test]
    fn test_admin_only_denied() {
        let doc = make_doc(1, false, "admin_only", None);
        assert!(!can_read_document(&doc, 42));
    }

    #[test]
    fn test_unknown_visibility_denied() {
        let doc = make_doc(1, false, "classified", None);
        assert!(!can_read_document(&doc, 42));
    }
}
