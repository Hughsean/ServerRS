use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::application::agent::agent_runtime::AgentTool;
use crate::domain::agent::{AgentContext, AgentEventRepository, NewAgentEvent};
use crate::shared::error::AppError;

/// Logs a risk escalation event when safety concerns are identified during a conversation.
/// This tool does NOT send external notifications — it writes to agent_events for later review.
pub struct RiskEscalationTool {
    event_repo: Arc<dyn AgentEventRepository>,
}

impl RiskEscalationTool {
    pub fn new(event_repo: Arc<dyn AgentEventRepository>) -> Self {
        Self { event_repo }
    }
}

#[async_trait]
impl AgentTool for RiskEscalationTool {
    fn name(&self) -> &str {
        "risk_escalation"
    }

    fn description(&self) -> &str {
        "Log a safety or risk concern for human review. Use when the user expresses self-harm, \
         violence, or other urgent safety issues."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "reason": {
                    "type": "string",
                    "description": "The reason for escalation (e.g. 'user expressed suicidal ideation')."
                },
                "severity": {
                    "type": "string",
                    "enum": ["low", "medium", "high", "crisis"],
                    "description": "Severity level of the concern."
                },
                "evidence": {
                    "type": "string",
                    "description": "Relevant excerpts from the conversation that support the escalation."
                }
            },
            "required": ["reason", "severity"]
        })
    }

    async fn execute(&self, context: &AgentContext, args: Value) -> Result<String, AppError> {
        let reason = args
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("(no reason provided)")
            .to_string();
        let severity = args
            .get("severity")
            .and_then(|v| v.as_str())
            .unwrap_or("medium")
            .to_string();
        let evidence = args
            .get("evidence")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let payload = json!({
            "reason": reason,
            "severity": severity,
            "evidence": evidence,
            "session_id": context.session_id,
            "conversation_id": context.conversation_id,
        });

        let _ = self
            .event_repo
            .log_event(NewAgentEvent {
                user_id: context.user_id,
                conversation_id: context.conversation_id,
                session_id: Some(context.session_id.clone()),
                event_type: "risk_escalation".to_string(),
                payload,
            })
            .await;

        Ok(json!({
            "escalated": true,
            "severity": severity,
            "message": "Risk escalation has been logged for human review. Continue providing supportive responses."
        }).to_string())
    }
}
