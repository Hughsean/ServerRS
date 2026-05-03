use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use chrono::Local;
use tokio::sync::RwLock;
use tracing::info;
use uuid::Uuid;

use super::conversation_orchestrator::{ConversationOrchestrator, MessageResult};
use super::risk_detection_service::RiskDetectionService;
use super::tool_calling::ToolCallService;
use crate::domain::llm::ChatMessage;
use crate::domain::llm::tools::ToolExecutionContext;
use crate::domain::tasks::task_event::{SessionLifecycleTask, TaskEvent};
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::shared::error::AppError;

/// Manages in-memory session state and routes messages to the ConversationOrchestrator.
/// Focused on session lifecycle: create, process, status, cleanup.
pub struct SessionManager {
    task_publisher: Arc<dyn TaskPublisher>,
    risk_detection: Arc<RiskDetectionService>,
    orchestrator: Arc<ConversationOrchestrator>,
    tool_service: Arc<ToolCallService>,
    sessions: RwLock<HashMap<String, SessionState>>,
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
    pub tool_context: ToolExecutionContext,
}

impl SessionState {
    pub fn is_expired(&self, timeout_secs: u64) -> bool {
        self.last_active.elapsed().as_secs() > timeout_secs
    }
}

impl SessionManager {
    pub fn new(
        task_publisher: Arc<dyn TaskPublisher>,
        risk_detection: Arc<RiskDetectionService>,
        orchestrator: Arc<ConversationOrchestrator>,
        tool_service: Arc<ToolCallService>,
        timeout_seconds: u64,
    ) -> Self {
        Self {
            risk_detection,
            task_publisher,
            orchestrator,
            tool_service,
            sessions: RwLock::new(HashMap::new()),
            timeout_seconds,
        }
    }

    pub async fn create(
        &self,
        user_id: u64,
        dialogue_id: Option<u64>,
        location: Option<&HashMap<String, serde_json::Value>>,
    ) -> Result<SessionState, AppError> {
        // Reuse existing session for this dialogue if still alive
        if let Some(did) = dialogue_id {
            let sessions = self.sessions.read().await;
            for s in sessions.values() {
                if s.dialogue_id == Some(did) && !s.is_expired(self.timeout_seconds) {
                    return Ok(s.clone());
                }
            }
        }

        let date_time = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let final_prompt = self
            .orchestrator
            .build_persona(user_id, location, &date_time)
            .await;

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
            tool_context: ToolExecutionContext::default(),
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

        // Ensure DB conversation exists
        let conv_id = self
            .orchestrator
            .ensure_conversation(state.user_id, state.dialogue_id)
            .await?;
        state.dialogue_id = Some(conv_id);

        // Persist user message
        let user_content =
            serde_json::json!({ "text": text, "composed": composed, "emotion": emotion });
        self.orchestrator
            .save_message(conv_id, "user", Some(state.user_id), &user_content)
            .await;

        // Async risk detection
        let rd = Arc::clone(&self.risk_detection);
        let text_owned = text.to_string();
        let uid = state.user_id;
        let cid = Some(conv_id);
        tokio::spawn(async move {
            rd.detect_and_save(&text_owned, uid, cid, None).await;
        });

        // Title generation on first turn
        let title = if is_first_turn {
            self.orchestrator
                .generate_title(conv_id, &state.messages)
                .await
        } else {
            None
        };

        // LLM chat with tool calls
        let tool_result = self
            .tool_service
            .chat_with_tools(&mut state.messages, &mut state.tool_context)
            .await;
        let reply = tool_result.reply;
        let session_closed = tool_result.exit_requested;

        if !reply.is_empty() {
            state.messages.push(ChatMessage {
                role: "assistant".into(),
                content: reply.clone(),
                tool_calls: None,
                tool_call_id: None,
            });

            let asst_content = serde_json::json!({ "text": reply });
            self.orchestrator
                .save_message(conv_id, "assistant", None, &asst_content)
                .await;

            let _ = self.orchestrator.touch_and_incr(conv_id, 2).await;
        }

        Ok(Some(MessageResult {
            reply,
            session_closed,
            dialogue_id: Some(conv_id),
            title,
        }))
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
