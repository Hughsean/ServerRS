use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::domain::vector_store::{
    VectorCondition, VectorDistance, VectorFilter, VectorPoint, VectorSearchHit, VectorStoreT,
};
use crate::shared::error::AppError;

/// In-memory mock `VectorStore` for unit tests — no external service needed.
///
/// Collections are created on first `ensure_collection` call and points are
/// stored in a `HashMap<String, Vec<VectorPoint>>`.  Search uses a brute-force
/// inner-product loop (cosine-similarity approximation when vectors are
/// normalized) and respects `VectorFilter`.
pub struct MockVectorStore {
    collections: Mutex<HashMap<String, CollectionState>>,
}

struct CollectionState {
    dimension: usize,
    distance: VectorDistance,
    points: Vec<StoredPoint>,
}

#[derive(Clone)]
struct StoredPoint {
    id: String,
    vector: Vec<f32>,
    payload: serde_json::Value,
}

impl MockVectorStore {
    pub fn new() -> Self {
        Self {
            collections: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for MockVectorStore {
    fn default() -> Self {
        Self::new()
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let (dot, norm_a, norm_b) = a
        .iter()
        .zip(b.iter())
        .fold((0.0f32, 0.0f32, 0.0f32), |(d, na, nb), (x, y)| {
            (d + x * y, na + x * x, nb + y * y)
        });
    let denom = (norm_a.sqrt() * norm_b.sqrt()).max(1e-12);
    dot / denom
}

fn matches_condition(point: &StoredPoint, cond: &VectorCondition) -> bool {
    match cond {
        VectorCondition::MatchString { key, value } => point
            .payload
            .get(key)
            .and_then(|v| v.as_str())
            .map(|v| v == value.as_str())
            .unwrap_or(false),
        VectorCondition::MatchU64 { key, value } => point
            .payload
            .get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v == *value)
            .unwrap_or(false),
        VectorCondition::MatchI64 { key, value } => point
            .payload
            .get(key)
            .and_then(|v| v.as_i64())
            .map(|v| v == *value)
            .unwrap_or(false),
        VectorCondition::MatchBool { key, value } => point
            .payload
            .get(key)
            .and_then(|v| v.as_bool())
            .map(|v| v == *value)
            .unwrap_or(false),
        VectorCondition::RangeI64 {
            key,
            gt,
            gte,
            lt,
            lte,
        } => {
            let Some(value) = point.payload.get(key).and_then(|v| v.as_i64()) else {
                return false;
            };
            if gt.is_some_and(|bound| value <= bound) {
                return false;
            }
            if gte.is_some_and(|bound| value < bound) {
                return false;
            }
            if lt.is_some_and(|bound| value >= bound) {
                return false;
            }
            if lte.is_some_and(|bound| value > bound) {
                return false;
            }
            true
        }
    }
}

fn matches_all_conditions(point: &StoredPoint, filter: &VectorFilter) -> bool {
    filter
        .must
        .iter()
        .all(|cond| matches_condition(point, cond))
}

#[async_trait]
impl VectorStoreT for MockVectorStore {
    async fn ensure_collection(
        &self,
        collection: &str,
        dimension: usize,
        distance: VectorDistance,
    ) -> Result<(), AppError> {
        let mut cols = self
            .collections
            .lock()
            .map_err(|e| AppError::Internal(format!("mock vector store lock poisoned: {e}")))?;
        if let Some(existing) = cols.get(collection) {
            if existing.dimension != dimension {
                return Err(AppError::Validation(format!(
                    "collection '{collection}' has dimension {} but requested {dimension}",
                    existing.dimension
                )));
            }
            if existing.distance != distance {
                return Err(AppError::Validation(format!(
                    "collection '{collection}' has distance {:?} but requested {distance:?}",
                    existing.distance
                )));
            }
            return Ok(());
        }
        cols.insert(
            collection.to_string(),
            CollectionState {
                dimension,
                distance,
                points: Vec::new(),
            },
        );
        Ok(())
    }

    async fn upsert_points(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> Result<(), AppError> {
        let mut cols = self
            .collections
            .lock()
            .map_err(|e| AppError::Internal(format!("mock vector store lock poisoned: {e}")))?;
        let state = cols.get_mut(collection).ok_or_else(|| {
            AppError::NotFound(format!("collection '{collection}' does not exist"))
        })?;

        for pt in points {
            // Deduplicate by id: remove any existing point with the same id
            state.points.retain(|p| p.id != pt.id);
            state.points.push(StoredPoint {
                id: pt.id,
                vector: pt.vector,
                payload: pt.payload,
            });
        }
        Ok(())
    }

    async fn search(
        &self,
        collection: &str,
        query: Vec<f32>,
        filter: VectorFilter,
        limit: usize,
    ) -> Result<Vec<VectorSearchHit>, AppError> {
        let cols = self
            .collections
            .lock()
            .map_err(|e| AppError::Internal(format!("mock vector store lock poisoned: {e}")))?;
        let state = cols.get(collection).ok_or_else(|| {
            AppError::NotFound(format!("collection '{collection}' does not exist"))
        })?;

        let mut scored: Vec<(f32, &StoredPoint)> = state
            .points
            .iter()
            .filter(|p| matches_all_conditions(p, &filter))
            .map(|p| (cosine_similarity(&query, &p.vector), p))
            .collect();

        // Sort descending by score
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let results: Vec<VectorSearchHit> = scored
            .into_iter()
            .take(limit)
            .map(|(score, pt)| VectorSearchHit {
                id: pt.id.clone(),
                score,
                payload: pt.payload.clone(),
            })
            .collect();

        Ok(results)
    }

    async fn delete_points(&self, collection: &str, ids: Vec<String>) -> Result<(), AppError> {
        let mut cols = self
            .collections
            .lock()
            .map_err(|e| AppError::Internal(format!("mock vector store lock poisoned: {e}")))?;
        let state = cols.get_mut(collection).ok_or_else(|| {
            AppError::NotFound(format!("collection '{collection}' does not exist"))
        })?;
        state.points.retain(|p| !ids.contains(&p.id));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_upsert_and_search() {
        let store = MockVectorStore::new();
        store
            .ensure_collection("test", 384, VectorDistance::Cosine)
            .await
            .unwrap();

        let points = vec![VectorPoint {
            id: "pt-1".into(),
            vector: vec![1.0, 0.0, 0.0],
            payload: json!({"user_id": 42, "text": "hello"}),
        }];
        store.upsert_points("test", points).await.unwrap();

        let hits = store
            .search("test", vec![1.0, 0.1, 0.0], VectorFilter::default(), 5)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].score > 0.9);
    }

    #[tokio::test]
    async fn test_collection_isolation() {
        let store = MockVectorStore::new();
        store
            .ensure_collection("a", 3, VectorDistance::Cosine)
            .await
            .unwrap();
        store
            .ensure_collection("b", 3, VectorDistance::Cosine)
            .await
            .unwrap();

        store
            .upsert_points(
                "a",
                vec![VectorPoint {
                    id: "1".into(),
                    vector: vec![1.0, 0.0, 0.0],
                    payload: json!({}),
                }],
            )
            .await
            .unwrap();

        let hits = store
            .search("b", vec![1.0, 0.0, 0.0], VectorFilter::default(), 5)
            .await
            .unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn test_filter_by_user_id() {
        let store = MockVectorStore::new();
        store
            .ensure_collection("mem", 3, VectorDistance::Cosine)
            .await
            .unwrap();

        store
            .upsert_points(
                "mem",
                vec![
                    VectorPoint {
                        id: "m1".into(),
                        vector: vec![1.0, 0.0, 0.0],
                        payload: json!({"user_id": 1, "content": "a"}),
                    },
                    VectorPoint {
                        id: "m2".into(),
                        vector: vec![1.0, 0.1, 0.0],
                        payload: json!({"user_id": 2, "content": "b"}),
                    },
                ],
            )
            .await
            .unwrap();

        let filter = VectorFilter::default().with_condition(VectorCondition::MatchU64 {
            key: "user_id".into(),
            value: 1,
        });

        let hits = store
            .search("mem", vec![1.0, 0.0, 0.0], filter, 5)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "m1");
    }

    #[tokio::test]
    async fn test_filter_by_i64_range() {
        let store = MockVectorStore::new();
        store
            .ensure_collection("fresh", 3, VectorDistance::Cosine)
            .await
            .unwrap();

        store
            .upsert_points(
                "fresh",
                vec![
                    VectorPoint {
                        id: "expired".into(),
                        vector: vec![1.0, 0.0, 0.0],
                        payload: json!({"expires_at_ts": 100}),
                    },
                    VectorPoint {
                        id: "active".into(),
                        vector: vec![1.0, 0.0, 0.0],
                        payload: json!({"expires_at_ts": 200}),
                    },
                ],
            )
            .await
            .unwrap();

        let filter = VectorFilter::default().with_condition(VectorCondition::RangeI64 {
            key: "expires_at_ts".into(),
            gt: Some(150),
            gte: None,
            lt: None,
            lte: None,
        });

        let hits = store
            .search("fresh", vec![1.0, 0.0, 0.0], filter, 5)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "active");
    }

    #[tokio::test]
    async fn test_limit() {
        let store = MockVectorStore::new();
        store
            .ensure_collection("x", 2, VectorDistance::Cosine)
            .await
            .unwrap();

        let pts: Vec<_> = (0..10)
            .map(|i| VectorPoint {
                id: format!("p{i}"),
                vector: vec![i as f32, 0.0],
                payload: json!({}),
            })
            .collect();
        store.upsert_points("x", pts).await.unwrap();

        let hits = store
            .search("x", vec![9.0, 0.0], VectorFilter::default(), 3)
            .await
            .unwrap();
        assert_eq!(hits.len(), 3);
    }

    #[tokio::test]
    async fn test_delete_points() {
        let store = MockVectorStore::new();
        store
            .ensure_collection("d", 3, VectorDistance::Cosine)
            .await
            .unwrap();

        store
            .upsert_points(
                "d",
                vec![VectorPoint {
                    id: "del-me".into(),
                    vector: vec![1.0, 0.0, 0.0],
                    payload: json!({}),
                }],
            )
            .await
            .unwrap();

        store
            .delete_points("d", vec!["del-me".into()])
            .await
            .unwrap();

        let hits = store
            .search("d", vec![1.0, 0.0, 0.0], VectorFilter::default(), 5)
            .await
            .unwrap();
        assert!(hits.is_empty());
    }
}
