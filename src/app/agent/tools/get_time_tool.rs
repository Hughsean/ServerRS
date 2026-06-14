use async_trait::async_trait;
use chrono::{Datelike, Local};
use serde_json::{Value, json};

use crate::app::agent::agent_runtime::AgentTool;
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
        let now = Local::now();
        let weekday = match now.weekday() {
            chrono::Weekday::Mon => "星期一",
            chrono::Weekday::Tue => "星期二",
            chrono::Weekday::Wed => "星期三",
            chrono::Weekday::Thu => "星期四",
            chrono::Weekday::Fri => "星期五",
            chrono::Weekday::Sat => "星期六",
            chrono::Weekday::Sun => "星期日",
        };
        Ok(json!({
            "date": now.format("%Y-%m-%d").to_string(),
            "time": now.format("%H:%M:%S").to_string(),
            "weekday": weekday,
            "timezone": now.format("%:z").to_string(),
            "instruction": "回答日期、时间和星期时必须严格使用这些字段，不得自行换算。"
        })
        .to_string())
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
            conversation_id: None,
            recent_messages: vec![],
            summary: None,
            memories: vec![],
            rag_chunks: vec![],
            user_profile: None,
            location: None,
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

        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        let combined = format!(
            "{} {}",
            value["date"].as_str().unwrap(),
            value["time"].as_str().unwrap()
        );
        NaiveDateTime::parse_from_str(&combined, "%Y-%m-%d %H:%M:%S")
            .expect("time fields should match yyyy-MM-dd HH:mm:ss");
        assert!(value["weekday"].as_str().unwrap().starts_with("星期"));
    }
}
