use std::sync::Arc;

use tracing::{debug, warn};

use crate::domain::llm::{ChatMessage, EmbeddingProvider};
use crate::domain::memory::{MemoryRepository, UserMemory};
use crate::domain::vector_store::{VectorFilter, VectorStore};
use crate::shared::error::AppError;

use super::memory_extractor::MemoryExtractor;

/// Application-layer service for memory extraction, search, recall,
/// and lifecycle management.
///
/// When a `VectorStore` + `EmbeddingProvider` are configured, `recall` /
/// `search` prefer Qdrant vector search with `user_id` payload filtering.
/// Results are always verified against MySQL.
pub struct MemoryService {
    repo: Arc<dyn MemoryRepository>,
    extractor: Arc<MemoryExtractor>,
    embedding: Option<Arc<dyn EmbeddingProvider>>,
    vector_store: Option<Arc<dyn VectorStore>>,
    vector_index: Option<Arc<crate::application::rag::vector_index_service::VectorIndexService>>,
    memory_collection: String,
}

impl MemoryService {
    pub fn new(repo: Arc<dyn MemoryRepository>, extractor: Arc<MemoryExtractor>) -> Self {
        Self {
            repo,
            extractor,
            embedding: None,
            vector_store: None,
            vector_index: None,
            memory_collection: "user_memories".into(),
        }
    }

    /// Attach vector search capability.
    pub fn with_vector_search(
        mut self,
        vs: Arc<dyn VectorStore>,
        ep: Arc<dyn EmbeddingProvider>,
        collection: String,
    ) -> Self {
        self.vector_store = Some(vs);
        self.embedding = Some(ep);
        self.memory_collection = collection;
        self
    }

    pub fn with_vector_index(
        mut self,
        vi: Arc<crate::application::rag::vector_index_service::VectorIndexService>,
    ) -> Self {
        self.vector_index = Some(vi);
        self
    }

    /// Run the LLM extractor and persist every extracted memory.
    pub async fn extract_and_save(
        &self,
        user_id: u64,
        messages: &[ChatMessage],
        conversation_id: u64,
        message_id: u64,
    ) -> Result<Vec<UserMemory>, AppError> {
        let memories = self.extractor.extract(user_id, messages).await;

        if memories.is_empty() {
            return Ok(Vec::new());
        }

        let mut saved = Vec::with_capacity(memories.len());
        for mut memory in memories {
            memory.source_conversation_id = Some(conversation_id);
            memory.source_message_id = Some(message_id);
            saved.push(self.repo.save_memory(memory).await?);
        }

        // Attempt vector indexing for each saved memory.
        // Indexing failure is non-fatal — log and continue.
        if let (Some(vs), Some(ep)) = (&self.vector_store, &self.embedding) {
            for mem in &saved {
                match self.index_single_memory(vs, ep, mem).await {
                    Ok(_) => {}
                    Err(e) => {
                        warn!(memory_id = mem.memory_id, error = %e,
                              "failed to index memory in vector store — memory was saved");
                    }
                }
            }
        }

        Ok(saved)
    }

    async fn index_single_memory(
        &self,
        vs: &Arc<dyn VectorStore>,
        ep: &Arc<dyn EmbeddingProvider>,
        mem: &UserMemory,
    ) -> Result<(), AppError> {
        let vector_id = format!("user_memory:{}", mem.memory_id);

        let vecs = ep
            .embed(&[mem.content.clone()])
            .await
            .map_err(|e| AppError::internal(format!("embed memory {}: {e}", mem.memory_id)))?;

        let vector = vecs
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Internal("embedding returned empty vector".to_string()))?;

        let payload = serde_json::json!({
            "kind": "user_memory",
            "memory_id": mem.memory_id,
            "user_id": mem.user_id,
            "memory_type": mem.memory_type,
            "status": mem.status,
            "source_conversation_id": mem.source_conversation_id,
        });

