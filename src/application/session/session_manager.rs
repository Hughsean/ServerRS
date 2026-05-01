use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use chrono::Local;
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use super::risk_detection_service::RiskDetectionService;
use crate::domain::conversation::conversation::NewConversation;
use crate::domain::conversation::conversation_message::NewConversationMessage;
use crate::domain::conversation::conversation_repository::ConversationRepository;
use crate::domain::tasks::task_event::{SessionLifecycleTask, TaskEvent};
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::domain::user::user_profile_repository::UserProfileRepository;
use crate::infrastructure::llm::ollama_client::{ChatMessage, OllamaClient};
use crate::infrastructure::llm::prompt_provider::PromptProvider;
use crate::shared::error::AppError;

pub struct SessionManager {
    task_publisher: Arc<dyn TaskPublisher>,
    risk_detection: Arc<RiskDetectionService>,
    sessions: RwLock<HashMap<String, SessionState>>,
    llm: OllamaClient,
    prompt_provider: PromptProvider,
    conversation_repo: Arc<dyn ConversationRepository>,
    user_profile_repo: Arc<dyn UserProfileRepository>,
    timeout_seconds: u64,
}

#[derive(Debug, Clone)]
pub struct SessionState {
    pub id: String,
    pub prompt: String,
    pub messages: Vec<ChatMessage>,
    pub user_id: u64,
    pub dialogue_id: Option<u64>,
    pub last_active: Instant,
}

impl SessionState {
    pub fn is_expired(&self, timeout_secs: u64) -> bool {
        self.last_active.elapsed().as_secs() > timeout_secs
    }
}

pub struct MessageResult {
    pub reply: String,
    pub session_closed: bool,
    pub dialogue_id: Option<u64>,
    pub title: Option<String>,
}

impl SessionManager {
    pub fn new(
        task_publisher: Arc<dyn TaskPublisher>,
        risk_detection: Arc<RiskDetectionService>,
        llm: OllamaClient,
        prompt_provider: PromptProvider,
        conversation_repo: Arc<dyn ConversationRepository>,
        user_profile_repo: Arc<dyn UserProfileRepository>,
        timeout_seconds: u64,
    ) -> Self {
        Self {
            risk_detection,
            task_publisher,
            sessions: RwLock::new(HashMap::new()),
            llm,
            prompt_provider,
            conversation_repo,
            user_profile_repo,
            timeout_seconds,
        }
    }

    pub async fn create(
        &self,
        user_id: u64,
        dialogue_id: Option<u64>,
        location: Option<&HashMap<String, serde_json::Value>>,
    ) -> Result<SessionState, AppError> {
        if let Some(did) = dialogue_id {
            let sessions = self.sessions.read().await;
            for s in sessions.values() {
                if s.dialogue_id == Some(did) && !s.is_expired(self.timeout_seconds) {
                    return Ok(s.clone());
                }
            }
        }
        let persona = self.build_persona(user_id, location).await;
        let date_time = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let base_prompt = self.prompt_provider.get_prompt(&date_time);
        let final_prompt = match persona {
            Some(p) => format!("{base_prompt}\n\n[个性化画像]\n{p}"),
            None => base_prompt,
        };

        let id = Uuid::new_v4().to_string();
        let state = SessionState {
            id: id.clone(),
            prompt: final_prompt.clone(),
            messages: vec![ChatMessage {
                role: "system".into(),
                content: final_prompt,
                tool_calls: None,
                tool_call_id: None,
            }],
            user_id,
            dialogue_id,
            last_active: Instant::now(),
        };

        self.sessions
            .write()
            .await
            .insert(state.id.clone(), state.clone());
        info!(session_id = %state.id, user_id, "session created");
        let _ = self
            .task_publisher
            .publish(TaskEvent::SessionCreated(SessionLifecycleTask {
                session_id: state.id.clone(),
                user_id,
                dialogue_id: state.dialogue_id,
            }))
            .await;
        Ok(state)
    }

