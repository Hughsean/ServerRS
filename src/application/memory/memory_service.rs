use std::sync::Arc;

use crate::domain::llm::ChatMessage;
use crate::domain::memory::{MemoryRepository, UserMemory};
use crate::shared::error::AppError;

use super::memory_extractor::MemoryExtractor;

/// Application-layer service that coordinates memory extraction from
/// conversation messages and delegates persistence to the repository.
pub struct MemoryService {
    repo: Arc<dyn MemoryRepository>,
    extractor: Arc<MemoryExtractor>,
}

impl MemoryService {
    pub fn new(repo: Arc<dyn MemoryRepository>, extractor: Arc<MemoryExtractor>) -> Self {
        Self { repo, extractor }
    }

    /// Run the LLM extractor on the given messages and persist every
    /// extracted memory.  Sets `source_conversation_id` and
    /// `source_message_id` on each memory before saving.
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

        Ok(saved)
    }

    /// List all non-disabled memories for a user.
    pub async fn list(&self, user_id: u64) -> Result<Vec<UserMemory>, AppError> {
        self.repo.find_by_user_id(user_id, Some(1)).await
    }

    /// Semantic search over the user's memories.
    pub async fn search(
        &self,
        user_id: u64,
        query: &str,
    ) -> Result<Vec<UserMemory>, AppError> {
        // Use a default top_k of 10 when no specific limit is given.
        self.repo.search_by_user(user_id, query, 10).await
    }

    /// Recall (semantic search) with an explicit top_k.
    pub async fn recall(
        &self,
        user_id: u64,
        query: &str,
        top_k: u32,
    ) -> Result<Vec<UserMemory>, AppError> {
        self.repo.search_by_user(user_id, query, top_k).await
    }

    /// Soft-disable (logical delete) a memory.  Verifies the memory
    /// belongs to the requesting user before disabling.
    pub async fn disable(
        &self,
        id: u64,
        user_id: u64,
    ) -> Result<(), AppError> {
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

        self.repo.disable_memory(id).await
    }

    /// Permanently delete a memory.  Verifies ownership first.
    pub async fn delete(
        &self,
        id: u64,
        user_id: u64,
    ) -> Result<(), AppError> {
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
            _memory_id: u64,
            _content: Option<String>,
            _confidence: Option<f64>,
        ) -> Result<UserMemory, AppError> {
            unimplemented!()
        }
        async fn disable_memory(&self, _memory_id: u64) -> Result<(), AppError> {
            Ok(())
        }
        async fn delete_memory(&self, _memory_id: u64) -> Result<bool, AppError> {
            Ok(true)
        }
        async fn find_memories_by_conversation(
            &self,
            _conversation_id: u64,
        ) -> Result<Vec<UserMemory>, AppError> {
            Ok(vec![])
        }
    }

    fn make_service() -> MemoryService {
        let llm = Arc::new(MockLlm);
        let extractor = Arc::new(MemoryExtractor::new(llm));
        MemoryService::new(Arc::new(MockRepo), extractor)
    }

    #[tokio::test]
    async fn test_list() {
        let svc = make_service();
        let mems = svc.list(42).await.unwrap();
        assert_eq!(mems.len(), 1);
    }

    #[tokio::test]
    async fn test_search() {
        let svc = make_service();
        let mems = svc.search(42, "hiking").await.unwrap();
        assert_eq!(mems.len(), 1);
        assert_eq!(mems[0].memory_type, "preference");
    }

    #[tokio::test]
    async fn test_recall() {
        let svc = make_service();
        let mems = svc.recall(42, "hiking", 5).await.unwrap();
        assert_eq!(mems.len(), 1);
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
}
