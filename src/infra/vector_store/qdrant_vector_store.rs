use std::collections::HashMap;

use async_trait::async_trait;
use qdrant_client::qdrant::{
    Condition, CreateCollectionBuilder, DeletePointsBuilder, Distance, Filter, PointId,
    PointStruct, ScoredPoint, SearchPointsBuilder, UpsertPointsBuilder, Value, VectorParamsBuilder,
    point_id::PointIdOptions,
};
use qdrant_client::{Payload, Qdrant};
use tracing::{debug, warn};

use crate::domain::vector_store::{
    VectorCondition, VectorDistance, VectorFilter, VectorPoint, VectorSearchHit, VectorStoreT,
};
use crate::shared::error::AppError;

// ── Deterministic point-id mapping ──────────────────────────────────

/// Convert a business vector ID (e.g. `rag_chunk:42`) into a deterministic
/// Qdrant `PointId`. Uses FNV-1a for speed; collisions on the same
/// collection are astronomically unlikely for our cardinality.
fn vector_id_to_point_id(vector_id: &str) -> PointId {
    let hash = fnv_hash(vector_id);
    PointId::from(hash)
}

/// FNV-1a 64 位哈希 — 快速、确定、无需额外依赖。
fn fnv_hash(s: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut hash = FNV_OFFSET;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Extract the business `vector_id` from a Qdrant `ScoredPoint`.
/// 优先使用 payload 中的 `vector_id` 字段；回退到 Qdrant point-id。
fn hit_vector_id(pt: &ScoredPoint) -> String {
    if let Some(vid) = pt.payload.get("vector_id") {
        if let Some(qdrant_client::qdrant::value::Kind::StringValue(ref s)) = vid.kind {
            if !s.is_empty() {
                return s.clone();
            }
        }
    }
    // Fallback to the Qdrant point ID
    match &pt.id {
        Some(PointId {
            point_id_options: Some(PointIdOptions::Num(n)),
        }) => format!("point:{n}"),
        Some(PointId {
            point_id_options: Some(PointIdOptions::Uuid(u)),
        }) => u.clone(),
        _ => String::new(),
    }
}

// ══════════════════════════════════════════════════════════════════════

/// 基于 Qdrant 的 [`VectorStore`] 实现。
pub struct QdrantVectorStore {
    client: Qdrant,
}

impl QdrantVectorStore {
    pub async fn new(url: &str, api_key: Option<&str>) -> Result<Self, AppError> {
        let mut builder = Qdrant::from_url(url);

        if let Some(key) = api_key {
            builder = builder.api_key(key);
        }

        let client = builder.build().map_err(|e| {
            AppError::internal(format!("failed to build Qdrant client for {url}: {e}"))
        })?;

        match client.health_check().await {
            Ok(_) => debug!(url, "Qdrant health check passed"),
            Err(e) => warn!(url, error = %e, "Qdrant health check failed"),
        }

        Ok(Self { client })
    }

    // ── helpers ──────────────────────────────────────────────────────

    fn map_distance(d: VectorDistance) -> Distance {
        match d {
            VectorDistance::Cosine => Distance::Cosine,
            VectorDistance::Dot => Distance::Dot,
            VectorDistance::Euclid => Distance::Euclid,
        }
    }

    fn build_filter(filter: &VectorFilter) -> Option<Filter> {
        if filter.must.is_empty() {
            return None;
        }
        let conditions: Vec<Condition> = filter
            .must
            .iter()
            .map(|c| match c {
                VectorCondition::MatchString { key, value } => {
                    Condition::matches(key.clone(), value.clone())
                }
                VectorCondition::MatchU64 { key, value } => {
                    Condition::matches(key.clone(), *value as i64)
                }
                VectorCondition::MatchI64 { key, value } => Condition::matches(key.clone(), *value),
                VectorCondition::MatchBool { key, value } => {
                    Condition::matches(key.clone(), *value)
                }
            })
            .collect();
        Some(Filter::all(conditions))
    }

    fn domain_payload_to_qdrant(payload: serde_json::Value) -> Payload {
        let mut map: HashMap<String, Value> = match payload {
            serde_json::Value::Object(ref obj) => obj
                .iter()
                .map(|(k, v)| (k.clone(), json_to_qdrant_value(v.clone())))
                .collect(),
            _ => HashMap::new(),
        };
        // Ensure vector_id is always present in the Qdrant payload
        if let serde_json::Value::Object(ref obj) = payload {
            if let Some(vid) = obj.get("vector_id").and_then(|v| v.as_str()) {
                map.entry("vector_id".to_string())
                    .or_insert_with(|| vid.to_string().into());
            }
        }
        Payload::from(map)
    }

    async fn get_collection_dimension(&self, collection: &str) -> Result<Option<usize>, AppError> {
        match self.client.collection_info(collection).await {
            Ok(info) => {
                let cfg = match info.result.and_then(|r| r.config) {
                    Some(c) => c,
                    None => return Ok(None),
                };
                let params = match cfg.params {
                    Some(p) => p,
                    None => return Ok(None),
                };
                if let Some(vc) = params.vectors_config {
                    if let Some(config) = vc.config {
                        // Match on the oneof variant to extract VectorParams.size
                        if let qdrant_client::qdrant::vectors_config::Config::Params(p) = config {
                            return Ok(Some(p.size as usize));
                        }
                    }
                }
                Ok(None)
            }
            Err(e) => {
                let msg = format!("{e}");
                if msg.contains("Not found") || msg.contains("doesn't exist") {
                    Ok(None)
                } else {
                    Err(AppError::internal(format!(
                        "failed to check collection '{collection}': {e}"
                    )))
                }
            }
        }
    }
}

#[async_trait]
impl VectorStoreT for QdrantVectorStore {
    async fn ensure_collection(
        &self,
        collection: &str,
        dimension: usize,
        distance: VectorDistance,
    ) -> Result<(), AppError> {
        // Check if collection already exists
        if let Some(existing_dim) = self.get_collection_dimension(collection).await? {
            if existing_dim != dimension {
                return Err(AppError::Validation(format!(
                    "collection '{collection}' has dimension {existing_dim} but config expects {dimension}"
                )));
            }
            debug!(
                collection,
                dimension, "collection exists with matching dimension"
            );
            return Ok(());
        }

        // Create new collection
        let dist = Self::map_distance(distance);
        self.client
            .create_collection(
                CreateCollectionBuilder::new(collection)
                    .vectors_config(VectorParamsBuilder::new(dimension as u64, dist)),
            )
            .await
            .map_err(|e| {
                AppError::internal(format!(
                    "failed to create Qdrant collection '{collection}' (dim={dimension}): {e}"
                ))
            })?;

        debug!(collection, dimension, "created Qdrant collection");
        Ok(())
    }

    async fn upsert_points(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> Result<(), AppError> {
        if points.is_empty() {
            return Ok(());
        }

        let count = points.len();
        let point_structs: Vec<PointStruct> = points
            .into_iter()
            .map(|pt| {
                let vid = pt.id.clone();
                let point_id = vector_id_to_point_id(&vid);
                // Inject vector_id into payload so search can return it
                let mut payload = pt.payload;
                if let serde_json::Value::Object(ref mut obj) = payload {
                    obj.insert("vector_id".to_string(), serde_json::json!(vid));
                }
                let qdrant_payload = Self::domain_payload_to_qdrant(payload);
                PointStruct::new(point_id, pt.vector, qdrant_payload)
            })
            .collect();

        self.client
            .upsert_points(UpsertPointsBuilder::new(collection, point_structs))
            .await
            .map_err(|e| {
                AppError::internal(format!(
                    "failed to upsert {count} points to Qdrant '{collection}': {e}"
                ))
            })?;

        debug!(collection, count, "upserted points to Qdrant");
        Ok(())
    }

    async fn search(
        &self,
        collection: &str,
        query: Vec<f32>,
        filter: VectorFilter,
        limit: usize,
    ) -> Result<Vec<VectorSearchHit>, AppError> {
        let mut builder =
            SearchPointsBuilder::new(collection, query, limit as u64).with_payload(true);

        if let Some(f) = Self::build_filter(&filter) {
            builder = builder.filter(f);
        }

        let request = builder.build();
        let response =
            self.client.search_points(request).await.map_err(|e| {
                AppError::internal(format!("Qdrant search '{collection}' failed: {e}"))
            })?;

        let hits: Vec<VectorSearchHit> = response
            .result
            .into_iter()
            .map(|pt| {
                let id = hit_vector_id(&pt);
                let payload = qdrant_payload_to_json(pt.payload);
                VectorSearchHit {
                    id,
                    score: pt.score,
                    payload,
                }
            })
            .collect();

        debug!(collection, count = hits.len(), "Qdrant search completed");
        Ok(hits)
    }

    async fn delete_points(&self, collection: &str, ids: Vec<String>) -> Result<(), AppError> {
        if ids.is_empty() {
            return Ok(());
        }

        let point_ids: Vec<PointId> = ids.iter().map(|id| vector_id_to_point_id(id)).collect();

        self.client
            .delete_points(DeletePointsBuilder::new(collection).points(point_ids))
            .await
            .map_err(|e| {
                AppError::internal(format!(
                    "failed to delete points from Qdrant '{collection}': {e}"
                ))
            })?;

        Ok(())
    }
}

// ── Qdrant ↔ JSON conversion ────────────────────────────────────────

fn json_to_qdrant_value(v: serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value {
            kind: Some(qdrant_client::qdrant::value::Kind::NullValue(0)),
        },
        serde_json::Value::Bool(b) => b.into(),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into()
            } else if let Some(f) = n.as_f64() {
                f.into()
            } else {
                n.to_string().into()
            }
        }
        serde_json::Value::String(s) => s.into(),
        serde_json::Value::Array(arr) => {
            let values: Vec<Value> = arr.into_iter().map(json_to_qdrant_value).collect();
            Value {
                kind: Some(qdrant_client::qdrant::value::Kind::ListValue(
                    qdrant_client::qdrant::ListValue { values },
                )),
            }
        }
        serde_json::Value::Object(obj) => {
            let fields: HashMap<String, Value> = obj
                .into_iter()
                .map(|(k, v)| (k, json_to_qdrant_value(v)))
                .collect();
            Value {
                kind: Some(qdrant_client::qdrant::value::Kind::StructValue(
                    qdrant_client::qdrant::Struct { fields },
                )),
            }
        }
    }
}

