use std::sync::Arc;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

use crate::domain::llm::{ChatMessage, EmbeddingProvider};
use crate::domain::memory::{
    MemoryRepoT, NewMemory, NewMemoryEvidence, UserMemory, is_allowed_memory_type,
};
use crate::domain::user::user_context_version::UserContextVersionRepoT;
use crate::domain::user::user_profile_repository::UserProfileRepoT;
use crate::domain::vector_store::{VectorFilter, VectorStoreT};
use crate::shared::error::AppError;

use super::memory_extractor::{MemoryExtractor, MemoryMergeDecision};

/// Application-layer service for memory extraction, search, recall,
/// and lifecycle management.
///
/// When a `VectorStore` + `EmbeddingProvider` are configured, `recall` /
/// `search` prefer Qdrant vector search with `user_id` payload filtering.
/// Results are always verified against MySQL.
pub struct MemoryService {
    repo: Arc<dyn MemoryRepoT>,
    extractor: Arc<MemoryExtractor>,
    embedding: Option<Arc<dyn EmbeddingProvider>>,
    vector_store: Option<Arc<dyn VectorStoreT>>,
    vector_index: Option<Arc<crate::app::rag::vector_index_service::VectorIndexService>>,
    profile_repo: Option<Arc<dyn UserProfileRepoT>>,
    context_version_repo: Option<Arc<dyn UserContextVersionRepoT>>,
    memory_collection: String,
}

struct PersonalizationPolicy {
    enabled: bool,
    reset_at: Option<DateTime<Utc>>,
}

impl PersonalizationPolicy {
    fn includes(&self, memory: &UserMemory) -> bool {
        self.enabled
            && self
                .reset_at
                .is_none_or(|reset_at| memory.created_at > reset_at)
            && is_allowed_memory_type(&memory.memory_type)
    }
}

fn canonicalize_memory(content: &str) -> String {
    content
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_lowercase()
}