    async fn build_persona(
        &self,
        user_id: u64,
        location: Option<&HashMap<String, serde_json::Value>>,
    ) -> Option<String> {
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
                parts.push(format!("\n[用户地理位置]\n{json}\n请根据用户所在地区，提供更有针对性的建议和本地化内容。"));
            }
        }
        let result = parts.join("\n").trim().to_string();
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    pub async fn process_message(
        &self,
        session_id: &str,
        text: &str,
        emotion: Option<&str>,
    ) -> Result<Option<MessageResult>, AppError> {
        let mut sessions = self.sessions.write().await;
        let state = match sessions.get_mut(session_id) {
            Some(s) if !s.is_expired(self.timeout_seconds) => s,
            _ => {
                sessions.remove(session_id);
                return Ok(None);
            }
        };
        state.last_active = Instant::now();

        let composed = match emotion.filter(|e| !e.is_empty()) {
            Some(e) => format!("{text}\n\n[情绪提示] 系统检测到用户当前情绪：{e}"),
            None => format!("{text}\n\n[情绪提示] 前端未提供用户明确情绪。"),
        };
        let is_first_turn = !state.messages.iter().any(|m| m.role == "user");
        state.messages.push(ChatMessage {
            role: "user".into(),
            content: composed.clone(),
            tool_calls: None,
            tool_call_id: None,
        });

        let conv_id = self.ensure_conversation(state).await?;
        let user_content =
            serde_json::json!({ "text": text, "composed": composed, "emotion": emotion });
        let _ = self
            .conversation_repo
            .save_message(NewConversationMessage {
                conversation_id: conv_id,
                sender_role: "user".into(),
                sender_user_id: Some(state.user_id),
                message_type: "text".into(),
                content: user_content.to_string(),
                token_count: None,
            })
            .await;

        let rd = Arc::clone(&self.risk_detection);
        let text_owned = text.to_string();
        let uid = state.user_id;
        let cid = Some(conv_id);
        tokio::spawn(async move {
            rd.detect_and_save(&text_owned, uid, cid, None).await;
        });

        let title = if is_first_turn {
            self.generate_title(conv_id, &state.messages).await
        } else {
            None
        };
        let reply = self.llm.chat(&state.messages).await;

        if !reply.is_empty() {
            state.messages.push(ChatMessage {
                role: "assistant".into(),
                content: reply.clone(),
                tool_calls: None,
                tool_call_id: None,
            });
            let asst_content = serde_json::json!({ "text": reply });
            let _ = self
                .conversation_repo
                .save_message(NewConversationMessage {
                    conversation_id: conv_id,
                    sender_role: "assistant".into(),
                    sender_user_id: None,
                    message_type: "text".into(),
                    content: asst_content.to_string(),
                    token_count: None,
                })
                .await;
            let _ = self.conversation_repo.touch_and_incr(conv_id, 2).await;
        }

        Ok(Some(MessageResult {
            reply,
            session_closed: false,
            dialogue_id: Some(conv_id),
            title,
        }))
    }

    async fn ensure_conversation(&self, state: &mut SessionState) -> Result<u64, AppError> {
        if let Some(did) = state.dialogue_id {
            return Ok(did);
        }
        let conv = self
            .conversation_repo
            .save(NewConversation {
                user_id: state.user_id,
                title: None,
            })
            .await?;
        state.dialogue_id = Some(conv.id);
        info!(
            dialogue_id = conv.id,
            user_id = state.user_id,
            "created database conversation"
        );
        let _ = self
            .task_publisher
            .publish(TaskEvent::ConversationCreated(
                crate::domain::tasks::task_event::ConversationLifecycleTask {
                    conversation_id: conv.id,
                    user_id: state.user_id,
                },
            ))
            .await;
        Ok(conv.id)
    }

    async fn generate_title(&self, conv_id: u64, messages: &[ChatMessage]) -> Option<String> {
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
            },
            ChatMessage {
                role: "user".into(),
                content: prompt,
                tool_calls: None,
                tool_call_id: None,
            },
        ];

        let raw = self.llm.chat(&title_msgs).await;
        // Remove punctuation and quote characters
        let title: String = raw
            .chars()
            .filter(|c| {
                !matches!(
                    c,
                    '"' | '\"' | '，' | '。' | '！' | '？' | ',' | '.' | '!' | '?' | ':'
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

    pub async fn status(&self, session_id: &str) -> Option<serde_json::Value> {
        let sessions = self.sessions.read().await;
        let s = sessions.get(session_id)?;
        if s.is_expired(self.timeout_seconds) {
            return None;
        }
        Some(serde_json::json!({
            "sessionId": s.id, "userId": s.user_id,
            "dialogueId": s.dialogue_id, "timeoutSeconds": self.timeout_seconds,
        }))
    }

    pub async fn cleanup(&self) -> usize {
        let mut sessions = self.sessions.write().await;
        let before = sessions.len();
        sessions.retain(|_, s| !s.is_expired(self.timeout_seconds));
        let removed = before - sessions.len();
        if removed > 0 {
            info!(removed, "cleaned expired sessions");
        }
        removed
    }
}
