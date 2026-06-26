pub mod types;

use async_trait::async_trait;

use crate::shared::error::AppError;
pub use types::*;

/// 向量相似性搜索后端的抽象 (Qdrant, in-memory mock, etc.).
///
/// This trait lives in the domain layer — no Qdrant-specific types leak through.
/// Implementations map Qdrant / mock internals to these domain types.
#[async_trait]
pub trait VectorStoreT: Send + Sync {
    /// 确保指定名称的集合存在，具有给定的维度和距离度量。
    /// If the collection already exists with matching parameters, this is a no-op.
    /// If it exists with different parameters, return an error.
    async fn ensure_collection(
        &self,
        collection: &str,
        dimension: usize,
        distance: VectorDistance,
    ) -> Result<(), AppError>;

    /// 批量更新（插入或更新）向量点。
    async fn upsert_points(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> Result<(), AppError>;

    /// 搜索 `query` 向量的最近邻, respecting the given filter.
    async fn search(
        &self,
        collection: &str,
        query: Vec<f32>,
        filter: VectorFilter,
        limit: usize,
    ) -> Result<Vec<VectorSearchHit>, AppError>;

    /// 从集合中按 ID 删除向量点。
    async fn delete_points(&self, collection: &str, ids: Vec<String>) -> Result<(), AppError>;
}
