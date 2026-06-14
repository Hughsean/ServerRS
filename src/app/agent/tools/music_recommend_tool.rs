use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::app::agent::agent_runtime::AgentTool;
use crate::domain::agent::AgentContext;
use crate::domain::music::MusicRepository;
use crate::shared::error::AppError;

/// Searches and recommends music tracks based on user mood or preferences.
pub struct MusicRecommendTool {
    music_repo: Arc<dyn MusicRepository>,
}

impl MusicRecommendTool {
    pub fn new(music_repo: Arc<dyn MusicRepository>) -> Self {
        Self { music_repo }
    }
}

#[async_trait]
impl AgentTool for MusicRecommendTool {
    fn name(&self) -> &str {
        "music_recommend"
    }

    fn description(&self) -> &str {
        "Search and recommend music tracks based on mood, genre, or user preferences."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query for music (e.g. 'calm piano', 'jazz', 'uplifting')."
                },
                "top_k": {
                    "type": "integer",
                    "description": "Maximum number of tracks to recommend (default: 5).",
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, _context: &AgentContext, args: Value) -> Result<String, AppError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("relaxing");
        let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(5);

        let (tracks, _total) = self
            .music_repo
            .find_all(None, Some(query.to_string()), top_k, 0)
            .await?;

        let items: Vec<Value> = tracks
            .into_iter()
            .map(|t| {
                json!({
                    "track_id": t.music_id,
                    "title": t.title,
                    "artist": t.artist,
                    "category": t.category,
                    "mood_tags": t.mood_tags,
                })
            })
            .collect();

        Ok(json!({"results": items}).to_string())
    }
}
