use std::sync::Arc;

use serde_json::json;
use tracing::{debug, warn};

use crate::domain::llm::EmbeddingProvider;
use crate::domain::memory::{ConversationSummary, MemoryRepoT, UserMemory};
use crate::domain::rag::{KnowledgeChunk, KnowledgeDocument, RAGRepoT};
use crate::domain::summary::SummaryRepoT;
use crate::domain::vector_index::{NewVectorIndexJob, NewVectorIndexRecord, VectorIndexRepoT};
use crate::domain::vector_store::{
    VectorCondition, VectorDistance, VectorFilter, VectorPoint, VectorStoreT,
};
use crate::shared::error::AppError;

/// Configuration for the `VectorIndexService`.
#[derive(Debug, Clone)]
pub struct VectorIndexConfig {
    pub rag_collection: String,
    pub memory_collection: String,
    pub summary_collection: String,
    pub distance: VectorDistance,
    pub embedding_provider_name: String,
    pub embedding_model: String,
}

impl Default for VectorIndexConfig {
    fn default() -> Self {
        Self {
            rag_collection: "rag_chunks".into(),
            memory_collection: "user_memories".into(),
            summary_collection: "conversation_summaries".into(),
            distance: VectorDistance::Cosine,
            embedding_provider_name: "ollama".into(),
            embedding_model: "bge-m3".into(),
        }
    }
}

/// Coordinates between business objects and the `VectorStore`.
///
/// Responsibilities:
/// - Generate stable vector IDs.
/// - Build collection payloads.
/// - Call the embedding provider.
/// - Upsert / delete points in the vector store.
/// - Rebuild entire collections from MySQL.
///
/// This service does NOT depend on Qdrant SDK types — only the `VectorStore`
/// trait.  It does NOT make authorization decisions.
pub struct VectorIndexService {
    rag_repo: Arc<dyn RAGRepoT>,
    memory_repo: Arc<dyn MemoryRepoT>,
    #[allow(dead_code)]
    summary_repo: Arc<dyn SummaryRepoT>,
    vector_index_repo: Arc<dyn VectorIndexRepoT>,
    vector_store: Arc<dyn VectorStoreT>,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    config: VectorIndexConfig,
}

