use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::shared::error::AppError;
use async_trait::async_trait;

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeDocument {
    pub document_id: u64,
    pub source_type: String,
    pub source_id: Option<u64>,
    pub title: Option<String>,
    pub content_hash: String,
    pub metadata: Option<serde_json::Value>,
    pub status: i8,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeChunk {
    pub chunk_id: u64,
    pub document_id: u64,
    pub chunk_index: u32,
    pub content: String,
    pub token_count: Option<u32>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEmbedding {
    pub embedding_id: u64,
    pub chunk_id: u64,
    pub provider: String,
    pub model: String,
    pub dimension: u32,
    pub embedding_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// New-* input structs (no auto-generated / default fields)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct NewDocument {
    pub source_type: String,
    pub source_id: Option<u64>,
    pub title: Option<String>,
    pub content_hash: String,
    pub metadata: Option<serde_json::Value>,
    pub status: i8,
}

#[derive(Debug, Clone)]
pub struct NewChunk {
    pub document_id: u64,
    pub chunk_index: u32,
    pub content: String,
    pub token_count: Option<u32>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct NewEmbedding {
    pub chunk_id: u64,
    pub provider: String,
    pub model: String,
    pub dimension: u32,
    pub embedding_json: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Repository trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait RAGRepository: Send + Sync {
    /// Persist a new document and return it with the assigned id.
    async fn save_document(&self, doc: NewDocument) -> Result<KnowledgeDocument, AppError>;

    /// Look up a document by its source type and optional source id.
    async fn find_document_by_source(
        &self,
        source_type: &str,
        source_id: Option<u64>,
    ) -> Result<Option<KnowledgeDocument>, AppError>;

    /// List all documents matching a source type.
    async fn list_documents_by_source_type(
        &self,
        source_type: &str,
    ) -> Result<Vec<KnowledgeDocument>, AppError>;

    /// Bulk-insert chunks for a document.
    async fn save_chunks(&self, chunks: &[NewChunk]) -> Result<Vec<KnowledgeChunk>, AppError>;

    /// Retrieve all chunks belonging to a document, ordered by chunk_index.
    async fn find_chunks_by_document(
        &self,
        document_id: u64,
    ) -> Result<Vec<KnowledgeChunk>, AppError>;

    /// Persist a single embedding.
    async fn save_embedding(
        &self,
        emb: NewEmbedding,
    ) -> Result<KnowledgeEmbedding, AppError>;

    /// Retrieve the embedding attached to a chunk, if any.
    async fn find_embedding_by_chunk(
        &self,
        chunk_id: u64,
    ) -> Result<Option<KnowledgeEmbedding>, AppError>;

    /// Full-text keyword search over chunk content.
    /// Returns chunk-score pairs ordered by descending relevance.
    async fn search_by_keyword(
        &self,
        query: &str,
        top_k: u64,
    ) -> Result<Vec<(KnowledgeChunk, f64)>, AppError>;

    /// Soft- or hard-delete a document (and cascading chunks / embeddings).
    async fn delete_document(&self, document_id: u64) -> Result<(), AppError>;

    /// Retrieve chunks that have a corresponding embedding row.
    /// Useful when rebuilding indexes or exporting vector data.
    async fn list_chunks_with_embeddings(
        &self,
    ) -> Result<Vec<(KnowledgeChunk, KnowledgeEmbedding)>, AppError>;
}
