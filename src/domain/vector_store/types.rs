use serde::{Deserialize, Serialize};

/// 用于向量相似性的距离度量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorDistance {
    Cosine,
    Dot,
    Euclid,
}

/// 要更新到向量集合的单个点。
#[derive(Debug, Clone)]
pub struct VectorPoint {
    pub id: String,
    pub vector: Vec<f32>,
    pub payload: serde_json::Value,
}

/// 向量搜索时应用的过滤器。
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

/// 向量搜索 payload 的单个过滤条件。
#[derive(Debug, Clone)]
pub enum VectorCondition {
    MatchString { key: String, value: String },
    MatchU64 { key: String, value: u64 },
    MatchI64 { key: String, value: i64 },
    MatchBool { key: String, value: bool },
}

/// 向量存储返回的搜索结果项。
#[derive(Debug, Clone)]
pub struct VectorSearchHit {
    pub id: String,
    pub score: f32,
    pub payload: serde_json::Value,
}
