use async_trait::async_trait;
use serde_json::{Value, json};

use crate::application::agent::agent_runtime::AgentTool;
use crate::domain::agent::AgentContext;
use crate::shared::error::AppError;

/// Placeholder tool for searching user diary entries.
/// Currently returns an empty result set — implementation pending diary search service.
pub struct DiarySearchTool;

impl DiarySearchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AgentTool for DiarySearchTool {
    fn name(&self) -> &str {
        "diary_search"
    }

    fn description(&self) -> &str {
        "Search the user's diary entries for relevant journal content."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query for diary entries."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, _context: &AgentContext, _args: Value) -> Result<String, AppError> {
        Ok(json!({"results": [], "message": "Diary search not yet implemented."}).to_string())
    }
}
