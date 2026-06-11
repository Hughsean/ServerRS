use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::shared::error::AppError;

/// Represents a user's long-term memory entry.
///
/// Supported memory types:
/// - `preference`: User stated preferences (e.g. "I like jazz music")
/// - `profile`: Personal background facts (e.g. "user is a college student")
/// - `fact`: Objective facts inferred or stated (e.g. "user has a cat named Luna")
/// - `emotional_pattern`: Recurring emotional states or triggers
/// - `goal`: Current or past goals the user has stated
/// - `safety_note`: Safety-relevant observations (e.g. mentions of self-harm)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMemory {
    pub memory_id: u64,
    pub user_id: u64,
    pub memory_type: String,
    pub content: String,
    pub confidence: f64,
    pub source_conversation_id: Option<u64>,
    pub source_message_id: Option<u64>,
    pub status: i8,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Stores the embedding vector for a memory entry, keyed by provider/model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMemoryEmbedding {
    pub embedding_id: u64,
    pub memory_id: u64,
    pub provider: String,
    pub model: String,
    pub dimension: u32,
    pub embedding_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// Input struct for creating a new memory.
#[derive(Debug, Clone)]
pub struct NewMemory {
    pub user_id: u64,
    pub memory_type: String,
    pub content: String,
    pub confidence: f64,
    pub source_conversation_id: Option<u64>,
    pub source_message_id: Option<u64>,
}

/// A compressed summary of a conversation, used for context window management
/// and long-term memory consolidation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub summary_id: u64,
    pub conversation_id: u64,
    pub user_id: u64,
    pub summary_type: String,
    pub content: String,
    pub message_start_id: Option<u64>,
    pub message_end_id: Option<u64>,
    pub token_count: Option<u32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input struct for creating a new conversation summary.
#[derive(Debug, Clone)]
pub struct NewSummary {
    pub conversation_id: u64,
    pub user_id: u64,
    pub summary_type: String,
    pub content: String,
    pub message_start_id: Option<u64>,
    pub message_end_id: Option<u64>,
    pub token_count: Option<u32>,
}

/// Repository trait for persisting and querying user memories.
#[async_trait]
pub trait MemoryRepository: Send + Sync {
    /// Persist a new memory entry. Returns the saved memory with generated id and timestamps.
    async fn save_memory(&self, memory: NewMemory) -> Result<UserMemory, AppError>;

    /// Retrieve a single memory by its primary key.
    async fn find_by_id(&self, memory_id: u64) -> Result<Option<UserMemory>, AppError>;

    /// Retrieve all memories for a user, optionally filtered by status.
    async fn find_by_user_id(
        &self,
        user_id: u64,
        status: Option<i8>,
    ) -> Result<Vec<UserMemory>, AppError>;

    /// Semantic search over user memories.
    /// `top_k` controls the maximum number of results returned.
    async fn search_by_user(
        &self,
        user_id: u64,
        query: &str,
        top_k: u32,
    ) -> Result<Vec<UserMemory>, AppError>;

    /// Update a memory's content and/or confidence score.
    async fn update_memory(
        &self,
        memory_id: u64,
        content: Option<String>,
        confidence: Option<f64>,
    ) -> Result<UserMemory, AppError>;

    /// Soft-disable (logical delete) a memory by setting its status to a disabled value.
    async fn disable_memory(&self, memory_id: u64) -> Result<(), AppError>;

    /// Permanently remove a memory from the store.
    async fn delete_memory(&self, memory_id: u64) -> Result<bool, AppError>;

    /// Find all memories that were sourced from a given conversation.
    async fn find_memories_by_conversation(
        &self,
        conversation_id: u64,
    ) -> Result<Vec<UserMemory>, AppError>;

    /// Update vector-index metadata on a memory row.
    async fn update_memory_index_metadata(
        &self,
        memory_id: u64,
        vector_id: String,
        embedding_provider: String,
        embedding_model: String,
        embedding_dimension: u32,
    ) -> Result<(), AppError>;

    /// Record access (last_accessed_at = now, access_count += 1).
    async fn touch_memory_access(&self, memory_id: u64) -> Result<(), AppError>;

    /// Find a memory by its dedup key (user_id + memory_key).
    async fn find_by_memory_key(
        &self,
        user_id: u64,
        memory_key: &str,
    ) -> Result<Option<UserMemory>, AppError>;

    /// List memories eligible for vector indexing (status = 1, unindexed).
    async fn list_indexable_memories(
        &self,
        user_id: Option<u64>,
        limit: u64,
    ) -> Result<Vec<UserMemory>, AppError>;
}
