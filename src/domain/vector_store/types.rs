use serde::{Deserialize, Serialize};

/// Distance metric used for vector similarity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorDistance {
    Cosine,
    Dot,
    Euclid,
}

/// A single point to be upserted into a vector collection.
#[derive(Debug, Clone)]
pub struct VectorPoint {
    pub id: String,
    pub vector: Vec<f32>,
    pub payload: serde_json::Value,
}

/// A filter applied during vector search.
#[derive(Debug, Clone, Default)]
pub struct VectorFilter {
    pub must: Vec<VectorCondition>,
}

impl VectorFilter {
    pub fn new() -> Self {
        Self { must: Vec::new() }
    }

    pub fn with_condition(mut self, condition: VectorCondition) -> Self {
        self.must.push(condition);
        self
    }
}

/// A single filter condition for vector search payloads.
#[derive(Debug, Clone)]
pub enum VectorCondition {
    MatchString { key: String, value: String },
    MatchU64 { key: String, value: u64 },
    MatchI64 { key: String, value: i64 },
    MatchBool { key: String, value: bool },
}

/// A search hit returned by a vector store.
#[derive(Debug, Clone)]
pub struct VectorSearchHit {
    pub id: String,
    pub score: f32,
    pub payload: serde_json::Value,
}
