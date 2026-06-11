use std::sync::Arc;

use serde_json::Value;
use tracing::{info, warn};

use super::agent_context::AgentContextBuilder;
use crate::application::memory::memory_service::MemoryService;
use crate::application::summary::summary_service::SummaryService;
use crate::domain::agent::{AgentContext, AgentEventRepository, NewAgentEvent};
use crate::domain::conversation::conversation_message::NewConversationMessage;
use crate::domain::conversation::conversation_repository::ConversationRepository;
use crate::domain::llm::{
    ChatCompletionRequest, ChatMessage, LlmProvider, ToolDefinition as LlmToolDef,
};
use crate::domain::memory::NewSummary;
use crate::domain::risk::detection_types::{DetectionResult, RiskLevel};
use crate::domain::risk::risk_detection_result::NewRiskDetectionResult;
use crate::domain::risk::risk_detector::RiskDetector;
use crate::domain::risk::risk_repository::RiskRepository;
use crate::domain::user::user_profile_repository::UserProfileRepository;
use crate::shared::error::AppError;

// ---------------------------------------------------------------------------
// AgentTool trait
// ---------------------------------------------------------------------------

/// A callable tool that the agent runtime can invoke.
///
/// This is intentionally kept separate from `LlmTool` so the agent layer can
/// define tools that have access to the full `AgentContext` (user profile,
/// memories, RAG chunks, etc.) rather than the narrower `ToolExecutionContext`.
#[async_trait::async_trait]
pub trait AgentTool: Send + Sync {
    /// Unique tool name (must match the name in `ToolDefinition`).
    fn name(&self) -> &str;

    /// Human-readable description.
    fn description(&self) -> &str;

    /// JSON Schema describing the accepted arguments.
    fn parameters(&self) -> Value;

    /// Execute the tool.
    ///
    /// `context` provides the full agent context for this turn so the tool
    /// can make decisions based on the user's profile, memories, etc.
    async fn execute(&self, context: &AgentContext, args: Value) -> Result<String, AppError>;
}

// ---------------------------------------------------------------------------
// AgentResponse
// ---------------------------------------------------------------------------

/// Structured result produced by the agent runtime after processing one turn.
#[derive(Debug, Clone)]
pub struct AgentResponse {
    /// The final textual reply sent back to the user.
    pub reply: String,
    /// Trace of every tool that was invoked during this turn.
    pub tool_calls: Vec<ToolTrace>,
    /// Whether the session should be marked as closed.
    pub session_closed: bool,
    /// Whether a safety intervention was triggered.
    pub safety_triggered: bool,
}

/// Record of a single tool invocation within a turn.
#[derive(Debug, Clone)]
pub struct ToolTrace {
    pub tool_name: String,
    pub arguments: Value,
    pub result: String,
}

struct PersistedTurn {
    user_message_id: u64,
}

// ---------------------------------------------------------------------------
// AgentRuntime
// ---------------------------------------------------------------------------

/// High-level agent runtime that orchestrates a single message-response turn.
///
/// Flow:
/// 1. Safety pre-check on the incoming user message.
/// 2. If crisis level is detected, return a safety response immediately
///    (no LLM call, no tool execution).
/// 3. Build `AgentContext` (summary, memories, RAG chunks, profile).
/// 4. Run the LLM with tool definitions.
/// 5. Extract tool calls from the LLM response and execute them.
/// 6. Feed tool results back to the LLM for a final response.
/// 7. Persist messages.
/// 8. Spawn async tasks for memory extraction and risk persistence.
pub struct AgentRuntime {
    llm: Arc<dyn LlmProvider>,
    memory_service: Arc<MemoryService>,
    risk_detector: Arc<dyn RiskDetector>,
    risk_repo: Arc<dyn RiskRepository>,
    event_repo: Arc<dyn AgentEventRepository>,
    conversation_repo: Arc<dyn ConversationRepository>,
    user_profile_repo: Arc<dyn UserProfileRepository>,
    context_builder: Arc<AgentContextBuilder>,
    summary_service: Arc<SummaryService>,
    tools: Vec<Arc<dyn AgentTool>>,
    max_tool_depth: usize,
}

