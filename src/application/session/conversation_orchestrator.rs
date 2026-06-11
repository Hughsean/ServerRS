use std::sync::Arc;

use tracing::info;

use crate::domain::conversation::conversation::NewConversation;
use crate::domain::conversation::conversation_message::NewConversationMessage;
use crate::domain::conversation::conversation_repository::ConversationRepository;
use crate::domain::llm::{ChatMessage, LlmClient, PromptProvider};
use crate::domain::tasks::task_event::{ConversationLifecycleTask, TaskEvent};
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::domain::user::user_profile_repository::UserProfileRepository;
use crate::shared::error::AppError;

/// Orchestrates LLM conversations: persona building, chat, title generation, message persistence.
/// Separated from SessionManager to keep session lifecycle and LLM orchestration apart.
pub struct ConversationOrchestrator {
    task_publisher: Arc<dyn TaskPublisher>,
    llm: Arc<dyn LlmClient>,
    prompt_provider: Arc<dyn PromptProvider>,
    conversation_repo: Arc<dyn ConversationRepository>,
    user_profile_repo: Arc<dyn UserProfileRepository>,
}

pub struct MessageResult {
    pub reply: String,
    pub session_closed: bool,
    pub dialogue_id: Option<u64>,
    pub title: Option<String>,
}

impl ConversationOrchestrator {
    pub fn new(
        task_publisher: Arc<dyn TaskPublisher>,
        llm: Arc<dyn LlmClient>,
        prompt_provider: Arc<dyn PromptProvider>,
        conversation_repo: Arc<dyn ConversationRepository>,
        user_profile_repo: Arc<dyn UserProfileRepository>,
    ) -> Self {
        Self {
            task_publisher,
            llm,
            prompt_provider,
            conversation_repo,
            user_profile_repo,
        }
    }

    pub async fn build_persona(
        &self,
        user_id: u64,
        location: Option<&std::collections::HashMap<String, serde_json::Value>>,
        date_time: &str,
    ) -> String {
        let base_prompt = self.prompt_provider.get_prompt(date_time);

        let profile = self
            .user_profile_repo
            .find_by_user_id(user_id)
            .await
            .ok()
            .flatten();

        let mut parts = Vec::new();
        if let Some(ref p) = profile {
            if let Ok(json) = serde_json::to_string(p) {
                parts.push(format!("以下是该用户的画像信息（JSON）：\n{json}\n请在后续对话中遵循用户的兴趣、偏好与情绪倾向，提供更契合用户的回答。"));
            }
        }
        if let Some(loc) = location {
            if let Ok(json) = serde_json::to_string(loc) {
                parts.push(format!(
                    "\n[用户地理位置]\n{json}\n请根据用户所在地区，提供更有针对性的建议和本地化内容。"
                ));
            }
        }

        let persona = parts.join("\n");
        if persona.is_empty() {
            base_prompt
        } else {
            format!("{base_prompt}\n\n[个性化画像]\n{persona}")
        }
    }

    pub async fn ensure_conversation(
        &self,
        user_id: u64,
        current_dialogue_id: Option<u64>,
    ) -> Result<u64, AppError> {
        if let Some(did) = current_dialogue_id {
            self.validate_conversation_access(user_id, did).await?;
            return Ok(did);
        }
        let conv = self
            .conversation_repo
            .save(NewConversation {
                user_id,
                title: None,
            })
            .await?;
        info!(
            dialogue_id = conv.id,
            user_id, "created database conversation"
        );
        let _ = self
            .task_publisher
            .publish(TaskEvent::ConversationCreated(ConversationLifecycleTask {
                conversation_id: conv.id,
                user_id,
            }))
            .await;
        Ok(conv.id)
    }

    pub async fn validate_conversation_access(
        &self,
        user_id: u64,
        dialogue_id: u64,
    ) -> Result<(), AppError> {
        let conv = self
            .conversation_repo
            .find_by_id(dialogue_id)
            .await?
            .ok_or_else(|| AppError::NotFound("conversation not found".into()))?;

        if conv.user_id != user_id {
            return Err(AppError::Forbidden("not your conversation".into()));
        }

        Ok(())
    }

    pub async fn chat(&self, messages: &[ChatMessage]) -> String {
        self.llm.chat(messages).await
    }

    pub async fn generate_title(&self, conv_id: u64, messages: &[ChatMessage]) -> Option<String> {
        let synopsis: String = messages
            .iter()
            .filter(|m| m.role == "user" || m.role == "assistant")
            .map(|m| {
                let t: String = m.content.chars().take(160).collect();
                format!("[{}] {}\n", m.role, t)
            })
            .collect();

        let prompt = format!(
            "你是一名对话摘要分析专家，负责为完整的对话历史生成一个聚焦主题的标题。\n\
             请阅读我提供的对话内容，根据核心事件或议题产出标题。\n\n\
             写作要求：\n1. 使用简洁的陈述句式。\n2. 标题建议 6-16 个中文字符。\n\
             对话摘要：\n{synopsis}\n\n只输出标题本身"
        );

        let title_msgs = vec![
            ChatMessage {
                role: "system".into(),
                content: "生成中文短标题".into(),
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
        ];

        let raw = self.llm.chat(&title_msgs).await;
        let title: String = raw
            .chars()
            .filter(|c| {
                !matches!(
                    c,
                    '"' | '，' | '。' | '！' | '？' | ',' | '.' | '!' | '?' | ':'
                )
            })
            .take(16)
            .collect();

        if !title.is_empty() {
            let _ = self.conversation_repo.update_title(conv_id, &title).await;
            return Some(title);
        }
        None
    }

    pub async fn save_message(
        &self,
        conv_id: u64,
        sender_role: &str,
        sender_user_id: Option<u64>,
        content: &serde_json::Value,
    ) {
        let _ = self
            .conversation_repo
            .save_message(NewConversationMessage {
                conversation_id: conv_id,
                sender_role: sender_role.into(),
                sender_user_id,
                message_type: "text".into(),
                content: content.to_string(),
                token_count: None,
            })
            .await;
    }

    pub async fn touch_and_incr(&self, conv_id: u64, inc: i32) -> Result<(), AppError> {
        self.conversation_repo.touch_and_incr(conv_id, inc).await
    }
}