impl VectorIndexService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rag_repo: Arc<dyn RAGRepoT>,
        memory_repo: Arc<dyn MemoryRepoT>,
        summary_repo: Arc<dyn SummaryRepoT>,
        vector_index_repo: Arc<dyn VectorIndexRepoT>,
        vector_store: Arc<dyn VectorStoreT>,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        config: VectorIndexConfig,
    ) -> Self {
        Self {
            rag_repo,
            memory_repo,
            summary_repo,
            vector_index_repo,
            vector_store,
            embedding_provider,
            config,
        }
    }

    // ── Collection management ──────────────────────────────────────

    /// Ensure all three collections exist in the vector store.
    ///
    /// Dimension is auto-detected by embedding a small piece of text.
    /// Call this once at startup before indexing or searching.
    pub async fn ensure_collections(&self) -> Result<(), AppError> {
        // Dimension is taken from the embedding provider via a probe
        let dim = self.probe_dimension().await?;

        for (coll, label) in [
            (&self.config.rag_collection, "rag"),
            (&self.config.memory_collection, "memory"),
            (&self.config.summary_collection, "summary"),
        ] {
            self.vector_store
                .ensure_collection(coll, dim, self.config.distance)
                .await
                .map_err(|e| {
                    AppError::internal(format!("failed to ensure {label} collection '{coll}': {e}"))
                })?;
        }
        debug!("all vector collections ensured (dim={dim})");
        Ok(())
    }

    async fn probe_dimension(&self) -> Result<usize, AppError> {
        let vecs = self
            .embedding_provider
            .embed(&["dimension probe".to_string()])
            .await
            .map_err(|e| AppError::internal(format!("embedding probe failed: {e}")))?;

        let dim = vecs
            .first()
            .map(|v| v.len())
            .ok_or_else(|| AppError::Internal("embedding returned empty vector".to_string()))?;

        Ok(dim)
    }

    // ── Chunk indexing ─────────────────────────────────────────────

    pub async fn index_knowledge_chunk(
        &self,
        chunk: &KnowledgeChunk,
        document: Option<&KnowledgeDocument>,
    ) -> Result<String, AppError> {
        let vector_id = chunk_vector_id(chunk.chunk_id);

        let embedding = match self
            .embedding_provider
            .embed(&[chunk.content.clone()])
            .await
        {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("failed to embed chunk {}: {e}", chunk.chunk_id);
                let _ = self
                    .vector_index_repo
                    .mark_failed(&vector_id, msg.clone())
                    .await;
                let _ = self
                    .vector_index_repo
                    .enqueue_job(NewVectorIndexJob {
                        action: "upsert".into(),
                        object_type: "knowledge_chunk".into(),
                        object_id: chunk.chunk_id,
                        collection_name: self.config.rag_collection.clone(),
                        vector_id: Some(vector_id.clone()),
                        priority: 100,
                    })
                    .await;
                return Err(AppError::internal(msg));
            }
        };

        let vector = embedding
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Internal("embedding returned empty vector".to_string()))?;
        let actual_dim = vector.len() as u32;

        let payload = json!({
            "kind": "rag_chunk",
            "vector_id": vector_id,
            "chunk_id": chunk.chunk_id,
            "document_id": chunk.document_id,
            "source_type": document.map(|d| d.source_type.as_str()).unwrap_or("unknown"),
            "source_id": document.and_then(|d| d.source_id),
            "status": document.map(|d| d.status).unwrap_or(1),
            "visibility": document.and_then(|d| Some(d.visibility.as_str())).unwrap_or("public"),
        });

        // 1. Upsert Qdrant
        if let Err(e) = self
            .vector_store
            .upsert_points(
                &self.config.rag_collection,
                vec![VectorPoint {
                    id: vector_id.clone(),
                    vector,
                    payload: payload.clone(),
                }],
            )
            .await
        {
            let msg = format!("Qdrant upsert failed for chunk {}: {e}", chunk.chunk_id);
            let _ = self
                .vector_index_repo
                .mark_failed(&vector_id, msg.clone())
                .await;
            let _ = self
                .vector_index_repo
                .enqueue_job(NewVectorIndexJob {
                    action: "upsert".into(),
                    object_type: "knowledge_chunk".into(),
                    object_id: chunk.chunk_id,
                    collection_name: self.config.rag_collection.clone(),
                    vector_id: Some(vector_id.clone()),
                    priority: 100,
                })
                .await;
            return Err(AppError::internal(msg));
        }

        // 2. Write vector_index_records
        let _ = self
            .vector_index_repo
            .upsert_record(NewVectorIndexRecord {
                vector_id: vector_id.clone(),
                collection_name: self.config.rag_collection.clone(),
                object_type: "knowledge_chunk".into(),
                object_id: chunk.chunk_id,
                owner_user_id: document.and_then(|d| d.owner_user_id),
                source_table: "knowledge_chunks".into(),
                source_hash: None,
                embedding_provider: self.config.embedding_provider_name.clone(),
                embedding_model: self.config.embedding_model.clone(),
                embedding_dimension: actual_dim,
                payload: payload.clone(),
                index_status: "indexed".into(),
            })
            .await;

        // 3. Update business table metadata
        let _ = self
            .rag_repo
            .update_chunk_index_metadata(
                chunk.chunk_id,
                vector_id.clone(),
                self.config.embedding_provider_name.clone(),
                self.config.embedding_model.clone(),
                actual_dim,
            )
            .await;

        debug!(chunk_id = chunk.chunk_id, vector_id = %vector_id, "indexed knowledge chunk");
        Ok(vector_id)
    }

    pub async fn delete_knowledge_chunk_index(&self, chunk_id: u64) -> Result<(), AppError> {
        let id = chunk_vector_id(chunk_id);
        // Delete from Qdrant (ignore errors on missing points)
        let _ = self
            .vector_store
            .delete_points(&self.config.rag_collection, vec![id.clone()])
            .await;
        // Mark deleted in records
        let _ = self.vector_index_repo.mark_deleted(&id).await;
        // Clear business table metadata
        let _ = self.rag_repo.mark_chunk_unindexed(chunk_id).await;
        Ok(())
    }

    pub async fn rebuild_rag_chunks(&self) -> Result<usize, AppError> {
        let pairs = self
            .rag_repo
            .list_indexable_chunks(500)
            .await
            .map_err(|e| AppError::internal(format!("failed to list chunks for rebuild: {e}")))?;

        let mut indexed = 0usize;
        for (chunk, _emb) in &pairs {
            match self.index_knowledge_chunk(chunk, None).await {
                Ok(_) => indexed += 1,
                Err(e) => {
                    warn!(chunk_id = chunk.chunk_id, error = %e, "failed to index chunk during rebuild")
                }
            }
        }

        Ok(indexed)
    }

    // ── Memory indexing ────────────────────────────────────────────

    pub async fn index_memory(&self, memory: &UserMemory) -> Result<String, AppError> {
        let vector_id = memory_vector_id(memory.memory_id);
        let embedding = match self
            .embedding_provider
            .embed(&[memory.content.clone()])
            .await
        {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("embed memory {}: {e}", memory.memory_id);
                let _ = self
                    .vector_index_repo
                    .mark_failed(&vector_id, msg.clone())
                    .await;
                let _ = self
                    .vector_index_repo
                    .enqueue_job(NewVectorIndexJob {
                        action: "upsert".into(),
                        object_type: "user_memory".into(),
                        object_id: memory.memory_id,
                        collection_name: self.config.memory_collection.clone(),
                        vector_id: Some(vector_id.clone()),
                        priority: 100,
                    })
                    .await;
                return Err(AppError::internal(msg));
            }
        };
        let vector = embedding
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Internal("empty vector".into()))?;
        let actual_dim = vector.len() as u32;
        let payload = json!({"kind":"user_memory","vector_id":vector_id,"memory_id":memory.memory_id,"user_id":memory.user_id,"memory_type":memory.memory_type,"status":memory.status,"source_conversation_id":memory.source_conversation_id});
        if let Err(e) = self
            .vector_store
            .upsert_points(
                &self.config.memory_collection,
                vec![VectorPoint {
                    id: vector_id.clone(),
                    vector,
                    payload: payload.clone(),
                }],
            )
            .await
        {
            let msg = format!("Qdrant upsert memory {}: {e}", memory.memory_id);
            let _ = self
                .vector_index_repo
                .mark_failed(&vector_id, msg.clone())
                .await;
            let _ = self
                .vector_index_repo
                .enqueue_job(NewVectorIndexJob {
                    action: "upsert".into(),
                    object_type: "user_memory".into(),
                    object_id: memory.memory_id,
                    collection_name: self.config.memory_collection.clone(),
                    vector_id: Some(vector_id.clone()),
                    priority: 100,
                })
                .await;
            return Err(AppError::internal(msg));
        }
        let _ = self
            .vector_index_repo
            .upsert_record(NewVectorIndexRecord {
                vector_id: vector_id.clone(),
                collection_name: self.config.memory_collection.clone(),
                object_type: "user_memory".into(),
                object_id: memory.memory_id,
                owner_user_id: Some(memory.user_id),
                source_table: "user_memories".into(),
                source_hash: None,
                embedding_provider: self.config.embedding_provider_name.clone(),
                embedding_model: self.config.embedding_model.clone(),
                embedding_dimension: actual_dim,
                payload: payload.clone(),
                index_status: "indexed".into(),
            })
            .await;
        let _ = self
            .memory_repo
            .update_memory_index_metadata(
                memory.memory_id,
                vector_id.clone(),
                self.config.embedding_provider_name.clone(),
                self.config.embedding_model.clone(),
                actual_dim,
            )
            .await;
        debug!(memory_id=memory.memory_id, vector_id=%vector_id, "indexed memory");
        Ok(vector_id)
    }

    pub async fn delete_memory_index(&self, memory_id: u64) -> Result<(), AppError> {
        let id = memory_vector_id(memory_id);
        self.vector_store
            .delete_points(&self.config.memory_collection, vec![id])
            .await
    }

    pub async fn rebuild_user_memories(&self, user_id: Option<u64>) -> Result<usize, AppError> {
        // For simplicity, load all active memories. A full implementation
        // would paginate through all users.
        let mut indexed = 0usize;

        // Load all memories (in production, paginate and filter by user_id)
        let all_ids: Vec<u64> = if let Some(uid) = user_id {
            self.memory_repo
                .find_by_user_id(uid, Some(1))
                .await?
                .into_iter()
                .map(|m| m.memory_id)
                .collect()
        } else {
            // Without a "list all" method, we accept the limitation
            warn!("rebuild_user_memories: user_id=None is not fully supported without pagination");
            return Ok(0);
        };

        for id in &all_ids {
            if let Some(mem) = self.memory_repo.find_by_id(*id).await? {
                match self.index_memory(&mem).await {
                    Ok(_) => indexed += 1,
                    Err(e) => {
                        warn!(memory_id = id, error = %e, "failed to index memory during rebuild")
                    }
                }
            }
        }

        Ok(indexed)
    }

    // ── Summary indexing ───────────────────────────────────────────

    pub async fn index_summary(&self, summary: &ConversationSummary) -> Result<String, AppError> {
        let vector_id = summary_vector_id(summary.summary_id);

        let embedding = self
            .embedding_provider
            .embed(&[summary.content.clone()])
            .await
            .map_err(|e| {
                AppError::internal(format!(
                    "failed to embed summary {}: {e}",
                    summary.summary_id
                ))
            })?;

        let vector = embedding
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Internal("embedding returned empty vector".to_string()))?;

        let payload = json!({
            "kind": "conversation_summary",
            "summary_id": summary.summary_id,
            "conversation_id": summary.conversation_id,
            "user_id": summary.user_id,
            "summary_type": summary.summary_type,
        });

        self.vector_store
            .upsert_points(
                &self.config.summary_collection,
                vec![VectorPoint {
                    id: vector_id.clone(),
                    vector,
                    payload,
                }],
            )
            .await?;

        debug!(summary_id = summary.summary_id, vector_id = %vector_id, "indexed summary");
        Ok(vector_id)
    }

    pub async fn delete_summary_index(&self, summary_id: u64) -> Result<(), AppError> {
        let id = summary_vector_id(summary_id);
        self.vector_store
            .delete_points(&self.config.summary_collection, vec![id])
            .await
    }

    pub async fn enqueue_memory_delete(&self, memory_id: u64) -> Result<(), AppError> {
        self.vector_index_repo
            .enqueue_job(NewVectorIndexJob {
                action: "delete".into(),
                object_type: "memory".into(),
                object_id: memory_id,
                collection_name: self.config.memory_collection.clone(),
                vector_id: Some(memory_vector_id(memory_id)),
                priority: 200,
            })
            .await?;
        Ok(())
    }

    pub async fn enqueue_summary_delete(&self, summary_id: u64) -> Result<(), AppError> {
        self.vector_index_repo
            .enqueue_job(NewVectorIndexJob {
                action: "delete".into(),
                object_type: "summary".into(),
                object_id: summary_id,
                collection_name: self.config.summary_collection.clone(),
                vector_id: Some(summary_vector_id(summary_id)),
                priority: 200,
            })
            .await?;
        Ok(())
    }

    // ── Helpers for the application layer ──────────────────────────

    pub fn rag_collection(&self) -> &str {
        &self.config.rag_collection
    }

    pub fn memory_collection(&self) -> &str {
        &self.config.memory_collection
    }

    pub fn summary_collection(&self) -> &str {
        &self.config.summary_collection
    }

    /// Build a `VectorFilter` that restricts results to a given user_id.
    pub fn user_id_filter(user_id: u64) -> VectorFilter {
        VectorFilter::default().with_condition(VectorCondition::MatchU64 {
            key: "user_id".into(),
            value: user_id,
        })
    }
}

