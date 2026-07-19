//! Pipeline dependency bundle shared by all web-ingestion handlers.
//!
//! Replaces the 14-argument handler signatures from the original monolithic
//! `bootstrap/web_ingestion.rs`. A single `PipelineContext` is built once during
//! bootstrap and passed by reference to the dispatcher and every handler.

use std::sync::Arc;

use crate::domain::llm::EmbeddingProvider;
use crate::domain::rag::RAGRepoT;
use crate::domain::vector_store::VectorStoreT;
use crate::domain::web_ingestion::distiller::KnowledgeDistiller;
use crate::domain::web_ingestion::fetcher::WebContentFetcher;
use crate::domain::web_ingestion::repo::*;
use crate::shared::config::{EmbeddingConfig, WebIngestionConfig};

/// All dependencies a handler may need. Cheap to clone (everything is `Arc`).
#[derive(Clone)]
pub struct PipelineContext {
    pub source_repo: Arc<dyn WebSourceRepoT>,
    pub source_url_repo: Arc<dyn WebSourceUrlRepoT>,
    pub crawl_job_repo: Arc<dyn WebCrawlJobRepoT>,
    pub page_repo: Arc<dyn WebPageRepoT>,
    pub run_repo: Arc<dyn IngestionRunRepoT>,
    pub publish_repo: Arc<dyn PublishRecordRepoT>,
    pub chunk_manifest_repo: Arc<dyn ChunkManifestRepoT>,
    pub vector_manifest_repo: Arc<dyn VectorManifestRepoT>,
    pub outbox_repo: Arc<dyn OutboxRepoT>,
    pub audit_repo: Arc<dyn AuditLogRepoT>,
    /// RAG knowledge store (knowledge_documents / knowledge_chunks / embeddings).
    pub rag_repo: Arc<dyn RAGRepoT>,
    pub fetcher: Arc<dyn WebContentFetcher>,
    pub distiller: Arc<dyn KnowledgeDistiller>,
    /// Embedding provider — MUST be separate from the distill chat LLM.
    pub embedding_provider: Arc<dyn EmbeddingProvider>,
    /// 可选向量存储。None 表示向量检索未启用。
    pub vector_store: Option<Arc<dyn VectorStoreT>>,
    pub config: WebIngestionConfig,
    /// Real embedding config — source of truth for embedding model / dimension
    /// used in run_key/version_key (§5.6). NEVER use the distill model here.
    pub embedding: EmbeddingConfig,
}

impl PipelineContext {
    /// The effective embedding model name. Comes from the real EmbeddingConfig,
    /// never from the distill chat LLM (§5.6, hard constraint #3/#4).
    pub fn embedding_model(&self) -> &str {
        &self.embedding.model
    }

    pub fn embedding_provider_name(&self) -> &str {
        &self.embedding.provider
    }

    pub fn embedding_dimension(&self) -> usize {
        self.embedding.dimension
    }

    pub fn llm_prompt_version(&self) -> &str {
        &self.config.llm_prompt_version
    }

    pub fn chunker_version(&self) -> &str {
        &self.config.chunker_version
    }

    pub fn pipeline_version(&self) -> &str {
        &self.config.pipeline_version
    }
}
