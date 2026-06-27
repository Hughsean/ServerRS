use std::sync::Arc;

use tracing::{debug, trace, warn};

use crate::domain::llm::EmbeddingProvider;
use crate::domain::rag::{KnowledgeChunk, KnowledgeDocument, RAGRepoT};
use crate::domain::vector_store::{VectorCondition, VectorFilter, VectorStoreT};
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
/// Strategy:
/// 1. Qdrant vector search  →  MySQL second validation  →  permission filter.
/// 2. MySQL keyword search  →  the same permission and lifecycle validation.
/// 3. Normalize and merge both result sets using configured hybrid weights.
///
/// Web-ingestion content lives in a SEPARATE Qdrant collection and is only
/// surfaced when its publish version is active. Two layers enforce this
/// (task-book §13):
///   - The Qdrant query against the web collection carries an `active=true`
///     payload filter, so staged/superseded points are not returned.
///   - The shared MySQL re-validation requires `knowledge_documents.status==1`;
///     publish flips a web document to status=1, supersede/rollback back to 0.
///     Legacy RAG documents have no web manifest and keep status=1, so they are
///     never affected by the web active filter.
pub struct RetrievalService {
    repo: Arc<dyn RAGRepoT>,
    embedding: Option<Arc<dyn EmbeddingProvider>>,
    vector_store: Option<Arc<dyn VectorStoreT>>,
    rag_collection: String,
    hybrid_vector_weight: f64,
    hybrid_keyword_weight: f64,
    /// Optional web-ingestion collection. When set, retrieval also searches it
    /// with an `active=true` payload filter. None → web ingestion not searched
    /// (legacy-only behaviour, unchanged).
    web_collection: Option<String>,
}

impl RetrievalService {
    pub fn new(repo: Arc<dyn RAGRepoT>, embedding: Option<Arc<dyn EmbeddingProvider>>) -> Self {
        Self {
            repo,
            embedding,
            vector_store: None,
            rag_collection: "rag_chunks".into(),
            hybrid_vector_weight: 0.6,
            hybrid_keyword_weight: 0.4,
            web_collection: None,
        }
    }

    pub fn with_vector_store(mut self, vs: Arc<dyn VectorStoreT>, collection: String) -> Self {
        self.vector_store = Some(vs);
        self.rag_collection = collection;
        self
    }

    pub fn with_hybrid_weights(mut self, vector_weight: f64, keyword_weight: f64) -> Self {
        let total = vector_weight + keyword_weight;
        self.hybrid_vector_weight = vector_weight / total;
        self.hybrid_keyword_weight = keyword_weight / total;
        self
    }

    /// Enable retrieval of published web-ingestion content from its own Qdrant
    /// collection (task-book §13). Only points with payload `active=true` are
    /// returned, and they are still MySQL-revalidated.
    pub fn with_web_collection(mut self, collection: String) -> Self {
        self.web_collection = Some(collection);
        self
    }

    /// Retrieve top-k relevant chunks, scoped to `user_id`.
    ///
    /// Qdrant and keyword results are merged when vector search is available.
    /// Either side can fail independently without discarding valid results.
    pub async fn retrieve(
        &self,
        query: &str,
        user_id: u64,
        top_k: u64,
    ) -> Result<Vec<(KnowledgeChunk, f64)>, AppError> {
        if let (Some(vs), Some(ep)) = (&self.vector_store, &self.embedding) {
            let vector_results = self.qdrant_retrieve(vs, ep, query, user_id, top_k).await;
            let keyword_results = self
                .keyword_retrieve_with_validation(query, user_id, top_k)
                .await;

            return match (vector_results, keyword_results) {
                (Ok(vector), Ok(keyword)) => {
                    debug!(
                        vector_count = vector.len(),
                        keyword_count = keyword.len(),
                        "hybrid retrieval succeeded"
                    );
                    Ok(self.hybrid_merge(keyword, vector, top_k))
                }
                (Ok(vector), Err(e)) if !vector.is_empty() => {
                    warn!(error = %e, "keyword retrieval failed; using vector results");
                    Ok(vector)
                }
                (Err(e), Ok(keyword)) => {
                    warn!(error = %e, "Qdrant retrieval failed; using keyword results");
                    Ok(keyword)
                }
                (Ok(_), Err(e)) => Err(e),
                (Err(vector_error), Err(keyword_error)) => Err(AppError::internal(format!(
                    "vector retrieval failed: {vector_error}; keyword retrieval failed: {keyword_error}"
                ))),
            };
        }

        self.keyword_retrieve_with_validation(query, user_id, top_k)
            .await
    }

    // ── Qdrant path ────────────────────────────────────────────────

