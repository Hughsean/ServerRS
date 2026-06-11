pub mod types;

use async_trait::async_trait;

use crate::shared::error::AppError;
pub use types::*;

/// Abstraction over a vector similarity search backend (Qdrant, in-memory mock, etc.).
///
/// This trait lives in the domain layer — no Qdrant-specific types leak through.
/// Implementations map Qdrant / mock internals to these domain types.
#[async_trait]
pub trait VectorStore: Send + Sync {
    /// Ensure a named collection exists with the given dimension and distance metric.
    /// If the collection already exists with matching parameters, this is a no-op.
    /// If it exists with different parameters, return an error.
    async fn ensure_collection(
        &self,
        collection: &str,
        dimension: usize,
        distance: VectorDistance,
    ) -> Result<(), AppError>;

    /// Upsert (insert or update) a batch of points.
    async fn upsert_points(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> Result<(), AppError>;

    /// Search for the nearest neighbours of `query` vector, respecting the given filter.
    async fn search(
        &self,
        collection: &str,
        query: Vec<f32>,
        filter: VectorFilter,
        limit: usize,
    ) -> Result<Vec<VectorSearchHit>, AppError>;

    /// Delete points by their IDs from a collection.
    async fn delete_points(&self, collection: &str, ids: Vec<String>) -> Result<(), AppError>;
}
