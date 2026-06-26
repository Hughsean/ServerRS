use std::sync::Arc;

use crate::app::rag::vector_index_service::{VectorIndexConfig, VectorIndexService};
use crate::bootstrap::infra::InfraContext;
use crate::bootstrap::repos::RepoGraph;
use crate::domain::llm::EmbeddingProvider;
use crate::domain::vector_index::VectorIndexRepository;
use crate::domain::vector_store::VectorStore;
use crate::infra::db::imp::seaorm_vector_index_repository::SeaOrmVectorIndexRepository;
use crate::infra::llm::ollama_embedding_provider::OllamaEmbeddingProvider;
use crate::shared::config::AppConfig;

/// Embedding Provider、Qdrant 向量存储、VectorIndex。
pub struct VectorContext {
    pub embedding_provider: Arc<dyn EmbeddingProvider>,
    pub vector_store: Option<Arc<dyn VectorStore>>,
    pub vector_index: Option<Arc<VectorIndexService>>,
}

impl VectorContext {
    /// 构造 EmbeddingProvider → Qdrant → VectorIndex（含集合初始化）。
    pub async fn new(
        config: &AppConfig,
        infra: &InfraContext,
        repos: &RepoGraph,
    ) -> Result<Self, std::io::Error> {
        let embedding_provider: Arc<dyn EmbeddingProvider> =
            Arc::new(OllamaEmbeddingProvider::with_options(
                config.embedding.base_url.clone(),
                config.embedding.model.clone(),
                config.embedding.dimension,
                config.embedding.batch_size,
                config.embedding.timeout_secs,
            ));

        // ── Qdrant 向量存储（可选，通过配置启用）──
        let vector_store: Option<Arc<dyn VectorStore>> = if config.qdrant.enabled {
            #[cfg(feature = "qdrant")]
            {
                let qdrant =
                    crate::infra::vector_store::qdrant_vector_store::QdrantVectorStore::new(
                        &config.qdrant.url,
                        config.qdrant.api_key.as_deref(),
                    )
                    .await
                    .map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::Other,
                            format!("Qdrant init failed: {e}"),
                        )
                    })?;
                Some(Arc::new(qdrant) as Arc<dyn VectorStore>)
            }
            #[cfg(not(feature = "qdrant"))]
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "qdrant.enabled=true but binary built without --features qdrant",
                ));
            }
        } else {
            None
        };

        // ── VectorIndex ──
        let vector_index_repo: Arc<dyn VectorIndexRepository> =
            Arc::new(SeaOrmVectorIndexRepository::new(infra.db.clone()));

        let vector_index: Option<Arc<VectorIndexService>> = vector_store.as_ref().map(|vs| {
            Arc::new(VectorIndexService::new(
                Arc::clone(&repos.rag_repo),
                Arc::clone(&repos.memory_repo),
                Arc::clone(&repos.summary_repo),
                vector_index_repo,
                Arc::clone(vs),
                Arc::clone(&embedding_provider),
                VectorIndexConfig {
                    rag_collection: config.qdrant.rag_collection.clone(),
                    memory_collection: config.qdrant.memory_collection.clone(),
                    summary_collection: config.qdrant.summary_collection.clone(),
                    ..Default::default()
                },
            ))
        });

        Ok(Self {
            embedding_provider,
            vector_store,
            vector_index,
        })
    }

    /// 确保 Qdrant 集合已存在（在构造后调用）。
    pub async fn ensure_collections(&self) -> Result<(), std::io::Error> {
        if let Some(ref vi) = self.vector_index {
            vi.ensure_collections()
                .await
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        }
        Ok(())
    }
}
