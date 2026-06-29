use serde::Deserialize;

// ── QdrantConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct QdrantConfig {
    #[serde(default = "default_qdrant_enabled")]
    pub enabled: bool,
    pub url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_qdrant_rag_collection")]
    pub rag_collection: String,
    #[serde(default = "default_qdrant_memory_collection")]
    pub memory_collection: String,
    #[serde(default = "default_qdrant_summary_collection")]
    pub summary_collection: String,
    #[serde(default)]
    pub tunnel: Option<String>,
}

impl Default for QdrantConfig {
    fn default() -> Self {
        Self {
            enabled: default_qdrant_enabled(),
            url: String::new(),
            api_key: None,
            rag_collection: default_qdrant_rag_collection(),
            memory_collection: default_qdrant_memory_collection(),
            summary_collection: default_qdrant_summary_collection(),
            tunnel: None,
        }
    }
}

fn default_qdrant_enabled() -> bool {
    false
}
pub fn default_qdrant_rag_collection() -> String {
    "rag_chunks".into()
}
fn default_qdrant_memory_collection() -> String {
    "user_memories".into()
}
fn default_qdrant_summary_collection() -> String {
    "conversation_summaries".into()
}
