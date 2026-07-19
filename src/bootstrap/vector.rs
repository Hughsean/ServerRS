use std::sync::Arc;

use crate::app::rag::vector_index_service::{VectorIndexConfig, VectorIndexService};
use crate::bootstrap::infra::InfraContext;
use crate::bootstrap::repos::RepoGraph;
use crate::domain::llm::EmbeddingProvider;
use crate::domain::vector_index::VectorIndexRepoT;
use crate::domain::vector_store::VectorStoreT;
use crate::infra::llm::ollama_embedding_provider::OllamaEmbeddingProvider;
use crate::infra::repo::seaorm_impl::vector_index::VectorIndexRepo;
use crate::shared::config::AppConfig;

/// Embedding Provider、向量存储、VectorIndex。
pub struct VectorContext {
    pub embedding_provider: Arc<dyn EmbeddingProvider>,
    pub vector_store: Option<Arc<dyn VectorStoreT>>,
    pub vector_index: Option<Arc<VectorIndexService>>,
}

impl VectorContext {
    /// 构造 EmbeddingProvider → 向量存储 → VectorIndex（含索引初始化）。
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

        // ── 向量存储（可选，通过配置启用）──
        let vector_store: Option<Arc<dyn VectorStoreT>> = if config.vector_store.enabled {
            #[cfg(feature = "qdrant")]
            {
                let qdrant =
                    crate::infra::vector_store::qdrant_vector_store::QdrantVectorStore::new(
                        &config.vector_store.url,
                        config.vector_store.api_key.as_deref(),
                    )
                    .await
                    .map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::Other,
                            format!("vector store init failed: {e}"),
                        )
                    })?;
                Some(Arc::new(qdrant) as Arc<dyn VectorStoreT>)
            }
            #[cfg(not(feature = "qdrant"))]
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "vector_store.enabled=true but binary built without --features qdrant",
                ));
            }
        } else {
            None
        };

        // ── VectorIndex ──
        let vector_index_repo: Arc<dyn VectorIndexRepoT> =
            Arc::new(VectorIndexRepo::new(infra.db.clone()));

        let vector_index: Option<Arc<VectorIndexService>> = vector_store.as_ref().map(|vs| {
            Arc::new(VectorIndexService::new(
                Arc::clone(&repos.rag_repo),
                Arc::clone(&repos.memory_repo),
                Arc::clone(&repos.summary_repo),
                vector_index_repo,
                Arc::clone(vs),
                Arc::clone(&embedding_provider),
                VectorIndexConfig {
                    rag_collection: config.vector_store.rag_index_name.clone(),
                    memory_collection: config.vector_store.memory_index_name.clone(),
                    summary_collection: config.vector_store.summary_index_name.clone(),
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

    /// 确保向量索引已存在（在构造后调用）。
    pub async fn ensure_indexes(&self) -> Result<(), std::io::Error> {
        if let Some(ref vi) = self.vector_index {
            vi.ensure_collections()
                .await
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        }
        Ok(())
    }
}
