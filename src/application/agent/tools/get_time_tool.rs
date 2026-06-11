use async_trait::async_trait;
use chrono::Local;
use serde_json::{Value, json};

use crate::application::agent::agent_runtime::AgentTool;
use crate::domain::agent::AgentContext;
use crate::shared::error::AppError;

pub struct GetTimeTool;

impl GetTimeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GetTimeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentTool for GetTimeTool {
    fn name(&self) -> &str {
        "get_time"
    }

    fn description(&self) -> &str {
        "当用户询问现在几点、今天是几号、星期几、日期、时间等问题时，获取当前日期时间信息。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _context: &AgentContext, _args: Value) -> Result<String, AppError> {
        let now = Local::now().format("%Y-%m-%d %H:%M:%S");
        Ok(format!("当前日期时间为: {}", now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent::ToolDefinition;

    #[test]
    fn name_returns_get_time() {
        let tool = GetTimeTool::new();
        assert_eq!(tool.name(), "get_time");
    }

    #[test]
    fn parameters_is_object_with_empty_required() {
        let tool = GetTimeTool::new();
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"].is_object());
        assert!(params["required"].is_array());
        assert_eq!(params["required"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn execute_returns_time_string() {
        use chrono::NaiveDateTime;

        let tool = GetTimeTool::new();

        let context = AgentContext {
            user_id: 1,
            session_id: "test-session".into(),
            conversation_id: None,
            recent_messages: vec![],
            summary: None,
            memories: vec![],
            rag_chunks: vec![],
            user_profile: None,
            tools: vec![ToolDefinition {
                name: "get_time".into(),
                description: "get current time".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            }],
        };

        let output = tool.execute(&context, serde_json::json!({})).await.unwrap();

        let prefix = "当前日期时间为: ";
        assert!(
            output.starts_with(prefix),
            "output should start with '{prefix}', got: {output}"
        );

        let time_part = output.trim_start_matches(prefix);
        NaiveDateTime::parse_from_str(time_part, "%Y-%m-%d %H:%M:%S")
            .expect("time should match yyyy-MM-dd HH:mm:ss");
    }
}
