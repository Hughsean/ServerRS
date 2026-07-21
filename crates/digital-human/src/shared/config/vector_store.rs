use serde::Deserialize;

// ── VectorStoreConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct VectorStoreConfig {
    #[serde(default = "default_vector_store_enabled")]
    pub enabled: bool,
    pub url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_rag_index_name")]
    pub rag_index_name: String,
    #[serde(default = "default_memory_index_name")]
    pub memory_index_name: String,
    #[serde(default = "default_summary_index_name")]
    pub summary_index_name: String,
    #[serde(default)]
    pub tunnel: Option<String>,
}

impl Default for VectorStoreConfig {
    fn default() -> Self {
        Self {
            enabled: default_vector_store_enabled(),
            url: String::new(),
            api_key: None,
            rag_index_name: default_rag_index_name(),
            memory_index_name: default_memory_index_name(),
            summary_index_name: default_summary_index_name(),
            tunnel: None,
        }
    }
}

fn default_vector_store_enabled() -> bool {
    false
}
pub fn default_rag_index_name() -> String {
    "rag_chunks".into()
}
fn default_memory_index_name() -> String {
    "user_memories".into()
}
fn default_summary_index_name() -> String {
    "conversation_summaries".into()
}
