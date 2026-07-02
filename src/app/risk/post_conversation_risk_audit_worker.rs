use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tracing::{debug, warn};

use crate::domain::conversation::conversation_message::ConversationMessage;
use crate::domain::conversation::conversation_repo::ConversationRepoT;
use crate::domain::tasks::task_event::{TaskEvent, TurnClosedEvent};
use crate::domain::tasks::task_handler::TaskHandler;

use super::risk_detection_service::RiskDetectionService;

pub struct PostConversationRiskAuditWorker {
    conversation_repo: Arc<dyn ConversationRepoT>,
    risk_detection_service: Arc<RiskDetectionService>,
}

impl PostConversationRiskAuditWorker {
    pub fn new(
        conversation_repo: Arc<dyn ConversationRepoT>,
        risk_detection_service: Arc<RiskDetectionService>,
    ) -> Self {
        Self {
            conversation_repo,
            risk_detection_service,
        }
    }

    async fn handle_turn_closed(&self, event: &TurnClosedEvent) {
        let (Some(user_message_id), Some(assistant_message_id)) =
            (event.user_message_id, event.assistant_message_id)
        else {
            warn!(
                user_id = event.user_id,
                conversation_id = event.conversation_id,
                "risk audit skipped because persisted turn ids are missing"
            );
            return;
        };

        let messages = match self
            .conversation_repo
            .find_messages_by_ids(
                event.conversation_id,
                &[user_message_id, assistant_message_id],
            )
            .await
        {
            Ok(messages) => messages,
            Err(error) => {
                warn!(
                    user_id = event.user_id,
                    conversation_id = event.conversation_id,
                    %error,
                    "failed to load persisted turn for risk audit"
                );
                return;
            }
        };
        let Some(user_message) = find_message(&messages, user_message_id, "user") else {
            debug!(
                user_id = event.user_id,
                conversation_id = event.conversation_id,
                "risk audit skipped because user message no longer exists"
            );
            return;
        };
        let Some(assistant_message) = find_message(&messages, assistant_message_id, "assistant")
        else {
            debug!(
                user_id = event.user_id,
                conversation_id = event.conversation_id,
                "risk audit skipped because assistant message no longer exists"
            );
            return;
        };

        let canonical_input = json!({
            "conversation_id": event.conversation_id,
            "user_message": {
                "id": user_message.id,
                "text": message_text(&user_message.content),
            },
            "assistant_message": {
                "id": assistant_message.id,
                "text": message_text(&assistant_message.content),
            }
        })
        .to_string();

        if let Err(error) = self
            .risk_detection_service
            .audit_closed_turn(
                event.user_id,
                event.conversation_id,
                user_message_id,
                assistant_message_id,
                canonical_input,
            )
            .await
        {
            warn!(
                user_id = event.user_id,
                conversation_id = event.conversation_id,
                %error,
                "post-conversation risk audit failed"
            );
        }
    }
}

#[async_trait]
impl TaskHandler for PostConversationRiskAuditWorker {
    async fn handle(&self, event: &TaskEvent) {
        if let TaskEvent::TurnClosed(event) = event {
            self.handle_turn_closed(event).await;
        }
    }

    fn name(&self) -> &str {
        "PostConversationRiskAuditWorker"
    }
}

fn find_message(
    messages: &[ConversationMessage],
    message_id: u64,
    expected_role: &str,
) -> Option<ConversationMessage> {
    messages
        .iter()
        .find(|message| message.id == message_id && message.sender_role == expected_role)
        .cloned()
}

fn message_text(content: &str) -> String {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|value| {
            value
                .get("text")
                .and_then(|text| text.as_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| content.to_string())
}

#[cfg(test)]
mod tests {
    use super::message_text;

    #[test]
    fn message_text_extracts_json_text() {
        assert_eq!(message_text(r#"{"text":"hello"}"#), "hello");
        assert_eq!(message_text("plain"), "plain");
    }
}
