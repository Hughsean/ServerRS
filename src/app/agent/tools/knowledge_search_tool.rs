use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::app::agent::agent_runtime::AgentTool;
use crate::app::rag::retrieval_service::RetrievalService;
use crate::domain::agent::AgentContext;
use crate::shared::error::AppError;

/// Searches the knowledge base for relevant chunks via `RetrievalService`
/// (which prefers Qdrant when configured).
pub struct KnowledgeSearchTool {
    retrieval_service: Arc<RetrievalService>,
}

impl KnowledgeSearchTool {
    pub fn new(retrieval_service: Arc<RetrievalService>) -> Self {
        Self { retrieval_service }
    }
}

#[async_trait]
impl AgentTool for KnowledgeSearchTool {
    fn name(&self) -> &str {
        "knowledge_search"
    }

    fn description(&self) -> &str {
        "Search the mental-health knowledge base for relevant information about a topic or question."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query." },
                "top_k": { "type": "integer", "description": "Max results (default: 5).", "default": 5 }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, _context: &AgentContext, args: Value) -> Result<String, AppError> {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(5);

        if query.is_empty() {
            return Ok(json!({"error": "query is required"}).to_string());
        }

        let results = self
            .retrieval_service
            .retrieve(query, _context.user_id, top_k)
            .await?;

        if results.is_empty() {
            return Ok(
                json!({"results": [], "message": "No relevant knowledge found."}).to_string(),
            );
        }

        let items: Vec<Value> = results
            .into_iter()
            .map(|(chunk, score)| {
                json!({
                    "chunk_id": chunk.chunk_id, "document_id": chunk.document_id,
                    "content": chunk.content, "relevance_score": score,
                })
            })
            .collect();

        Ok(json!({"results": items}).to_string())
    }
}
