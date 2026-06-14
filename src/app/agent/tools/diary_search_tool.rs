use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::app::agent::agent_runtime::AgentTool;
use crate::domain::agent::AgentContext;
use crate::domain::diary::DiaryRepository;
use crate::shared::error::AppError;

pub struct DiarySearchTool {
    diary_repo: Arc<dyn DiaryRepository>,
}

impl DiarySearchTool {
    pub fn new(diary_repo: Arc<dyn DiaryRepository>) -> Self {
        Self { diary_repo }
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
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 20,
                    "description": "Maximum number of diary entries to return."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, context: &AgentContext, args: Value) -> Result<String, AppError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .clamp(1, 20);

        let (diaries, _) = self
            .diary_repo
            .find_by_user_id(context.user_id, 100, 0)
            .await?;
        let mut results = Vec::new();

        for diary in diaries {
            let haystack = format!(
                "{}\n{}\n{}",
                diary.title,
                diary.content,
                diary.mood_description.clone().unwrap_or_default()
            )
            .to_lowercase();

            if query.is_empty() || haystack.contains(&query) {
                let excerpt: String = diary.content.chars().take(240).collect();
                results.push(json!({
                    "id": diary.id,
                    "title": diary.title,
                    "excerpt": excerpt,
                    "moodDescription": diary.mood_description,
                    "createdAt": diary.created_at.to_rfc3339(),
                }));
            }

            if results.len() >= limit as usize {
                break;
            }
        }

        Ok(json!({ "results": results }).to_string())
    }
}
