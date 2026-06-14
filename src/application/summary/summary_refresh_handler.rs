use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::domain::conversation::conversation_repository::ConversationRepository;
use crate::domain::llm::{ChatCompletionRequest, ChatMessage, LlmProvider};
use crate::domain::memory::{NewSummary, ROLLING_GENERAL_SUMMARY};
use crate::domain::tasks::task_event::{TaskEvent, TurnClosedEvent};
use crate::domain::tasks::task_handler::TaskHandler;
use crate::domain::user::user_context_version::UserContextVersionRepository;

use super::summary_service::SummaryService;

const MIN_NEW_MESSAGES: usize = 6;

pub struct SummaryRefreshHandler {
    enabled: bool,
    llm: Arc<dyn LlmProvider>,
    conversation_repo: Arc<dyn ConversationRepository>,
    summary_service: Arc<SummaryService>,
    context_version_repo: Arc<dyn UserContextVersionRepository>,
    user_locks: DashMap<u64, Arc<Mutex<()>>>,
}

impl SummaryRefreshHandler {
    pub fn new(
        enabled: bool,
        llm: Arc<dyn LlmProvider>,
        conversation_repo: Arc<dyn ConversationRepository>,
        summary_service: Arc<SummaryService>,
        context_version_repo: Arc<dyn UserContextVersionRepository>,
    ) -> Self {
        Self {
            enabled,
            llm,
            conversation_repo,
            summary_service,
            context_version_repo,
            user_locks: DashMap::new(),
        }
    }

    fn user_lock(&self, user_id: u64) -> Arc<Mutex<()>> {
        self.user_locks
            .entry(user_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .value()
            .clone()
    }

    async fn refresh(&self, event: &TurnClosedEvent) {
        let lock = self.user_lock(event.user_id);
        let Ok(_guard) = lock.try_lock() else {
            debug!(
                user_id = event.user_id,
                "summary refresh already running; skipping"
            );
            return;
        };
        let task_epoch = match self.context_version_repo.get_or_create(event.user_id).await {
            Ok(version) => version.version,
            Err(error) => {
                warn!(user_id = event.user_id, %error, "failed to load summary task epoch");
                return;
            }
        };

        let previous = match self
            .summary_service
            .latest_rolling_general(event.conversation_id)
            .await
        {
            Ok(summary) => summary,
            Err(error) => {
                warn!(
                    conversation_id = event.conversation_id,
                    %error,
                    "failed to load rolling summary"
                );
                return;
            }
        };
        let since_id = previous
            .as_ref()
            .map(|summary| summary.message_end_id.saturating_add(1))
            .unwrap_or(0);
        let messages = match self
            .conversation_repo
            .find_messages_since(event.conversation_id, since_id)
            .await
        {
            Ok(messages) => messages,
            Err(error) => {
                warn!(
                    conversation_id = event.conversation_id,
                    %error,
                    "failed to load messages for summary"
                );
                return;
            }
        };
        let dialogue: Vec<_> = messages
            .iter()
            .filter(|message| message.sender_role == "user" || message.sender_role == "assistant")
            .collect();
        if dialogue.len() < MIN_NEW_MESSAGES {
            return;
        }

        let transcript = dialogue
            .iter()
            .map(|message| {
                format!(
                    "{}: {}",
                    message.sender_role,
                    message_text(&message.content)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let previous_context = previous
            .as_ref()
            .map(|summary| format!("\nPrevious rolling general summary:\n{}\n", summary.content))
            .unwrap_or_default();
        let prompt = summary_prompt(&previous_context, &transcript);
        let request = ChatCompletionRequest {
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: "You write concise rolling conversation summaries.".into(),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                },
                ChatMessage {
                    role: "user".into(),
                    content: prompt,
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                },
            ],
            temperature: 0.2,
            top_p: 1.0,
            max_tokens: Some(512),
            tools: None,
        };
        let summary = match self.llm.chat(request).await {
            Ok(response) => response.content.trim().to_string(),
            Err(error) => {
                warn!(
                    conversation_id = event.conversation_id,
                    %error,
                    "failed to generate conversation summary"
                );
                return;
            }
        };
        if summary.is_empty() {
            return;
        }
        let (Some(first_message), Some(last_message)) = (dialogue.first(), dialogue.last()) else {
            return;
        };
        match self.context_version_repo.get_or_create(event.user_id).await {
            Ok(version) if version.version == task_epoch => {}
            Ok(_) => {
                debug!(
                    user_id = event.user_id,
                    "summary refresh discarded because context version changed"
                );
                return;
            }
            Err(error) => {
                warn!(user_id = event.user_id, %error, "failed to recheck summary task epoch");
                return;
            }
        }
        let token_count = Some(summary.split_whitespace().count().min(u32::MAX as usize) as u32);
        if let Err(error) = self
            .summary_service
            .save_summary(NewSummary {
                conversation_id: event.conversation_id,
                user_id: event.user_id,
                summary_type: ROLLING_GENERAL_SUMMARY.into(),
                content: summary,
                message_start_id: previous
                    .as_ref()
                    .map(|summary| summary.message_start_id)
                    .unwrap_or(first_message.id),
                message_end_id: last_message.id,
                token_count,
            })
            .await
        {
            warn!(
                conversation_id = event.conversation_id,
                %error,
                "failed to save conversation summary"
            );
        }
    }
}

#[async_trait]
impl TaskHandler for SummaryRefreshHandler {
    async fn handle(&self, event: &TaskEvent) {
        if !self.enabled {
            return;
        }
        if let TaskEvent::TurnClosed(event) = event {
            self.refresh(event).await;
        }
    }

    fn name(&self) -> &str {
        "SummaryRefreshHandler"
    }
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

fn summary_prompt(previous_context: &str, transcript: &str) -> String {
    format!(
        "Summarize this conversation for future continuity. Keep it concise and factual.\n\
         Include user concerns, stable preferences, current goals, unresolved topics, \
         and ordinary context useful for continuity.\n\
         Do NOT include risk labels, crisis signals, safety judgments, self-harm risk \
         analysis, clinical diagnosis, or personality disorder labels. If sensitive or \
         safety-related material appears, retain only ordinary conversational context \
         without classification or safety labels.\n\
         Merge the previous rolling summary with the new messages when a previous summary \
         is provided.{previous_context}\nNew messages:\n{transcript}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_is_general_only() {
        let prompt = summary_prompt("", "user: hello");
        assert!(prompt.contains("stable preferences"));
        assert!(prompt.contains("Do NOT include risk labels"));
        assert!(!prompt.contains("Include safety"));
    }

    #[test]
    fn message_text_reads_json_text() {
        assert_eq!(message_text(r#"{"text":"hello"}"#), "hello");
        assert_eq!(message_text("plain"), "plain");
    }
}
