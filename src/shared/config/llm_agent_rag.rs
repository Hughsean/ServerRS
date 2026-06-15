use serde::Deserialize;

use super::qdrant::default_qdrant_rag_collection;

// ── LlmConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct LlmConfig {
    #[serde(default = "default_llm_provider")]
    pub provider: String,
    #[serde(default = "default_llm_base_url")]
    pub base_url: String,
    #[serde(default = "default_llm_chat_model")]
    pub chat_model: String,
    #[serde(default = "default_llm_embedding_model")]
    pub embedding_model: String,
    #[serde(default = "default_llm_temperature")]
    pub temperature: f64,
    #[serde(default = "default_llm_top_p")]
    pub top_p: f64,
    #[serde(default = "default_llm_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_llm_max_tool_depth")]
    pub max_tool_depth: u32,
    #[serde(default = "default_llm_enable_reasoning")]
    pub enable_reasoning: bool,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: default_llm_provider(),
            base_url: default_llm_base_url(),
            chat_model: default_llm_chat_model(),
            embedding_model: default_llm_embedding_model(),
            temperature: default_llm_temperature(),
            top_p: default_llm_top_p(),
            timeout_secs: default_llm_timeout_secs(),
            max_tool_depth: default_llm_max_tool_depth(),
            enable_reasoning: default_llm_enable_reasoning(),
        }
    }
}

fn default_llm_provider() -> String {
    "openai".into()
}
fn default_llm_base_url() -> String {
    "http://127.0.0.1:11434/v1".into()
}
fn default_llm_chat_model() -> String {
    "qwen2.5:14b".into()
}
fn default_llm_embedding_model() -> String {
    "bge-m3".into()
}
fn default_llm_temperature() -> f64 {
    0.7
}
fn default_llm_top_p() -> f64 {
    0.9
}
fn default_llm_timeout_secs() -> u64 {
    120
}
fn default_llm_max_tool_depth() -> u32 {
    10
}
fn default_llm_enable_reasoning() -> bool {
    true
}

// ── AgentConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_agent_enabled")]
    pub enabled: bool,
    #[serde(default = "default_agent_memory_enabled")]
    pub memory_enabled: bool,
    #[serde(default = "default_agent_rag_enabled")]
    pub rag_enabled: bool,
    #[serde(default = "default_agent_summary_enabled")]
    pub summary_enabled: bool,
    #[serde(default = "default_agent_max_context_messages")]
    pub max_context_messages: u32,
    #[serde(default = "default_agent_max_memory_items")]
    pub max_memory_items: u32,
    #[serde(default = "default_agent_max_rag_chunks")]
    pub max_rag_chunks: u32,
    #[serde(default = "default_agent_memory_extraction_async")]
    pub memory_extraction_async: bool,
    #[serde(default = "default_agent_summary_async")]
    pub summary_async: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enabled: default_agent_enabled(),
            memory_enabled: default_agent_memory_enabled(),
            rag_enabled: default_agent_rag_enabled(),
            summary_enabled: default_agent_summary_enabled(),
            max_context_messages: default_agent_max_context_messages(),
            max_memory_items: default_agent_max_memory_items(),
            max_rag_chunks: default_agent_max_rag_chunks(),
            memory_extraction_async: default_agent_memory_extraction_async(),
            summary_async: default_agent_summary_async(),
        }
    }
}

fn default_agent_enabled() -> bool {
    false
}
fn default_agent_memory_enabled() -> bool {
    true
}
fn default_agent_rag_enabled() -> bool {
    true
}
fn default_agent_summary_enabled() -> bool {
    true
}
fn default_agent_max_context_messages() -> u32 {
    50
}
fn default_agent_max_memory_items() -> u32 {
    100
}
fn default_agent_max_rag_chunks() -> u32 {
    5
}
fn default_agent_memory_extraction_async() -> bool {
    true
}
fn default_agent_summary_async() -> bool {
    true
}

// ── RagConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct RagConfig {
    #[serde(default = "default_rag_chunk_size")]
    pub chunk_size: usize,
    #[serde(default = "default_rag_chunk_overlap")]
    pub chunk_overlap: usize,
    #[serde(default = "default_rag_top_k")]
    pub top_k: usize,
    #[serde(default = "default_rag_hybrid_vector_weight")]
    pub hybrid_vector_weight: f64,
    #[serde(default = "default_rag_hybrid_keyword_weight")]
    pub hybrid_keyword_weight: f64,
}

impl Default for RagConfig {
    fn default() -> Self {
        Self {
            chunk_size: default_rag_chunk_size(),
            chunk_overlap: default_rag_chunk_overlap(),
            top_k: default_rag_top_k(),
            hybrid_vector_weight: default_rag_hybrid_vector_weight(),
            hybrid_keyword_weight: default_rag_hybrid_keyword_weight(),
        }
    }
}

fn default_rag_chunk_size() -> usize {
    512
}
fn default_rag_chunk_overlap() -> usize {
    64
}
fn default_rag_top_k() -> usize {
    5
}
fn default_rag_hybrid_vector_weight() -> f64 {
    0.7
}
fn default_rag_hybrid_keyword_weight() -> f64 {
    0.3
}

// ── EmbeddingConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingConfig {
    #[serde(default = "default_embedding_provider")]
    pub provider: String,
    #[serde(default = "default_embedding_base_url")]
    pub base_url: String,
    #[serde(default = "default_embedding_model")]
    pub model: String,
    #[serde(default = "default_embedding_api_key")]
    pub api_key: String,
    #[serde(default = "default_embedding_dimension")]
    pub dimension: usize,
    #[serde(default = "default_embedding_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_embedding_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_qdrant_rag_collection")]
    pub qdrant_collection: String,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: default_embedding_provider(),
            base_url: default_embedding_base_url(),
            model: default_embedding_model(),
            api_key: default_embedding_api_key(),
            dimension: default_embedding_dimension(),
            batch_size: default_embedding_batch_size(),
            timeout_secs: default_embedding_timeout_secs(),
            qdrant_collection: default_qdrant_rag_collection(),
        }
    }
}

fn default_embedding_provider() -> String {
    "ollama".into()
}
fn default_embedding_base_url() -> String {
    "http://127.0.0.1:11434".into()
}
fn default_embedding_model() -> String {
    "nomic-embed-text".into()
}
fn default_embedding_api_key() -> String {
    String::new()
}
fn default_embedding_dimension() -> usize {
    768
}
fn default_embedding_batch_size() -> usize {
    32
}
fn default_embedding_timeout_secs() -> u64 {
    120
}
