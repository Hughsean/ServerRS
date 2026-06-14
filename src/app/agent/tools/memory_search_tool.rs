use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::app::agent::agent_runtime::AgentTool;
use crate::app::memory::memory_service::MemoryService;
use crate::domain::agent::AgentContext;
use crate::shared::error::AppError;

/// Searches user memories via `MemoryService` (which prefers Qdrant when configured).
pub struct MemorySearchTool {
    memory_service: Arc<MemoryService>,
}

impl MemorySearchTool {
    pub fn new(memory_service: Arc<MemoryService>) -> Self {
        Self { memory_service }
    }
}

#[async_trait]
impl AgentTool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search the user's long-term memories for relevant facts, preferences, or past interactions."
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

    async fn execute(&self, context: &AgentContext, args: Value) -> Result<String, AppError> {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(5) as u32;

        if query.is_empty() {
            return Ok(json!({"error": "query is required"}).to_string());
        }

        let memories = self
            .memory_service
            .recall(context.user_id, query, top_k)
            .await?;

        let items: Vec<Value> = memories
            .into_iter()
            .map(|m| {
                json!({ "memory_id": m.memory_id, "memory_type": m.memory_type, "content": m.content, "confidence": m.confidence })
            })
            .collect();

        Ok(json!({"results": items}).to_string())
    }
}