impl AgentRuntime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        llm: Arc<dyn LlmProvider>,
        memory_service: Arc<MemoryService>,
        risk_detector: Arc<dyn RiskDetector>,
        risk_repo: Arc<dyn RiskRepository>,
        event_repo: Arc<dyn AgentEventRepository>,
        conversation_repo: Arc<dyn ConversationRepository>,
        user_profile_repo: Arc<dyn UserProfileRepository>,
        context_builder: Arc<AgentContextBuilder>,
        summary_service: Arc<SummaryService>,
        tools: Vec<Arc<dyn AgentTool>>,
        max_tool_depth: usize,
    ) -> Self {
        Self {
            llm,
            memory_service,
            risk_repo,
            risk_detector,
            event_repo,
            conversation_repo,
            user_profile_repo,
            context_builder,
            summary_service,
            tools,
            max_tool_depth,
        }
    }

    /// Process a single user message and return the agent's response.
    pub async fn respond(
        &self,
        user_id: u64,
        session_id: String,
        conversation_id: Option<u64>,
        user_message: String,
        emotion: Option<String>,
        location: Option<Value>,
        recent_messages: Vec<ChatMessage>,
        session_prompt: Option<String>,
    ) -> Result<AgentResponse, AppError> {
        // ── Step 1: Safety pre-check ──────────────────────────────
        let detection = self.risk_detector.evaluate(&user_message);

        // Log the detection event.
        self.log_event(
            &session_id,
            user_id,
            conversation_id,
            "safety_block",
            serde_json::json!({
                "risk_level": format!("{:?}", detection.risk_level),
                "intent": format!("{:?}", detection.intent),
                "confidence": detection.confidence,
            }),
        )
        .await;

        // ── Step 2: Crisis fast-path ──────────────────────────────
        if detection.risk_level == RiskLevel::Crisis {
            info!(
                user_id,
                ?conversation_id,
                "crisis detected — returning safety response"
            );

            let safety_reply = self.build_crisis_response(&detection);

            // Persist both the user message and the safety reply.
            let persisted = self
                .persist_messages(
                    user_id,
                    conversation_id,
                    &user_message,
                    &safety_reply,
                    &emotion,
                )
                .await?;
            self.spawn_risk_persistence(
                user_id,
                conversation_id,
                Some(persisted.user_message_id),
                &detection,
            );

            return Ok(AgentResponse {
                reply: safety_reply,
                tool_calls: Vec::new(),
                session_closed: false,
                safety_triggered: true,
            });
        }

        if self.is_exit_intent(&user_message) {
            let reply =
                "好的，我们先到这里。需要的时候随时回来，我会继续接住你的话题。".to_string();
            let persisted = self
                .persist_messages(user_id, conversation_id, &user_message, &reply, &emotion)
                .await?;
            self.spawn_risk_persistence(
                user_id,
                conversation_id,
                Some(persisted.user_message_id),
                &detection,
            );

            return Ok(AgentResponse {
                reply,
                tool_calls: Vec::new(),
                session_closed: true,
                safety_triggered: false,
            });
        }

        // ── Step 3: Build agent context ──────────────────────────
        let profile = self
            .user_profile_repo
            .find_by_user_id(user_id)
            .await
            .ok()
            .flatten();

        let recent_messages =
            self.prepare_recent_messages(recent_messages, &user_message, &emotion);

        let llm_tool_defs: Vec<LlmToolDef> = self
            .tools
            .iter()
            .map(|t| LlmToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters(),
            })
            .collect();

        let agent_tool_defs: Vec<crate::domain::agent::ToolDefinition> = self
            .tools
            .iter()
            .map(|t| crate::domain::agent::ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters(),
            })
            .collect();

        let context = self
            .context_builder
            .build(
                session_id.clone(),
                user_id,
                conversation_id,
                recent_messages.clone(),
                profile,
                agent_tool_defs,
            )
            .await;

        // ── Step 4: LLM chat with tools ──────────────────────────
        let system_message =
            self.build_system_message(&context, session_prompt.as_deref(), location.as_ref());

        let mut llm_messages = Vec::with_capacity(recent_messages.len() + 1);
        llm_messages.push(ChatMessage {
            role: "system".into(),
            content: system_message,
            tool_calls: None,
            tool_call_id: None,
        });
        llm_messages.extend(
            recent_messages
                .iter()
                .filter(|m| m.role != "system")
                .cloned(),
        );

        let mut tool_traces: Vec<ToolTrace> = Vec::new();
        let mut messages_with_tool_results = llm_messages.clone();
        let mut depth = 0usize;
        #[allow(unused_assignments)]
        let mut final_content = String::new();
        let end_session = false;

        let have_tools = !self.tools.is_empty();

        loop {
            if depth > self.max_tool_depth {
                final_content = "I've reached the maximum number of tool calls for this turn. Let me summarize what I've found.".to_string();
                break;
            }

            let request = ChatCompletionRequest {
                messages: messages_with_tool_results.clone(),
                temperature: 0.7,
                top_p: 1.0,
                max_tokens: None,
                tools: if have_tools {
                    Some(llm_tool_defs.clone())
                } else {
                    None
                },
            };

            let response = match if have_tools {
                self.llm
                    .chat_with_tools(request.clone(), llm_tool_defs.clone())
                    .await
            } else {
                self.llm.chat(request.clone()).await
            } {
                Ok(r) => r,
                Err(e) => {
                    warn!(error = %e, "LLM chat failed");
                    final_content =
                        "Sorry, I encountered an error processing your request. Please try again."
                            .to_string();
                    break;
                }
            };

            // ── Step 5: Extract and execute tool calls ───────────
            if response.tool_calls.is_empty() {
                final_content = response.content;
                break;
            }

            let mut tool_results: Vec<ChatMessage> = Vec::new();

            for tc in &response.tool_calls {
                let result = self.execute_tool(&context, &tc.name, &tc.arguments).await;

                let result_string = match &result {
                    Ok(text) => text.clone(),
                    Err(e) => format!("Tool error: {e}"),
                };

                tool_traces.push(ToolTrace {
                    tool_name: tc.name.clone(),
                    arguments: tc.arguments.clone(),
                    result: result_string.clone(),
                });

                // Persist tool event on success.
                if result.is_ok() {
                    self.log_event(
                        &session_id,
                        user_id,
                        conversation_id,
                        "tool_call",
                        serde_json::json!({
                            "tool": tc.name,
                            "arguments": tc.arguments,
                            "result": &result_string,
                        }),
                    )
                    .await;
                }

                tool_results.push(ChatMessage {
                    role: "tool".into(),
                    content: result_string,
                    tool_calls: None,
                    tool_call_id: Some(tc.id.clone()),
                });
            }

            // Append the assistant message (with tool_calls metadata) and the
            // tool results to the conversation so the LLM sees them in the
            // next iteration.
            let tool_calls_value: Value =
                serde_json::to_value(&response.tool_calls).unwrap_or_default();
            messages_with_tool_results.push(ChatMessage {
                role: "assistant".into(),
                content: response.content.clone(),
                tool_calls: Some(tool_calls_value),
                tool_call_id: None,
            });
            messages_with_tool_results.extend(tool_results);
            depth += 1;
        }

        // ── Step 6: Final LLM call (if tools were invoked) ─────
        // If we broke out of the loop because of tool-call depth or because
        // the LLM produced text without tool calls, that text is already in
        // `final_content`. If tools *were* invoked (depth > 0) and the last
        // response still asked for tools, we ask the LLM one more time
        // without tools to produce a natural-language summary.
        if depth > 0 && !tool_traces.is_empty() && final_content.is_empty() {
            let final_request = ChatCompletionRequest {
                messages: messages_with_tool_results.clone(),
                temperature: 0.7,
                top_p: 1.0,
                max_tokens: None,
                tools: None,
            };

            match self.llm.chat(final_request).await {
                Ok(r) => final_content = r.content,
                Err(e) => {
                    warn!(error = %e, "final LLM call after tools failed");
                    final_content =
                        "I processed your request but encountered an issue generating the summary."
                            .to_string();
                }
            }
        }

        // ── Step 7: Persist messages ─────────────────────────────
        let persisted = self
            .persist_messages(
                user_id,
                conversation_id,
                &user_message,
                &final_content,
                &emotion,
            )
            .await?;
        self.spawn_risk_persistence(
            user_id,
            conversation_id,
            Some(persisted.user_message_id),
            &detection,
        );

        // ── Step 8: Async memory extraction ──────────────────────
        self.spawn_memory_extraction(
            user_id,
            conversation_id,
            persisted.user_message_id,
            &user_message,
            &final_content,
        );
        self.spawn_summary_refresh(user_id, conversation_id);

        Ok(AgentResponse {
            reply: final_content,
            tool_calls: tool_traces,
            session_closed: end_session,
            safety_triggered: false,
        })
    }

    // ── Private helpers ──────────────────────────────────────────

    fn prepare_recent_messages(
        &self,
        mut messages: Vec<ChatMessage>,
        user_message: &str,
        emotion: &Option<String>,
    ) -> Vec<ChatMessage> {
        let content = match emotion {
            Some(e) if !e.trim().is_empty() => {
                format!("{user_message}\n\n[user emotion: {}]", e.trim())
            }
            _ => user_message.to_string(),
        };

        if let Some(last_user_message) = messages.iter_mut().rev().find(|m| m.role == "user") {
            if last_user_message.content.trim() == user_message.trim() {
                last_user_message.content = content;
                return messages;
            }
        }

        messages.push(ChatMessage {
            role: "user".into(),
            content,
            tool_calls: None,
            tool_call_id: None,
        });
        messages
    }

    fn is_exit_intent(&self, text: &str) -> bool {
        let normalized = text
            .trim()
            .trim_matches(|c: char| c.is_ascii_punctuation() || "。！？、，；：".contains(c))
            .to_lowercase();

        matches!(
            normalized.as_str(),
            "结束对话"
                | "结束会话"
                | "关闭会话"
                | "退出"
                | "退出对话"
                | "不聊了"
                | "先这样"
                | "再见"
                | "拜拜"
                | "bye"
                | "goodbye"
                | "exit"
                | "quit"
                | "end chat"
                | "stop chat"
        )
    }

    /// Build the system message that seeds the LLM with context.
    fn build_system_message(
        &self,
        context: &AgentContext,
        session_prompt: Option<&str>,
        location: Option<&Value>,
    ) -> String {
        let mut parts = Vec::new();

        if let Some(prompt) = session_prompt.filter(|p| !p.trim().is_empty()) {
            parts.push(prompt.to_string());
        } else {
            parts.push(
                "You are a caring, professional mental-health support companion. \
                 Respond with empathy, clarity, and warmth. \
                 You have access to tools that can help you provide better assistance."
                    .to_string(),
            );
        }

        if let Some(location) = location {
            parts.push(format!(
                "\n[User location]\n{location}\nUse this only when local context is relevant."
            ));
        }

        if let Some(ref summary) = context.summary {
            parts.push(format!(
                "\n[Conversation summary]\n{summary}\n\
                 Use this to maintain continuity across turns."
            ));
        }

        if !context.memories.is_empty() {
            let memories_block = context.memories.join("\n- ");
            parts.push(format!(
                "\n[User memories]\n- {memories_block}\n\
                 These are long-term facts / preferences recalled about this user."
            ));
        }

        if !context.rag_chunks.is_empty() {
            let chunks_block = context.rag_chunks.join("\n---\n");
            parts.push(format!(
                "\n[Knowledge base excerpts]\n{chunks_block}\n\
                 Use these to provide accurate, evidence-based information."
            ));
        }

        if let Some(ref profile) = context.user_profile {
            parts.push(format!(
                "\n[User profile]\n{profile}\n\
                 Tailor your responses to the user's interests and preferences."
            ));
        }

        parts.join("\n\n")
    }

    /// Build a crisis / safety response when the risk detector flags Crisis level.
    fn build_crisis_response(&self, detection: &DetectionResult) -> String {
        let evidence = if detection.evidence.is_empty() {
            String::new()
        } else {
            format!(
                "\nSpecific concerns detected: {}",
                detection.evidence.join("; ")
            )
        };

        format!(
            "I'm really concerned about what you've shared. Your well-being is the most important thing right now.\
             {evidence}\n\n\
             Please reach out to a professional who can provide immediate support:\n\
             - National Suicide Prevention Lifeline: 988\n\
             - Crisis Text Line: Text HOME to 741741\n\
             - Or call your local emergency services (911 in the US)\n\n\
             Would you like me to help you find local mental health resources or talk \
             through what you're experiencing?"
        )
    }

    /// Execute a single tool by name, returning its output.
    async fn execute_tool(
        &self,
        context: &AgentContext,
        name: &str,
        args: &Value,
    ) -> Result<String, AppError> {
        for tool in &self.tools {
            if tool.name() == name {
                return tool.execute(context, args.clone()).await;
            }
        }
        Err(AppError::Internal(format!("Unknown tool: {name}")))
    }

    /// Persist the user message and assistant reply to the conversation store.
    async fn persist_messages(
        &self,
        user_id: u64,
        conversation_id: Option<u64>,
        user_message: &str,
        assistant_reply: &str,
        emotion: &Option<String>,
    ) -> Result<PersistedTurn, AppError> {
        let cid = match conversation_id {
            Some(id) => id,
            None => {
                return Err(AppError::Internal(
                    "conversation id is required to persist messages".into(),
                ));
            }
        };

        let user_content = serde_json::json!({
            "text": user_message,
            "emotion": emotion,
        });

        let user_msg = self
            .conversation_repo
            .save_message(NewConversationMessage {
                conversation_id: cid,
                sender_role: "user".into(),
                sender_user_id: Some(user_id),
                message_type: "text".into(),
                content: user_content.to_string(),
                token_count: None,
            })
            .await?;

        let asst_content = serde_json::json!({ "text": assistant_reply });
        self.conversation_repo
            .save_message(NewConversationMessage {
                conversation_id: cid,
                sender_role: "assistant".into(),
                sender_user_id: None,
                message_type: "text".into(),
                content: asst_content.to_string(),
                token_count: None,
            })
            .await?;

        self.conversation_repo.touch_and_incr(cid, 2).await?;

        Ok(PersistedTurn {
            user_message_id: user_msg.id,
        })
    }

    /// Fire-and-forget task: extract memories via MemoryService.
    fn spawn_memory_extraction(
        &self,
        user_id: u64,
        conversation_id: Option<u64>,
        source_message_id: u64,
        user_message: &str,
        assistant_reply: &str,
    ) {
        let memory_service = Arc::clone(&self.memory_service);

        let user_text = user_message.to_string();
        let asst_text = assistant_reply.to_string();

        tokio::spawn(async move {
            // Extract memories via MemoryService (uses MemoryExtractor + vector index)
            let messages = vec![
                crate::domain::llm::ChatMessage {
                    role: "user".into(),
                    content: user_text.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                },
                crate::domain::llm::ChatMessage {
                    role: "assistant".into(),
                    content: asst_text.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                },
            ];

            if let Some(cid) = conversation_id {
                if let Err(e) = memory_service
                    .extract_and_save(user_id, &messages, cid, source_message_id)
                    .await
                {
                    warn!(user_id, conversation_id = cid, error = %e, "memory extraction failed");
                }
            }
        });
    }

    fn spawn_summary_refresh(&self, user_id: u64, conversation_id: Option<u64>) {
        let Some(cid) = conversation_id else {
            return;
        };

        let conversation_repo = Arc::clone(&self.conversation_repo);
        let summary_service = Arc::clone(&self.summary_service);
        let llm = Arc::clone(&self.llm);

        tokio::spawn(async move {
            let messages = match conversation_repo
                .find_messages_by_conversation_id(cid)
                .await
            {
                Ok(messages) => messages,
                Err(e) => {
                    warn!(conversation_id = cid, error = %e, "failed to load messages for summary");
                    return;
                }
            };

            let dialogue: Vec<_> = messages
                .iter()
                .filter(|m| m.sender_role == "user" || m.sender_role == "assistant")
                .collect();

            if dialogue.len() < 8 || dialogue.len() % 6 != 0 {
                return;
            }

            let window: Vec<String> = dialogue
                .iter()
                .rev()
                .take(24)
                .rev()
                .map(|m| format!("{}: {}", m.sender_role, Self::message_text(&m.content)))
                .collect();

            let summary_prompt = format!(
                "Summarize this mental-health support conversation for future continuity. \
                 Keep it concise, factual, and useful. Include user concerns, preferences, \
                 current goals, and any safety-relevant context. Do not invent details.\n\n{}",
                window.join("\n")
            );

            let request = ChatCompletionRequest {
                messages: vec![
                    ChatMessage {
                        role: "system".into(),
                        content: "You write concise rolling conversation summaries.".into(),
                        tool_calls: None,
                        tool_call_id: None,
                    },
                    ChatMessage {
                        role: "user".into(),
                        content: summary_prompt,
                        tool_calls: None,
                        tool_call_id: None,
                    },
                ],
                temperature: 0.2,
                top_p: 1.0,
                max_tokens: Some(512),
                tools: None,
            };

            let summary = match llm.chat(request).await {
                Ok(resp) => resp.content.trim().to_string(),
                Err(e) => {
                    warn!(conversation_id = cid, error = %e, "failed to generate conversation summary");
                    return;
                }
            };

            if summary.is_empty() {
                return;
            }

            let message_start_id = dialogue.first().map(|m| m.id);
            let message_end_id = dialogue.last().map(|m| m.id);
            let token_count =
                Some(summary.split_whitespace().count().min(u32::MAX as usize) as u32);

            if let Err(e) = summary_service
                .save_summary(NewSummary {
                    conversation_id: cid,
                    user_id,
                    summary_type: "rolling".into(),
                    content: summary,
                    message_start_id,
                    message_end_id,
                    token_count,
                })
                .await
            {
                warn!(conversation_id = cid, error = %e, "failed to save conversation summary");
            }
        });
    }

    fn message_text(content: &str) -> String {
        serde_json::from_str::<Value>(content)
            .ok()
            .and_then(|v| {
                v.get("text")
                    .and_then(|text| text.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| content.to_string())
    }

    fn spawn_risk_persistence(
        &self,
        user_id: u64,
        conversation_id: Option<u64>,
        message_id: Option<u64>,
        detection: &DetectionResult,
    ) {
        let risk_repo = Arc::clone(&self.risk_repo);
        let det = detection.clone();

        tokio::spawn(async move {
            let evidence_json =
                serde_json::to_string(&det.evidence).unwrap_or_else(|_| "[]".into());

            if let Err(e) = risk_repo
                .save(NewRiskDetectionResult {
                    user_id,
                    message_id,
                    conversation_id,
                    risk_level: det.risk_level,
                    polarity: det.polarity,
                    intent: det.intent,
                    target: det.target,
                    confidence: det.confidence,
                    evidence: evidence_json,
                    reason: if det.reason.is_empty() {
                        None
                    } else {
                        Some(det.reason)
                    },
                    raw_payload: None,
                    model_name: Some("rule-based".into()),
                    detector_version: Some("1.0".into()),
                })
                .await
            {
                warn!(error = %e, "failed to persist risk detection");
            }
        });
    }

    /// Persist an agent event for observability / auditing.
    async fn log_event(
        &self,
        session_id: &str,
        user_id: u64,
        conversation_id: Option<u64>,
        event_type: &str,
        payload: Value,
    ) {
        let _ = self
            .event_repo
            .log_event(NewAgentEvent {
                user_id,
                conversation_id,
                session_id: Some(session_id.to_string()),
                event_type: event_type.to_string(),
                payload,
            })
            .await;
    }
}
