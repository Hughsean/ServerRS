use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::app::context_routing::ContextRoutingService;
use crate::domain::llm::{EmbeddingProvider, LlmProvider};
use crate::domain::semantic_classification::SemanticClassifierT;
use crate::infra::llm::ollama_embedding_provider::OllamaEmbeddingProvider;
use crate::infra::llm::ollama_provider::OllamaProvider;
use crate::infra::semantic_classification::EmbeddingSemanticClassifier;
use crate::repositories::{RepositorySet, build_repositories};
use crate::shared::config::AppConfig;

#[cfg(feature = "qdrant")]
use crate::domain::vector_store::VectorStoreT;
#[cfg(feature = "qdrant")]
use crate::infra::vector_store::qdrant_vector_store::QdrantVectorStore;

pub async fn connect_db(config: &AppConfig) -> DatabaseConnection {
    let mut options = sea_orm::ConnectOptions::new(config.database.url.clone());
    options.max_connections(config.database.max_connections.max(1));
    sea_orm::Database::connect(options)
        .await
        .unwrap_or_else(|error| panic!("连接测试数据库失败: {error}"))
}

pub fn repos(db: &DatabaseConnection, config: &AppConfig) -> RepositorySet {
    build_repositories(
        db,
        &config.vector_store.memory_index_name,
        &config.vector_store.summary_index_name,
    )
}

pub fn embedding_provider(config: &AppConfig) -> Arc<dyn EmbeddingProvider> {
    Arc::new(OllamaEmbeddingProvider::with_options(
        config.embedding.base_url.clone(),
        config.embedding.model.clone(),
        config.embedding.dimension,
        config.embedding.batch_size,
        config.embedding.timeout_secs,
    ))
}

pub fn llm_provider(config: &AppConfig) -> Arc<dyn LlmProvider> {
    Arc::new(OllamaProvider::with_timeout(
        config.llm.base_url.clone(),
        config.llm.chat_model.clone(),
        config.llm.timeout_secs,
    ))
}

#[cfg(feature = "qdrant")]
pub async fn vector_store(config: &AppConfig) -> Arc<dyn VectorStoreT> {
    Arc::new(
        QdrantVectorStore::new(
            &config.vector_store.url,
            config.vector_store.api_key.as_deref(),
        )
        .await
        .unwrap_or_else(|error| panic!("初始化测试 Qdrant 客户端失败: {error}")),
    )
}

pub async fn context_routing_service(
    config: &AppConfig,
    embedding_provider: Arc<dyn EmbeddingProvider>,
) -> Arc<ContextRoutingService> {
    let classifier: Arc<dyn SemanticClassifierT> = Arc::new(
        EmbeddingSemanticClassifier::from_config(
            &config.semantic_classification,
            embedding_provider,
        )
        .await
        .unwrap_or_else(|error| panic!("初始化测试语义分类器失败: {error}")),
    );
    Arc::new(ContextRoutingService::new(
        classifier,
        config.context_routing.clone(),
    ))
}
