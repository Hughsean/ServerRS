use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use crate::domain::llm::EmbeddingProvider;
use crate::domain::vector_store::{
    VectorDistance, VectorFilter, VectorPoint, VectorSearchHit, VectorStoreT,
};
use crate::shared::error::AppError;

pub struct ReadOnlyCountingVectorStore {
    inner: Arc<dyn VectorStoreT>,
    search_count: AtomicUsize,
    write_count: AtomicUsize,
}

impl ReadOnlyCountingVectorStore {
    pub fn new(inner: Arc<dyn VectorStoreT>) -> Self {
        Self {
            inner,
            search_count: AtomicUsize::new(0),
            write_count: AtomicUsize::new(0),
        }
    }

    pub fn search_count(&self) -> usize {
        self.search_count.load(Ordering::SeqCst)
    }

    pub fn write_count(&self) -> usize {
        self.write_count.load(Ordering::SeqCst)
    }

    fn reject_write(&self, action: &str, collection: &str) -> AppError {
        self.write_count.fetch_add(1, Ordering::SeqCst);
        AppError::Internal(format!(
            "只读集成测试不允许执行 Qdrant 写操作: {action}({collection})"
        ))
    }
}

#[async_trait]
impl VectorStoreT for ReadOnlyCountingVectorStore {
    async fn ensure_collection(
        &self,
        collection: &str,
        _dimension: usize,
        _distance: VectorDistance,
    ) -> Result<(), AppError> {
        Err(self.reject_write("ensure_collection", collection))
    }

    async fn upsert_points(
        &self,
        collection: &str,
        _points: Vec<VectorPoint>,
    ) -> Result<(), AppError> {
        Err(self.reject_write("upsert_points", collection))
    }

    async fn search(
        &self,
        collection: &str,
        query: Vec<f32>,
        filter: VectorFilter,
        limit: usize,
    ) -> Result<Vec<VectorSearchHit>, AppError> {
        self.search_count.fetch_add(1, Ordering::SeqCst);
        self.inner.search(collection, query, filter, limit).await
    }

    async fn delete_points(&self, collection: &str, _ids: Vec<String>) -> Result<(), AppError> {
        Err(self.reject_write("delete_points", collection))
    }
}

pub async fn assert_searchable(
    vector_store: &Arc<dyn VectorStoreT>,
    embedding_provider: &Arc<dyn EmbeddingProvider>,
    collection: &str,
) {
    let vectors = embedding_provider
        .embed(&["Qdrant 只读连通性测试".to_string()])
        .await
        .unwrap_or_else(|error| panic!("测试 embedding 调用失败: {error}"));
    let query = vectors
        .into_iter()
        .next()
        .filter(|vector| !vector.is_empty())
        .unwrap_or_else(|| panic!("测试 embedding 返回空向量"));

    vector_store
        .search(collection, query, VectorFilter::default(), 1)
        .await
        .unwrap_or_else(|error| panic!("测试 Qdrant 只读搜索失败: {error}"));
}
