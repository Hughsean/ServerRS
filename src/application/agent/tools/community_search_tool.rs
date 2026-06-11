use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::application::agent::agent_runtime::AgentTool;
use crate::domain::agent::AgentContext;
use crate::domain::community::CommunityRepository;
use crate::shared::error::AppError;

/// Searches community posts for relevant peer experiences and support.
pub struct CommunitySearchTool {
    community_repo: Arc<dyn CommunityRepository>,
}

impl CommunitySearchTool {
    pub fn new(community_repo: Arc<dyn CommunityRepository>) -> Self {
        Self { community_repo }
    }
}

#[async_trait]
impl AgentTool for CommunitySearchTool {
    fn name(&self) -> &str {
        "community_search"
    }

    fn description(&self) -> &str {
        "Search the community forum for peer experiences, support stories, and discussions."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query for community posts."
                },
                "top_k": {
                    "type": "integer",
                    "description": "Maximum number of posts to return (default: 5).",
                    "default": 5
                }
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

        let all_posts = self.community_repo.list_posts(50, 0).await?;

        let query_lower = query.to_lowercase();
        let posts: Vec<_> = all_posts
            .into_iter()
            .filter(|p| {
                p.title
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&query_lower)
                    || p.content.to_lowercase().contains(&query_lower)
            })
            .take(top_k as usize)
            .collect();

        let items: Vec<Value> = posts
            .into_iter()
            .map(|p| {
                json!({
                    "post_id": p.post_id,
                    "title": p.title,
                    "content_preview": p.content.chars().take(200).collect::<String>(),
                    "status": p.status.as_str(),
                })
            })
            .collect();

        Ok(json!({"results": items}).to_string())
    }
}