fn qdrant_value_to_json(v: Value) -> serde_json::Value {
    use qdrant_client::qdrant::value::Kind;
    match v.kind {
        Some(Kind::NullValue(_)) => serde_json::Value::Null,
        Some(Kind::DoubleValue(n)) => serde_json::json!(n),
        Some(Kind::IntegerValue(n)) => serde_json::json!(n),
        Some(Kind::StringValue(s)) => serde_json::json!(s),
        Some(Kind::BoolValue(b)) => serde_json::json!(b),
        Some(Kind::StructValue(s)) => {
            let obj: serde_json::Map<String, serde_json::Value> = s
                .fields
                .into_iter()
                .map(|(k, v)| (k, qdrant_value_to_json(v)))
                .collect();
            serde_json::Value::Object(obj)
        }
        Some(Kind::ListValue(l)) => {
            serde_json::Value::Array(l.values.into_iter().map(qdrant_value_to_json).collect())
        }
        None => serde_json::Value::Null,
    }
}

fn qdrant_payload_to_json(payload: HashMap<String, Value>) -> serde_json::Value {
    let map: serde_json::Map<String, serde_json::Value> = payload
        .into_iter()
        .map(|(k, v)| (k, qdrant_value_to_json(v)))
        .collect();
    serde_json::Value::Object(map)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {

    #[test]
    fn test_fnv_deterministic() {
        let a = super::fnv_hash("rag_chunk:42");
        let b = super::fnv_hash("rag_chunk:42");
        assert_eq!(a, b);
        assert_ne!(a, super::fnv_hash("rag_chunk:43"));
    }

    #[test]
    fn test_fnv_different_types() {
        let rag = super::fnv_hash("rag_chunk:1");
        let mem = super::fnv_hash("user_memory:1");
        let sum = super::fnv_hash("conversation_summary:1");
        assert_ne!(rag, mem);
        assert_ne!(mem, sum);
        assert_ne!(rag, sum);
    }
}