    async fn qdrant_retrieve(
        &self,
        vs: &Arc<dyn VectorStoreT>,
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

        // Legacy RAG collection — unfiltered (legacy behaviour preserved).
        let mut hits = vs
            .search(
                &self.rag_collection,
                query_vec.clone(),
                VectorFilter::default(),
                top_k as usize,
            )
            .await
            .map_err(|e| AppError::internal(format!("Qdrant search failed: {e}")))?;

        // Web-ingestion collection — ONLY active points (task-book §13). A
        // failure here must not break legacy retrieval, so it is logged and
        // skipped rather than propagated.
        if let Some(web_collection) = &self.web_collection {
            let active_filter = VectorFilter::new().with_condition(VectorCondition::MatchBool {
                key: "active".into(),
                value: true,
            });
            match vs
                .search(web_collection, query_vec, active_filter, top_k as usize)
                .await
            {
                Ok(web_hits) => hits.extend(web_hits),
                Err(e) => warn!(error = %e, "web-ingestion Qdrant search failed; skipping"),
            }
        }

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
                trace!(
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
                        trace!(chunk_id = chunk.chunk_id, doc_id = doc.document_id,
                               visibility = %doc.visibility, "keyword chunk denied by visibility");
                    }
                }
                Ok(None) => {
                    trace!(
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
                    trace!(chunk_id, "chunk not found in MySQL; skipping");
                    continue;
                }
                Err(e) => {
                    warn!(chunk_id, error = %e, "chunk lookup failed; skipping");
                    continue;
                }
            };

            // 2. Chunk status
            if chunk.status != 1 {
                trace!(
                    chunk_id,
                    "chunk disabled (status={}); skipping", chunk.status
                );
                continue;
            }

            // 3. Load document
            let document = match self.repo.find_document_by_id(chunk.document_id).await {
                Ok(Some(d)) => d,
                Ok(None) => {
                    trace!(doc_id = chunk.document_id, "document missing; skipping");
                    continue;
                }
                Err(e) => {
                    warn!(doc_id = chunk.document_id, error = %e, "document lookup failed; skipping");
                    continue;
                }
            };

            // 4. Permission + lifecycle check
            if !can_read_document(&document, user_id) {
                trace!(chunk_id, doc_id = document.document_id,
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
        Ok(self.hybrid_merge(keyword_results, vec_results, top_k))
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
        &self,
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
                (
                    id,
                    self.hybrid_vector_weight * vec + self.hybrid_keyword_weight * kw,
                )
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

    // ── Web‑ingestion active‑filter tests (§16.4, §13) ────────────────
    // These prove that the EXISTING status‑1 gate in can_read_document
    // already excludes staged/superseded/rejected web content. Publish
    // flips knowledge_documents.status 0→1; supersede/rollback 1→0.
    // Legacy docs (source_type≠"web_ingestion") keep status=1 and are
    // never affected.

    #[test]
    fn web_staged_not_retrievable() {
        // §16.4 #1: staged (status=0) web document is invisible.
        let mut doc = make_doc(0, false, "public", None);
        doc.source_type = "web_ingestion".into();
        doc.source_id = Some(42);
        assert!(
            !can_read_document(&doc, 1),
            "staged web doc must not be retrievable"
        );
    }

    #[test]
    fn web_published_retrievable() {
        // §16.4 #4: published (status=1) web document is visible.
        let mut doc = make_doc(1, false, "public", None);
        doc.source_type = "web_ingestion".into();
        doc.source_id = Some(42);
        assert!(
            can_read_document(&doc, 1),
            "published web doc must be retrievable"
        );
    }

    #[test]
    fn legacy_rag_unaffected_by_web_filter() {
        // §16.4 #11: a legacy doc (source_type≠"web_ingestion") with
        // status=1 remains retrievable — no regression.
        let mut doc = make_doc(1, false, "public", None);
        doc.source_type = "psychology_article".into();
        doc.source_id = Some(100);
        assert!(
            can_read_document(&doc, 1),
            "legacy RAG doc must be retrievable"
        );
    }

    #[test]
    fn legacy_disabled_still_filtered() {
        // Legacy docs with status=0 should still be filtered — the
        // status gate is source-type-agnostic.
        let mut doc = make_doc(0, false, "public", None);
        doc.source_type = "psychology_article".into();
        doc.source_id = Some(100);
        assert!(!can_read_document(&doc, 1));
    }

    #[test]
    fn web_superseded_not_retrievable() {
        // §16.4 #3/#6: supersede flips status to 0 → invisible.
        let mut doc = make_doc(0, false, "public", None);
        doc.source_type = "web_ingestion".into();
        doc.source_id = Some(42);
        assert!(
            !can_read_document(&doc, 1),
            "superseded web doc must not be retrievable"
        );
    }
}
