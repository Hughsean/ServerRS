use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::application::agent::agent_runtime::AgentTool;
use crate::domain::agent::AgentContext;
use crate::domain::depression::DepressionRepository;
use crate::shared::error::AppError;

/// Looks up depression assessment scales for reference.
pub struct DepressionScaleTool {
    depression_repo: Arc<dyn DepressionRepository>,
}

impl DepressionScaleTool {
    pub fn new(depression_repo: Arc<dyn DepressionRepository>) -> Self {
        Self { depression_repo }
    }
}

#[async_trait]
impl AgentTool for DepressionScaleTool {
    fn name(&self) -> &str {
        "depression_scale"
    }

    fn description(&self) -> &str {
        "Retrieve depression scale information or self-assessment references for the user."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What scale information is needed (e.g. 'PHQ-9', 'GAD-7', or 'list')."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, _context: &AgentContext, args: Value) -> Result<String, AppError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("PHQ-9");

        // List all scales and filter locally
        let all_scales = self.depression_repo.list_scales().await.unwrap_or_default();

        let query_lower = query.to_lowercase();
        let scales: Vec<_> = all_scales
            .into_iter()
            .filter(|s| {
                s.scale_name.to_lowercase().contains(&query_lower)
                    || s.scale_description
                        .as_ref()
                        .map(|d| d.to_lowercase().contains(&query_lower))
                        .unwrap_or(false)
            })
            .take(5)
            .collect();

        if scales.is_empty() {
            return Ok(json!({
                "results": [],
                "message": format!("No depression scales found matching '{query}'.")
            })
            .to_string());
        }

        let items: Vec<Value> = scales
            .into_iter()
            .map(|s| {
                let questions_array = &s.questions;
                let question_count = questions_array.as_array().map(|a| a.len()).unwrap_or(0);
                json!({
                    "scale_id": s.scale_id,
                    "name": s.scale_name,
                    "description": s.scale_description,
                    "question_count": question_count,
                })
            })
            .collect();

        Ok(json!({"results": items}).to_string())
    }
}