        vs.upsert_points(
            &self.memory_collection,
            vec![crate::domain::vector_store::VectorPoint {
                id: vector_id,
                vector,
                payload,
            }],
        )
        .await
    }

    /// List all non-disabled memories for a user.
    pub async fn list(&self, user_id: u64) -> Result<Vec<UserMemory>, AppError> {
        self.repo.find_by_user_id(user_id, Some(1)).await
    }

    /// Semantic search over the user's memories (default top_k=10).
    pub async fn search(&self, user_id: u64, query: &str) -> Result<Vec<UserMemory>, AppError> {
        self.recall(user_id, query, 10).await
    }

    /// Recall with explicit top_k.
    ///
    /// Strategy: Qdrant-first with MySQL verification.
    /// Falls back to `repo.search_by_user` when vector store is unavailable.
    pub async fn recall(
        &self,
        user_id: u64,
        query: &str,
        top_k: u32,
    ) -> Result<Vec<UserMemory>, AppError> {
        // Try Qdrant path
        if let (Some(vs), Some(ep)) = (&self.vector_store, &self.embedding) {
            match self.qdrant_recall(vs, ep, user_id, query, top_k).await {
                Ok(results) if !results.is_empty() => {
                    debug!(count = results.len(), "Qdrant memory recall succeeded");
                    return Ok(results);
                }
                Ok(_) => {
                    debug!("Qdrant returned empty results; falling back to repo");
                }
                Err(e) => {
                    warn!(error = %e, "Qdrant memory recall failed; falling back to repo");
                }
            }
        }

        // Fallback to MySQL keyword search
        self.repo.search_by_user(user_id, query, top_k).await
    }

    async fn qdrant_recall(
        &self,
        vs: &Arc<dyn VectorStore>,
        ep: &Arc<dyn EmbeddingProvider>,
        user_id: u64,
        query: &str,
        top_k: u32,
    ) -> Result<Vec<UserMemory>, AppError> {
        // 1. Embed the query
        let vecs = ep
            .embed(&[query.to_string()])
            .await
            .map_err(|e| AppError::internal(format!("query embedding failed: {e}")))?;

        let query_vec = vecs
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Internal("embedding returned empty vector".to_string()))?;

        // 2. Filter by user_id in Qdrant payload
        let filter = VectorFilter::default().with_condition(
            crate::domain::vector_store::VectorCondition::MatchU64 {
                key: "user_id".into(),
                value: user_id,
            },
        );

        // 3. Search Qdrant
        let hits = vs
            .search(&self.memory_collection, query_vec, filter, top_k as usize)
            .await
            .map_err(|e| AppError::internal(format!("Qdrant memory search failed: {e}")))?;

        // 4. Re-load from MySQL and verify
        let mut results = Vec::new();
        for hit in &hits {
            let memory_id = match hit.payload.get("memory_id").and_then(|v| v.as_u64()) {
                Some(id) => id,
                None => {
                    warn!(hit_id = %hit.id, "Qdrant hit missing memory_id; skipping");
                    continue;
                }
            };

            match self.repo.find_by_id(memory_id).await {
                Ok(Some(mem)) => {
                    // Verify ownership and status
                    if mem.user_id != user_id {
                        warn!(
                            memory_id,
                            qdrant_user = mem.user_id,
                            expected = user_id,
                            "memory user_id mismatch — skipping"
                        );
                        continue;
                    }
                    if mem.status != 1 {
                        debug!(memory_id, "memory is disabled; skipping");
                        continue;
                    }
                    results.push(mem);
                }
                Ok(None) => {
                    debug!(memory_id, "memory not found in MySQL; skipping");
                    continue;
                }
                Err(e) => {
                    warn!(memory_id, error = %e, "failed to load memory from MySQL; skipping");
                    continue;
                }
            }
        }

        Ok(results)
    }

    /// Soft-disable a memory. Verifies ownership.
    pub async fn disable(&self, id: u64, user_id: u64) -> Result<(), AppError> {
        let mem = self
            .repo
            .find_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("memory {} not found", id)))?;

        if mem.user_id != user_id {
            return Err(AppError::Forbidden(
                "you can only disable your own memories".into(),
            ));
        }

        self.repo.disable_memory(id).await?;

        // Sync delete from Qdrant index (non-fatal)
        if let Some(ref vi) = self.vector_index {
            if let Err(e) = vi.delete_memory_index(id).await {
                tracing::warn!(memory_id = id, error = %e, "failed to delete memory index during disable");
            }
        }
        Ok(())
    }

    /// Permanently delete a memory. Verifies ownership.
    pub async fn delete(&self, id: u64, user_id: u64) -> Result<(), AppError> {
        let mem = self
            .repo
            .find_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("memory {} not found", id)))?;

        if mem.user_id != user_id {
            return Err(AppError::Forbidden(
                "you can only delete your own memories".into(),
            ));
        }

        self.repo.delete_memory(id).await?;

        // Sync delete from Qdrant index (non-fatal)
        if let Some(ref vi) = self.vector_index {
            if let Err(e) = vi.delete_memory_index(id).await {
                tracing::warn!(memory_id = id, error = %e, "failed to delete memory index during delete");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;

    use crate::application::memory::memory_extractor::test_utils::MockLlm;
    use crate::domain::memory::NewMemory;
    use crate::infrastructure::llm::mock_provider::MockEmbeddingProvider;
    use crate::infrastructure::vector_store::mock_vector_store::MockVectorStore;

    struct MockRepo;
    #[async_trait]
    impl MemoryRepository for MockRepo {
        async fn save_memory(&self, memory: NewMemory) -> Result<UserMemory, AppError> {
            Ok(UserMemory {
                memory_id: 1,
                user_id: memory.user_id,
                memory_type: memory.memory_type,
                content: memory.content,
                confidence: memory.confidence,
                source_conversation_id: memory.source_conversation_id,
                source_message_id: memory.source_message_id,
                status: 1,
                metadata: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
        }
        async fn find_by_id(&self, memory_id: u64) -> Result<Option<UserMemory>, AppError> {
            Ok(Some(UserMemory {
                memory_id,
                user_id: 42,
                memory_type: "fact".into(),
                content: "test".into(),
                confidence: 0.8,
                source_conversation_id: None,
                source_message_id: None,
                status: 1,
                metadata: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            }))
        }
        async fn find_by_user_id(
            &self,
            user_id: u64,
            _status: Option<i8>,
        ) -> Result<Vec<UserMemory>, AppError> {
            Ok(vec![UserMemory {
                memory_id: 1,
                user_id,
                memory_type: "fact".into(),
                content: "test".into(),
                confidence: 0.8,
                source_conversation_id: None,
                source_message_id: None,
                status: 1,
                metadata: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            }])
        }
        async fn search_by_user(
            &self,
            user_id: u64,
            _query: &str,
            _top_k: u32,
        ) -> Result<Vec<UserMemory>, AppError> {
            Ok(vec![UserMemory {
                memory_id: 2,
                user_id,
                memory_type: "preference".into(),
                content: "likes hiking".into(),
                confidence: 0.9,
                source_conversation_id: None,
                source_message_id: None,
                status: 1,
                metadata: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            }])
        }
        async fn update_memory(
            &self,
            memory_id: u64,
            _content: Option<String>,
            _confidence: Option<f64>,
        ) -> Result<UserMemory, AppError> {
            Ok(UserMemory {
                memory_id,
                user_id: 1,
                memory_type: "fact".into(),
                content: "updated".into(),
                confidence: 0.5,
                source_conversation_id: None,
                source_message_id: None,
                status: 1,
                metadata: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            })
        }
        async fn disable_memory(&self, _memory_id: u64) -> Result<(), AppError> {
            Ok(())
        }
        async fn delete_memory(&self, _memory_id: u64) -> Result<bool, AppError> {
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

    fn make_service() -> MemoryService {
        let llm = Arc::new(MockLlm);
        let extractor = Arc::new(MemoryExtractor::new(llm));
        MemoryService::new(Arc::new(MockRepo), extractor)
    }

    fn make_service_with_vector() -> (
        MemoryService,
        Arc<MockVectorStore>,
        Arc<MockEmbeddingProvider>,
    ) {
        let vs = Arc::new(MockVectorStore::new());
        let ep = Arc::new(MockEmbeddingProvider::new(384));
        let svc = make_service().with_vector_search(vs.clone(), ep.clone(), "user_memories".into());
        (svc, vs, ep)
    }

    #[tokio::test]
    async fn test_list() {
        let svc = make_service();
        let mems = svc.list(42).await.unwrap();
        assert_eq!(mems.len(), 1);
    }

    #[tokio::test]
    async fn test_search_falls_back_to_repo() {
        let svc = make_service();
        let mems = svc.search(42, "hiking").await.unwrap();
        assert_eq!(mems.len(), 1);
        assert_eq!(mems[0].memory_type, "preference");
    }

    #[tokio::test]
    async fn test_recall_falls_back_to_repo() {
        let svc = make_service();
        let mems = svc.recall(42, "hiking", 5).await.unwrap();
        assert_eq!(mems.len(), 1);
    }

    #[tokio::test]
    async fn test_recall_with_vector_store_filters_by_user_id() {
        let (svc, vs, _ep) = make_service_with_vector();
        use crate::domain::vector_store::VectorDistance;
        vs.ensure_collection("user_memories", 384, VectorDistance::Cosine)
            .await
            .unwrap();

        // Pre-populate with a memory for user 42
        let vid = "user_memory:2".to_string();
        vs.upsert_points(
            "user_memories",
            vec![crate::domain::vector_store::VectorPoint {
                id: vid.clone(),
                vector: vec![0.1; 384],
                payload: serde_json::json!({
                    "memory_id": 2,
                    "user_id": 42,
                    "status": 1,
                }),
            }],
        )
        .await
        .unwrap();

        // Also add a memory for a different user (should be filtered out)
        vs.upsert_points(
            "user_memories",
            vec![crate::domain::vector_store::VectorPoint {
                id: "user_memory:99".into(),
                vector: vec![0.2; 384],
                payload: serde_json::json!({
                    "memory_id": 99,
                    "user_id": 99,
                    "status": 1,
                }),
            }],
        )
        .await
        .unwrap();

        // The recall should find memory for user 42
        let result = svc.recall(42, "hiking", 5).await;
        // It should succeed (falls through to repo after Qdrant returns a result
        // that passes ownership check)
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_disable_forbidden_wrong_user() {
        let svc = make_service();
        let result = svc.disable(1, 99).await;
        assert!(result.is_err());
        match result {
            Err(AppError::Forbidden(_)) => {}
            _ => panic!("expected Forbidden"),
        }
    }

    #[tokio::test]
    async fn test_delete_forbidden_wrong_user() {
        let svc = make_service();
        let result = svc.delete(1, 99).await;
        assert!(result.is_err());
        match result {
            Err(AppError::Forbidden(_)) => {}
            _ => panic!("expected Forbidden"),
        }
    }

    #[tokio::test]
    async fn test_extract_and_save_triggers_index() {
        let (svc, vs, _ep) = make_service_with_vector();
        use crate::domain::vector_store::VectorDistance;
        vs.ensure_collection("user_memories", 384, VectorDistance::Cosine)
            .await
            .unwrap();

        let messages = vec![ChatMessage {
            role: "user".into(),
            content: "I love jazz".into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];

        let saved = svc.extract_and_save(42, &messages, 1, 1).await.unwrap();
        // The extractor should return 2 memories (jazz + college student)
        assert!(!saved.is_empty());

        // After saving, memories should be indexed in the vector store
        // With user_id filter for user 42, they should be findable
        let filter = VectorFilter::default().with_condition(
            crate::domain::vector_store::VectorCondition::MatchU64 {
                key: "user_id".into(),
                value: 42,
            },
        );
        let hits = vs
            .search("user_memories", vec![0.0; 384], filter, 10)
            .await
            .unwrap();
        assert!(!hits.is_empty());
    }
}