// ── Stable vector IDs ───────────────────────────────────────────────

pub fn chunk_vector_id(chunk_id: u64) -> String {
    format!("rag_chunk:{chunk_id}")
}

pub fn memory_vector_id(memory_id: u64) -> String {
    format!("user_memory:{memory_id}")
}

pub fn summary_vector_id(summary_id: u64) -> String {
    format!("conversation_summary:{summary_id}")
}

// ── Payload field extractors (shared with RetrievalService / MemoryService) ──

pub fn payload_chunk_id(payload: &serde_json::Value) -> Option<u64> {
    payload.get("chunk_id").and_then(|v| v.as_u64())
}

pub fn payload_memory_id(payload: &serde_json::Value) -> Option<u64> {
    payload.get("memory_id").and_then(|v| v.as_u64())
}

pub fn payload_summary_id(payload: &serde_json::Value) -> Option<u64> {
    payload.get("summary_id").and_then(|v| v.as_u64())
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::vector_index::{VectorIndexJob, VectorIndexRecord};
    use crate::infra::llm::mock_provider::MockEmbeddingProvider;
    use crate::infra::vector_store::mock_vector_store::MockVectorStore;

    struct MockRagRepo;
    #[async_trait::async_trait]
    impl RAGRepoT for MockRagRepo {
        async fn save_document(
            &self,
            _d: crate::domain::rag::NewDocument,
        ) -> Result<crate::domain::rag::KnowledgeDocument, AppError> {
            Err(AppError::internal("mock"))
        }
        async fn find_document_by_source(
            &self,
            _: &str,
            _: Option<u64>,
        ) -> Result<Option<crate::domain::rag::KnowledgeDocument>, AppError> {
            Ok(None)
        }
        async fn list_documents_by_source_type(
            &self,
            _: &str,
        ) -> Result<Vec<crate::domain::rag::KnowledgeDocument>, AppError> {
            Ok(vec![])
        }
        async fn save_chunks(
            &self,
            _: &[crate::domain::rag::NewChunk],
        ) -> Result<Vec<crate::domain::rag::KnowledgeChunk>, AppError> {
            Err(AppError::internal("mock"))
        }
        async fn find_chunks_by_document(
            &self,
            _: u64,
        ) -> Result<Vec<crate::domain::rag::KnowledgeChunk>, AppError> {
            Ok(vec![])
        }
        async fn save_embedding(
            &self,
            _: crate::domain::rag::NewEmbedding,
        ) -> Result<crate::domain::rag::KnowledgeEmbedding, AppError> {
            Err(AppError::internal("mock"))
        }
        async fn find_embedding_by_chunk(
            &self,
            _: u64,
        ) -> Result<Option<crate::domain::rag::KnowledgeEmbedding>, AppError> {
            Ok(None)
        }
        async fn search_by_keyword(
            &self,
            _: &str,
            _: u64,
        ) -> Result<Vec<(crate::domain::rag::KnowledgeChunk, f64)>, AppError> {
            Ok(vec![])
        }
        async fn delete_document(&self, _: u64) -> Result<(), AppError> {
            Err(AppError::internal("mock"))
        }
        async fn list_chunks_with_embeddings(
            &self,
        ) -> Result<
            Vec<(
                crate::domain::rag::KnowledgeChunk,
                crate::domain::rag::KnowledgeEmbedding,
            )>,
            AppError,
        > {
            Ok(vec![])
        }

        async fn find_chunk_by_id(
            &self,
            _: u64,
        ) -> Result<Option<crate::domain::rag::KnowledgeChunk>, AppError> {
            Ok(None)
        }

        async fn find_document_by_id(
            &self,
            _: u64,
        ) -> Result<Option<crate::domain::rag::KnowledgeDocument>, AppError> {
            Ok(None)
        }

        async fn update_chunk_index_metadata(
            &self,
            _: u64,
            _: String,
            _: String,
            _: String,
            _: u32,
        ) -> Result<(), AppError> {
            Ok(())
        }
        async fn mark_chunk_unindexed(&self, _: u64) -> Result<(), AppError> {
            Ok(())
        }
        async fn list_indexable_chunks(
            &self,
            _: u64,
        ) -> Result<
            Vec<(
                crate::domain::rag::KnowledgeChunk,
                crate::domain::rag::KnowledgeDocument,
            )>,
            AppError,
        > {
            Ok(vec![])
        }
    }

    struct MockMemRepo;
    #[async_trait::async_trait]
    impl MemoryRepoT for MockMemRepo {
        async fn save_memory_with_evidence(
            &self,
            _: crate::domain::memory::NewMemory,
            _: crate::domain::memory::NewMemoryEvidence,
        ) -> Result<UserMemory, AppError> {
            Err(AppError::internal("mock"))
        }
        async fn reinforce_memory_with_evidence(
            &self,
            _: u64,
            _: crate::domain::memory::NewMemoryEvidence,
            _: f64,
        ) -> Result<UserMemory, AppError> {
            Err(AppError::internal("mock"))
        }
        async fn save_contradicting_memory_with_evidence(
            &self,
            _: crate::domain::memory::NewMemory,
            _: crate::domain::memory::NewMemoryEvidence,
            _: u64,
        ) -> Result<UserMemory, AppError> {
            Err(AppError::internal("mock"))
        }
        async fn find_by_id(&self, _: u64) -> Result<Option<UserMemory>, AppError> {
            Ok(None)
        }
        async fn find_by_user_id(
            &self,
            _: u64,
            _: Option<i8>,
        ) -> Result<Vec<UserMemory>, AppError> {
            Ok(vec![])
        }
        async fn search_by_user(
            &self,
            _: u64,
            _: &str,
            _: u32,
        ) -> Result<Vec<UserMemory>, AppError> {
            Ok(vec![])
        }
        async fn update_memory(
            &self,
            _: u64,
            _: Option<String>,
            _: Option<f64>,
        ) -> Result<UserMemory, AppError> {
            Err(AppError::internal("mock"))
        }
        async fn disable_memory(&self, _: u64) -> Result<(), AppError> {
            Ok(())
        }
        async fn delete_memory(&self, _: u64) -> Result<bool, AppError> {
            Ok(true)
        }
        async fn find_memories_by_conversation(&self, _: u64) -> Result<Vec<UserMemory>, AppError> {
            Ok(vec![])
        }
        async fn update_memory_index_metadata(
            &self,
            _: u64,
            _: String,
            _: String,
            _: String,
            _: u32,
        ) -> Result<(), AppError> {
            Ok(())
        }
        async fn touch_memory_access(&self, _: u64) -> Result<(), AppError> {
            Ok(())
        }
        async fn find_by_memory_key(
            &self,
            _: u64,
            _: &str,
        ) -> Result<Option<UserMemory>, AppError> {
            Ok(None)
        }
        async fn list_indexable_memories(
            &self,
            _: Option<u64>,
            _: u64,
        ) -> Result<Vec<UserMemory>, AppError> {
            Ok(vec![])
        }
    }

    struct MockSumRepo;
    #[async_trait::async_trait]
    impl crate::domain::summary::SummaryRepoT for MockSumRepo {
        async fn find_latest_by_conversation(
            &self,
            _: u64,
        ) -> Result<Option<crate::domain::memory::ConversationSummary>, AppError> {
            Ok(None)
        }
        async fn find_latest_rolling_general(
            &self,
            _: u64,
        ) -> Result<Option<crate::domain::memory::ConversationSummary>, AppError> {
            Ok(None)
        }
        async fn save_summary(
            &self,
            _: crate::domain::memory::NewSummary,
        ) -> Result<crate::domain::memory::ConversationSummary, AppError> {
            Err(AppError::internal("mock"))
        }
        async fn find_by_id(
            &self,
            _: u64,
        ) -> Result<Option<crate::domain::memory::ConversationSummary>, AppError> {
            Ok(None)
        }
        async fn disable_summary(&self, _: u64) -> Result<(), AppError> {
            Ok(())
        }
        async fn list_indexable_summaries(
            &self,
            _: u64,
        ) -> Result<Vec<crate::domain::memory::ConversationSummary>, AppError> {
            Ok(vec![])
        }
        async fn update_summary_index_metadata(
            &self,
            _: u64,
            _: String,
            _: String,
            _: String,
            _: u32,
        ) -> Result<(), AppError> {
            Ok(())
        }
    }

    struct MockIndexRepo;
    #[async_trait::async_trait]
    impl VectorIndexRepoT for MockIndexRepo {
        async fn upsert_record(
            &self,
            r: NewVectorIndexRecord,
        ) -> Result<VectorIndexRecord, AppError> {
            Ok(VectorIndexRecord {
                record_id: 1,
                vector_id: r.vector_id,
                collection_name: r.collection_name,
                object_type: r.object_type,
                object_id: r.object_id,
                owner_user_id: r.owner_user_id,
                source_table: r.source_table,
                source_hash: r.source_hash,
                embedding_provider: r.embedding_provider,
                embedding_model: r.embedding_model,
                embedding_dimension: r.embedding_dimension,
                payload: r.payload,
                index_status: r.index_status,
                indexed_at: None,
                failed_at: None,
                error_message: None,
            })
        }
        async fn mark_indexed(
            &self,
            _: &str,
            _: u32,
            _: serde_json::Value,
        ) -> Result<(), AppError> {
            Ok(())
        }
        async fn mark_failed(&self, _: &str, _: String) -> Result<(), AppError> {
            Ok(())
        }
        async fn mark_deleted(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn find_by_vector_id(&self, _: &str) -> Result<Option<VectorIndexRecord>, AppError> {
            Ok(None)
        }
        async fn list_stale_by_collection(
            &self,
            _: &str,
            _: u64,
        ) -> Result<Vec<VectorIndexRecord>, AppError> {
            Ok(vec![])
        }
        async fn enqueue_job(&self, j: NewVectorIndexJob) -> Result<VectorIndexJob, AppError> {
            Ok(VectorIndexJob {
                job_id: 1,
                action: j.action,
                object_type: j.object_type,
                object_id: j.object_id,
                collection_name: j.collection_name,
                vector_id: j.vector_id,
                priority: j.priority,
                status: "pending".into(),
                attempts: 0,
                max_attempts: 5,
                next_run_at: chrono::Utc::now(),
            })
        }
        async fn fetch_pending_jobs(
            &self,
            _: u64,
            _: &str,
        ) -> Result<Vec<VectorIndexJob>, AppError> {
            Ok(vec![])
        }
        async fn mark_job_succeeded(&self, _: u64) -> Result<(), AppError> {
            Ok(())
        }
        async fn mark_job_failed(&self, _: u64, _: String, _: bool) -> Result<(), AppError> {
            Ok(())
        }
    }

    fn make_service() -> (
        VectorIndexService,
        Arc<MockVectorStore>,
        Arc<MockEmbeddingProvider>,
    ) {
        let vs = Arc::new(MockVectorStore::new());
        let ep = Arc::new(MockEmbeddingProvider::new(384));
        let idx = Arc::new(MockIndexRepo);
        let svc = VectorIndexService::new(
            Arc::new(MockRagRepo),
            Arc::new(MockMemRepo),
            Arc::new(MockSumRepo),
            idx,
            vs.clone(),
            ep.clone(),
            VectorIndexConfig::default(),
        );
        (svc, vs, ep)
    }

    #[tokio::test]
    async fn test_index_knowledge_chunk() {
        let (svc, vs, _ep) = make_service();
        // Ensure collection first
        vs.ensure_collection("rag_chunks", 384, VectorDistance::Cosine)
            .await
            .unwrap();

        let chunk = KnowledgeChunk {
            chunk_id: 42,
            document_id: 1,
            chunk_index: 0,
            content: "test content for indexing".into(),
            token_count: None,
            metadata: None,
            status: 1,
            created_at: chrono::Utc::now(),
        };

        let vid = svc.index_knowledge_chunk(&chunk, None).await.unwrap();
        assert_eq!(vid, "rag_chunk:42");

        // Verify it's searchable
        let hits = vs
            .search("rag_chunks", vec![0.0; 384], VectorFilter::default(), 5)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(payload_chunk_id(&hits[0].payload), Some(42));
    }

    #[tokio::test]
    async fn test_index_memory_payload_contains_user_id() {
        let (svc, vs, _ep) = make_service();
        vs.ensure_collection("user_memories", 384, VectorDistance::Cosine)
            .await
            .unwrap();

        let mem = UserMemory {
            memory_id: 7,
            user_id: 99,
            memory_type: "preference".into(),
            content: "likes coffee".into(),
            confidence: 0.9,
            reinforce_count: 0,
            reinforced_at: None,
            source_conversation_id: None,
            source_message_id: None,
            status: 1,
            metadata: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let vid = svc.index_memory(&mem).await.unwrap();
        assert_eq!(vid, "user_memory:7");

        let filter = VectorIndexService::user_id_filter(99);
        let hits = vs
            .search("user_memories", vec![0.0; 384], filter, 5)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(payload_memory_id(&hits[0].payload), Some(7));

        // Filter for a different user should return empty
        let filter2 = VectorIndexService::user_id_filter(100);
        let hits2 = vs
            .search("user_memories", vec![0.0; 384], filter2, 5)
            .await
            .unwrap();
        assert!(hits2.is_empty());
    }

    #[tokio::test]
    async fn test_embedding_failure_returns_error() {
        // Create an embedding provider that always returns an error
        struct FailingEmbedding;
        #[async_trait::async_trait]
        impl EmbeddingProvider for FailingEmbedding {
            async fn embed(
                &self,
                _: &[String],
            ) -> Result<Vec<Vec<f32>>, crate::domain::llm::LlmError> {
                Err(crate::domain::llm::LlmError::EmbeddingError(
                    "simulated failure".into(),
                ))
            }
        }

        let vs = Arc::new(MockVectorStore::new());
        let ep = Arc::new(FailingEmbedding);
        let idx = Arc::new(MockIndexRepo);
        let svc = VectorIndexService::new(
            Arc::new(MockRagRepo),
            Arc::new(MockMemRepo),
            Arc::new(MockSumRepo),
            idx,
            vs.clone(),
            ep,
            VectorIndexConfig::default(),
        );

        vs.ensure_collection("rag_chunks", 384, VectorDistance::Cosine)
            .await
            .unwrap();

        let chunk = KnowledgeChunk {
            chunk_id: 1,
            document_id: 1,
            chunk_index: 0,
            content: "test".into(),
            token_count: None,
            metadata: None,
            status: 1,
            created_at: chrono::Utc::now(),
        };

        let result = svc.index_knowledge_chunk(&chunk, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_vector_id_stable() {
        assert_eq!(chunk_vector_id(123), "rag_chunk:123");
        assert_eq!(memory_vector_id(456), "user_memory:456");
        assert_eq!(summary_vector_id(789), "conversation_summary:789");
    }
}