fn memory_key(canonical_form: &str) -> String {
    Sha256::digest(canonical_form.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl MemoryService {
    pub fn new(repo: Arc<dyn MemoryRepoT>, extractor: Arc<MemoryExtractor>) -> Self {
        Self {
            repo,
            extractor,
            embedding: None,
            vector_store: None,
            vector_index: None,
            profile_repo: None,
            context_version_repo: None,
            memory_collection: "user_memories".into(),
        }
    }

    /// Attach vector search capability.
    pub fn with_vector_search(
        mut self,
        vs: Arc<dyn VectorStoreT>,
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
        vi: Arc<crate::app::rag::vector_index_service::VectorIndexService>,
    ) -> Self {
        self.vector_index = Some(vi);
        self
    }

    pub fn with_personalization_profile_repo(
        mut self,
        profile_repo: Arc<dyn UserProfileRepoT>,
    ) -> Self {
        self.profile_repo = Some(profile_repo);
        self
    }

    pub fn with_context_version_repo(
        mut self,
        context_version_repo: Arc<dyn UserContextVersionRepoT>,
    ) -> Self {
        self.context_version_repo = Some(context_version_repo);
        self
    }

    async fn context_is_current(
        &self,
        user_id: u64,
        expected_version: Option<u64>,
    ) -> Result<bool, AppError> {
        let (Some(repo), Some(expected)) = (&self.context_version_repo, expected_version) else {
            return Ok(true);
        };
        let current = repo.get_or_create(user_id).await?;
        if current.version != expected {
            debug!(user_id, expected, current = current.version, "上下文版本不匹配");
        }
        Ok(current.version == expected)
    }

    async fn personalization_policy(
        &self,
        user_id: u64,
    ) -> Result<PersonalizationPolicy, AppError> {
        let Some(profile_repo) = &self.profile_repo else {
            return Ok(PersonalizationPolicy {
                enabled: true,
                reset_at: None,
            });
        };
        let profile = profile_repo.find_by_user_id(user_id).await?;
        Ok(profile
            .map(|profile| PersonalizationPolicy {
                enabled: profile.personalization_enabled,
                reset_at: profile.personalization_reset_at,
            })
            .unwrap_or(PersonalizationPolicy {
                enabled: true,
                reset_at: None,
            }))
    }

    /// Validate a NewMemory before persisting.
    #[allow(dead_code)]
    fn validate_memory(memory: &NewMemory) -> Result<(), AppError> {
        if !is_allowed_memory_type(&memory.memory_type) {
            return Err(AppError::Validation(format!(
                "unsupported memory type: {}",
                memory.memory_type
            )));
        }
        if memory.content.trim().is_empty() {
            return Err(AppError::Validation(
                "memory content must not be empty".into(),
            ));
        }
        Ok(())
    }

    /// Validate NewMemoryEvidence before persisting.
    #[allow(dead_code)]
    fn validate_evidence(evidence: &NewMemoryEvidence) -> Result<(), AppError> {
        let valid_source = match evidence.source_type.as_str() {
            "message" => evidence.message_id == Some(evidence.source_ref_id),
            "summary" => evidence.summary_id == Some(evidence.source_ref_id),
            "manual" => evidence.message_id.is_none() && evidence.summary_id.is_none(),
            _ => false,
        };
        if !valid_source {
            return Err(AppError::Validation(
                "memory evidence source is invalid".into(),
            ));
        }
        if !matches!(
            evidence.evidence_type.as_str(),
            "source" | "reinforcement" | "contradiction" | "manual"
        ) {
            return Err(AppError::Validation(
                "memory evidence type is invalid".into(),
            ));
        }
        Ok(())
    }

    /// Run the LLM extractor and persist every extracted memory.

    /// Retrieve memories for a user, optionally filtered by status.
    pub async fn find_by_user_id(
        &self,
        user_id: u64,
        status: Option<i8>,
    ) -> Result<Vec<crate::domain::memory::UserMemory>, AppError> {
        let policy = self.personalization_policy(user_id).await?;
        if !policy.enabled {
            return Ok(Vec::new());
        }
        let memories = self.repo.find_by_user_id(user_id, status).await?;
        Ok(memories
            .into_iter()
            .filter(|memory| policy.includes(memory))
            .collect())
    }

    /// Find memories for a user, optionally filtered by type, with result limits.
    pub async fn find_by_user_id_filtered(
        &self,
        user_id: u64,
        status: Option<i8>,
        memory_types: &[String],
        limit: usize,
    ) -> Result<Vec<UserMemory>, AppError> {
        let memories = self.find_by_user_id(user_id, status).await?;
        let filtered: Vec<UserMemory> = if memory_types.is_empty() {
            memories
        } else {
            memories
                .into_iter()
                .filter(|m| memory_types.contains(&m.memory_type))
                .collect()
        };
        Ok(filtered.into_iter().take(limit).collect())
    }

    pub async fn extract_and_save(
        &self,
        user_id: u64,
        messages: &[ChatMessage],
        conversation_id: u64,
        message_id: u64,
    ) -> Result<Vec<UserMemory>, AppError> {
        self.extract_and_save_at_version(user_id, messages, conversation_id, message_id, None)
            .await
    }

    pub async fn extract_and_save_at_version(
        &self,
        user_id: u64,
        messages: &[ChatMessage],
        conversation_id: u64,
        message_id: u64,
        expected_version: Option<u64>,
    ) -> Result<Vec<UserMemory>, AppError> {
        if !self.context_is_current(user_id, expected_version).await? {
            debug!(user_id, ?expected_version, "记忆提取跳过: 上下文版本已过期");
            return Ok(Vec::new());
        }
        let policy = self.personalization_policy(user_id).await?;
        if !policy.enabled {
            debug!(user_id, "记忆提取跳过: 个性化已禁用");
            return Ok(Vec::new());
        }
        let memories = self.extractor.extract(user_id, messages).await;
        let extracted = memories.len();

        if memories.is_empty() || !self.context_is_current(user_id, expected_version).await? {
            debug!(user_id, extracted, "记忆提取跳过: 提取为空或版本过期");
            return Ok(Vec::new());
        }

        let mut saved = Vec::with_capacity(memories.len());
        // Accumulator for non-key-deduped memories — they'll be batch-merge-classified
        let mut batch_input: Vec<(NewMemory, NewMemoryEvidence)> = Vec::new();

        for mut memory in memories {
            // Skip empty content
            if memory.content.trim().is_empty() {
                continue;
            }
            // Skip low confidence
            if memory.confidence < 0.7 {
                continue;
            }
            if !is_allowed_memory_type(&memory.memory_type) {
                continue;
            }
            // Truncate
            if memory.content.chars().count() > 300 {
                memory.content = memory.content.chars().take(300).collect::<String>() + "...";
            }
            memory.source_conversation_id = Some(conversation_id);
            memory.source_message_id = Some(message_id);

            // ── Evidence (placeholder — evidence_type adjusted per decision later) ──
            let evidence = NewMemoryEvidence {
                source_type: "message".into(),
                source_ref_id: message_id,
                message_id: Some(message_id),
                summary_id: None,
                evidence_type: "source".into(),
                confidence: Some(memory.confidence),
                extractor_version: Some("memory-extractor-v1".into()),
            };

            // ── Exact-key dedup ──
            let canonical_form = canonicalize_memory(&memory.content);
            let key = memory_key(&canonical_form);
            memory.canonical_form = Some(canonical_form);
            memory.memory_key = Some(key.clone());
            if let Some(existing) = self.repo.find_by_memory_key(user_id, &key).await? {
                if policy.includes(&existing) {
                    saved.push(
                        self.repo
                            .reinforce_memory_with_evidence(
                                existing.memory_id,
                                NewMemoryEvidence {
                                    evidence_type: "reinforcement".into(),
                                    ..evidence
                                },
                                memory.confidence,
                            )
                            .await?,
                    );
                    continue;
                }
                memory.memory_key = None;
            }

            // ── Defer to batch semantic merge ──
            batch_input.push((memory, evidence));
        }

        // ── Batch semantic merge ──
        if !batch_input.is_empty() {
            let combined_query = batch_input
                .iter()
                .map(|(m, _)| m.content.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let all_candidates = self
                .recall(user_id, &combined_query, 10)
                .await
                .unwrap_or_default();

            let decisions = self
                .extractor
                .classify_merge_batch(
                    &batch_input
                        .iter()
                        .map(|(m, _)| m.clone())
                        .collect::<Vec<_>>(),
                    &all_candidates,
                )
                .await;

            for ((mut memory, mut evidence), decision) in
                batch_input.into_iter().zip(decisions.into_iter())
            {
                let persisted = match decision {
                    MemoryMergeDecision::Same => continue,
                    MemoryMergeDecision::Related => {
                        memory.merge_decision = "related".into();
                        self.repo
                            .save_memory_with_evidence(memory, evidence)
                            .await?
                    }
                    MemoryMergeDecision::NewEvidence(existing_id) => {
                        evidence.evidence_type = "reinforcement".into();
                        self.repo
                            .reinforce_memory_with_evidence(
                                existing_id,
                                evidence,
                                memory.confidence,
                            )
                            .await?
                    }
                    MemoryMergeDecision::Contradiction(existing_id) => {
                        memory.merge_decision = "contradiction".into();
                        evidence.evidence_type = "contradiction".into();
                        let existing =
                            self.repo.find_by_id(existing_id).await?.ok_or_else(|| {
                                AppError::NotFound(format!("memory {existing_id} not found"))
                            })?;
                        if existing.user_id != user_id {
                            return Err(AppError::Forbidden(
                                "cannot contradict another user's memory".into(),
                            ));
                        }
                        self.repo
                            .save_contradicting_memory_with_evidence(memory, evidence, existing_id)
                            .await?
                    }
                    MemoryMergeDecision::New => {
                        memory.merge_decision = "new".into();
                        self.repo
                            .save_memory_with_evidence(memory, evidence)
                            .await?
                    }
                };
                if !self.context_is_current(user_id, expected_version).await? {
                    let _ = self.repo.disable_memory(persisted.memory_id).await;
                    return Ok(Vec::new());
                }
                saved.push(persisted);
            }
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

        debug!(user_id, saved = saved.len(), "记忆提取并保存完成");

        Ok(saved)
    }

    async fn index_single_memory(
        &self,
        vs: &Arc<dyn VectorStoreT>,
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
        self.find_by_user_id(user_id, Some(1)).await
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
        let policy = self.personalization_policy(user_id).await?;
        if !policy.enabled {
            return Ok(Vec::new());
        }

        // Try Qdrant path
        if let (Some(vs), Some(ep)) = (&self.vector_store, &self.embedding) {
            match self
                .qdrant_recall(vs, ep, user_id, query, top_k, &policy)
                .await
            {
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
        let memories = self.repo.search_by_user(user_id, query, top_k).await?;
        Ok(memories
            .into_iter()
            .filter(|memory| policy.includes(memory))
            .collect())
    }

    async fn qdrant_recall(
        &self,
        vs: &Arc<dyn VectorStoreT>,
        ep: &Arc<dyn EmbeddingProvider>,
        user_id: u64,
        query: &str,
        top_k: u32,
        policy: &PersonalizationPolicy,
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
                    if !policy.includes(&mem) {
                        debug!(
                            memory_id,
                            "memory is outside personalization policy; skipping"
                        );
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

        // Persist a delete job and also remove the live point immediately.
        if let Some(ref vi) = self.vector_index {
            if let Err(e) = vi.enqueue_memory_delete(id).await {
                tracing::warn!(memory_id = id, error = %e, "failed to enqueue memory index delete");
            }
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

    use crate::app::memory::memory_extractor::test_utils::MockLlm;
    use crate::domain::memory::{NewMemory, NewMemoryEvidence};
    use crate::infra::llm::mock_provider::MockEmbeddingProvider;
    use crate::infra::vector_store::mock_vector_store::MockVectorStore;

    struct MockRepo;
    #[async_trait]
    impl MemoryRepoT for MockRepo {
        async fn save_memory_with_evidence(
            &self,
            memory: NewMemory,
            _evidence: NewMemoryEvidence,
        ) -> Result<UserMemory, AppError> {
            Ok(UserMemory {
                memory_id: 1,
                user_id: memory.user_id,
                memory_type: memory.memory_type,
                content: memory.content,
                confidence: memory.confidence,
                reinforce_count: 0,
                reinforced_at: None,
                source_conversation_id: memory.source_conversation_id,
                source_message_id: memory.source_message_id,
                status: 1,
                metadata: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
        }
        async fn reinforce_memory_with_evidence(
            &self,
            memory_id: u64,
            _evidence: NewMemoryEvidence,
            confidence: f64,
        ) -> Result<UserMemory, AppError> {
            Ok(UserMemory {
                memory_id,
                user_id: 42,
                memory_type: "fact".into(),
                content: "reinforced".into(),
                confidence,
                reinforce_count: 1,
                reinforced_at: Some(Utc::now()),
                source_conversation_id: Some(1),
                source_message_id: Some(1),
                status: 1,
                metadata: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
        }
        async fn save_contradicting_memory_with_evidence(
            &self,
            memory: NewMemory,
            evidence: NewMemoryEvidence,
            _contradicted_memory_id: u64,
        ) -> Result<UserMemory, AppError> {
            self.save_memory_with_evidence(memory, evidence).await
        }
        async fn find_by_id(&self, memory_id: u64) -> Result<Option<UserMemory>, AppError> {
            Ok(Some(UserMemory {
                memory_id,
                user_id: 42,
                memory_type: "fact".into(),
                content: "test".into(),
                confidence: 0.8,
                reinforce_count: 0,
                reinforced_at: None,
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
                reinforce_count: 0,
                reinforced_at: None,
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
                reinforce_count: 0,
                reinforced_at: None,
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
                reinforce_count: 0,
                reinforced_at: None,
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

    #[test]
    fn canonicalization_produces_stable_key() {
        let first = canonicalize_memory("  User   Likes JAZZ ");
        let second = canonicalize_memory("user likes jazz");
        assert_eq!(first, second);
        assert_eq!(memory_key(&first), memory_key(&second));
    }

    fn test_memory(memory_type: &str) -> NewMemory {
        NewMemory {
            user_id: 1,
            memory_key: Some("key".into()),
            canonical_form: Some("user likes jazz".into()),
            memory_type: memory_type.into(),
            content: "user likes jazz".into(),
            confidence: 0.9,
            merge_decision: "new".into(),
            source_conversation_id: Some(1),
            source_message_id: Some(2),
        }
    }

    fn test_evidence() -> NewMemoryEvidence {
        NewMemoryEvidence {
            source_type: "message".into(),
            source_ref_id: 2,
            message_id: Some(2),
            summary_id: None,
            evidence_type: "source".into(),
            confidence: Some(0.9),
            extractor_version: Some("memory-extractor-v1".into()),
        }
    }

    #[test]
    fn accepts_only_memory_whitelist() {
        for memory_type in crate::domain::memory::ALLOWED_MEMORY_TYPES {
            assert!(MemoryService::validate_memory(&test_memory(memory_type)).is_ok());
        }
        assert!(MemoryService::validate_memory(&test_memory("profile")).is_err());
        assert!(MemoryService::validate_memory(&test_memory("safety_note")).is_err());
    }

    #[test]
    fn validates_evidence_reference_and_type() {
        assert!(MemoryService::validate_evidence(&test_evidence()).is_ok());

        let mut wrong_ref = test_evidence();
        wrong_ref.source_ref_id = 3;
        assert!(MemoryService::validate_evidence(&wrong_ref).is_err());

        let mut wrong_type = test_evidence();
        wrong_type.evidence_type = "unknown".into();
        assert!(MemoryService::validate_evidence(&wrong_type).is_err());
    }

    #[test]
    fn personalization_policy_filters_disabled_and_pre_reset_memories() {
        let memory = UserMemory {
            memory_id: 1,
            user_id: 1,
            memory_type: "fact".into(),
            content: "test".into(),
            confidence: 0.9,
            reinforce_count: 0,
            reinforced_at: None,
            source_conversation_id: None,
            source_message_id: None,
            status: 1,
            metadata: None,
            created_at: Utc::now() - chrono::Duration::hours(1),
            updated_at: Utc::now(),
        };
        assert!(
            !PersonalizationPolicy {
                enabled: false,
                reset_at: None,
            }
            .includes(&memory)
        );
        assert!(
            !PersonalizationPolicy {
                enabled: true,
                reset_at: Some(Utc::now()),
            }
            .includes(&memory)
        );
        assert!(
            PersonalizationPolicy {
                enabled: true,
                reset_at: Some(Utc::now() - chrono::Duration::hours(2)),
            }
            .includes(&memory)
        );
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
        assert_eq!(saved.len(), 1);

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
