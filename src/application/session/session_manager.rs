use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use chrono::Local;
use tokio::sync::RwLock;
use tracing::info;
use uuid::Uuid;

use super::conversation_orchestrator::{ConversationOrchestrator, MessageResult};
use super::risk_detection_service::RiskDetectionService;
use crate::application::agent::agent_runtime::AgentRuntime;
use crate::domain::llm::ChatMessage;
use crate::domain::tasks::task_event::{SessionLifecycleTask, TaskEvent};
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::shared::error::AppError;

/// Manages in-memory session state and routes messages through AgentRuntime.
///
/// SessionManager owns session lifecycle (create, status, cleanup) and delegates
/// the actual message processing (safety check, LLM call, tool execution, RAG,
/// memory extraction) to `AgentRuntime`.
///
/// `ConversationOrchestrator` is retained for:
///   - `ensure_conversation` (creating the DB conversation row)
///   - `generate_title` (first-turn title generation)
///   - `save_message` / `touch_and_incr` (persisting user/assistant messages)
///   - `build_persona` (system prompt generation)
///
/// Message persistence is done by `AgentRuntime::persist_messages` internally;
/// the orchestrator is used here for pre-AgentRuntime tasks only.
pub struct SessionManager {
    task_publisher: Arc<dyn TaskPublisher>,
    risk_detection: Arc<RiskDetectionService>,
    orchestrator: Arc<ConversationOrchestrator>,
    agent_runtime: Arc<AgentRuntime>,
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
        agent_runtime: Arc<AgentRuntime>,
        timeout_seconds: u64,
    ) -> Self {
        Self {
            risk_detection,
            task_publisher,
            orchestrator,
            agent_runtime,
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

    /// Process a user message through the AgentRuntime.
    ///
    /// Flow:
    /// 1. Ensure the session is alive
    /// 2. Ensure a DB conversation row exists
    /// 3. Fire async risk detection
    /// 4. Delegate to `AgentRuntime::respond` (which handles safety, LLM, tools, persist)
    /// 5. Generate title on first turn
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

        let is_first_turn = !state.messages.iter().any(|m| m.role == "user");
        state.messages.push(ChatMessage {
            role: "user".into(),
            content: text.to_string(),
            tool_calls: None,
            tool_call_id: None,
        });

        // Ensure DB conversation exists
        let conv_id = self
            .orchestrator
            .ensure_conversation(state.user_id, state.dialogue_id)
            .await?;
        state.dialogue_id = Some(conv_id);

        // Async risk detection
        let rd = Arc::clone(&self.risk_detection);
        let text_owned = text.to_string();
        let uid = state.user_id;
        let cid = Some(conv_id);
        tokio::spawn(async move {
            rd.detect_and_save(&text_owned, uid, cid, None).await;
        });

        // ── Delegate to AgentRuntime ──────────────────────────────
        let emotion_owned = emotion.map(|e| e.to_string());
        let response = self
            .agent_runtime
            .respond(
                state.user_id,
                state.id.clone(),
                Some(conv_id),
                text.to_string(),
                emotion_owned,
                None, // location
            )
            .await;

        let reply = response.reply;
        let session_closed = response.session_closed;

        if !reply.is_empty() {
            state.messages.push(ChatMessage {
                role: "assistant".into(),
                content: reply.clone(),
                tool_calls: None,
                tool_call_id: None,
            });
        }

        // Title generation on first turn
        let title = if is_first_turn {
            self.orchestrator
                .generate_title(conv_id, &state.messages)
                .await
        } else {
            None
        };

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
