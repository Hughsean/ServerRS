use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use chrono::Local;
use tokio::sync::RwLock;
use tracing::info;
use uuid::Uuid;

use super::conversation_orchestrator::{ConversationOrchestrator, MessageResult};
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

#[derive(Debug, Clone)]
pub struct SessionStatus {
    pub id: String,
    pub user_id: u64,
    pub dialogue_id: Option<u64>,
    pub timeout_seconds: u64,
}

struct SessionSnapshot {
    id: String,
    prompt: String,
    messages: Vec<ChatMessage>,
    user_id: u64,
    dialogue_id: Option<u64>,
    is_first_turn: bool,
}

impl SessionManager {
    pub fn new(
        task_publisher: Arc<dyn TaskPublisher>,
        orchestrator: Arc<ConversationOrchestrator>,
        agent_runtime: Arc<AgentRuntime>,
        timeout_seconds: u64,
    ) -> Self {
        Self {
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
        if let Some(did) = dialogue_id {
            self.orchestrator
                .validate_conversation_access(user_id, did)
                .await?;
        }

        // Reuse existing session for this dialogue if still alive
        if let Some(did) = dialogue_id {
            let sessions = self.sessions.read().await;
            for s in sessions.values() {
                if s.user_id == user_id
                    && s.dialogue_id == Some(did)
                    && !s.is_expired(self.timeout_seconds)
                {
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

    pub fn timeout_seconds(&self) -> u64 {
        self.timeout_seconds
    }

    /// Process a user message through the AgentRuntime.
    ///
    /// Flow:
    /// 1. Ensure the session is alive
    /// 2. Ensure a DB conversation row exists
    /// 3. Delegate to `AgentRuntime::respond` (which handles safety, LLM, tools, persist)
    /// 4. Generate title on first turn
    pub async fn process_message(
        &self,
        requesting_user_id: u64,
        session_id: &str,
        text: &str,
        emotion: Option<&str>,
    ) -> Result<Option<MessageResult>, AppError> {
        let snapshot = {
            let mut sessions = self.sessions.write().await;
            let state = match sessions.get_mut(session_id) {
                Some(s) if !s.is_expired(self.timeout_seconds) => s,
                _ => {
                    sessions.remove(session_id);
                    return Ok(None);
                }
            };

            if state.user_id != requesting_user_id {
                return Err(AppError::Forbidden("not your session".into()));
            }

            state.last_active = Instant::now();
            SessionSnapshot {
                id: state.id.clone(),
                prompt: state.prompt.clone(),
                messages: state.messages.clone(),
                user_id: state.user_id,
                dialogue_id: state.dialogue_id,
                is_first_turn: !state.messages.iter().any(|m| m.role == "user"),
            }
        };

        let mut turn_messages = snapshot.messages.clone();
        turn_messages.push(ChatMessage {
            role: "user".into(),
            content: text.to_string(),
            tool_calls: None,
            tool_call_id: None,
        });

        // Ensure DB conversation exists
        let conv_id = self
            .orchestrator
            .ensure_conversation(snapshot.user_id, snapshot.dialogue_id)
            .await?;

        {
            let mut sessions = self.sessions.write().await;
            if let Some(state) = sessions.get_mut(session_id) {
                if state.user_id == requesting_user_id && !state.is_expired(self.timeout_seconds) {
                    state.dialogue_id = Some(conv_id);
                    state.last_active = Instant::now();
                }
            }
        }

        // ── Delegate to AgentRuntime ──────────────────────────────
        let emotion_owned = emotion.map(|e| e.to_string());
        let response = self
            .agent_runtime
            .respond(
                snapshot.user_id,
                snapshot.id.clone(),
                Some(conv_id),
                text.to_string(),
                emotion_owned,
                None,
                turn_messages.clone(),
                Some(snapshot.prompt.clone()),
            )
            .await?;

        let reply = response.reply;
        let session_closed = response.session_closed;

        let mut completed_messages = turn_messages;
        if !reply.is_empty() {
            completed_messages.push(ChatMessage {
                role: "assistant".into(),
                content: reply.clone(),
                tool_calls: None,
                tool_call_id: None,
            });
        }

        // Title generation on first turn
        let title = if snapshot.is_first_turn {
            self.orchestrator
                .generate_title(conv_id, &completed_messages)
                .await
        } else {
            None
        };

        {
            let mut sessions = self.sessions.write().await;
            if session_closed {
                sessions.remove(session_id);
            } else if let Some(state) = sessions.get_mut(session_id) {
                if state.user_id == requesting_user_id {
                    state.dialogue_id = Some(conv_id);
                    state.messages = completed_messages;
                    state.last_active = Instant::now();
                }
            }
        }

        Ok(Some(MessageResult {
            reply,
            session_closed,
            dialogue_id: Some(conv_id),
            title,
        }))
    }

    pub async fn status(
        &self,
        requesting_user_id: u64,
        session_id: &str,
    ) -> Result<Option<SessionStatus>, AppError> {
        let sessions = self.sessions.read().await;
        let s = match sessions.get(session_id) {
            Some(s) => s,
            None => return Ok(None),
        };
        if s.is_expired(self.timeout_seconds) {
            return Ok(None);
        }
        if s.user_id != requesting_user_id {
            return Err(AppError::Forbidden("not your session".into()));
        }
        Ok(Some(SessionStatus {
            id: s.id.clone(),
            user_id: s.user_id,
            dialogue_id: s.dialogue_id,
            timeout_seconds: self.timeout_seconds,
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
