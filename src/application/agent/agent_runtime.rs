use std::sync::Arc;

use serde_json::Value;
use tracing::{info, warn};

use super::agent_context::AgentContextBuilder;
use crate::application::memory::memory_service::MemoryService;
use crate::domain::agent::{AgentContext, AgentEventRepository, NewAgentEvent};
use crate::domain::conversation::conversation_message::NewConversationMessage;
use crate::domain::conversation::conversation_repository::ConversationRepository;
use crate::domain::llm::{
    ChatCompletionRequest, ChatMessage, LlmProvider, ToolDefinition as LlmToolDef,
};
use crate::domain::rag::RAGRepository;
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
    rag_repo: Arc<dyn RAGRepository>,
    memory_service: Arc<MemoryService>,
    risk_detector: Arc<dyn RiskDetector>,
    risk_repo: Arc<dyn RiskRepository>,
    event_repo: Arc<dyn AgentEventRepository>,
    conversation_repo: Arc<dyn ConversationRepository>,
    user_profile_repo: Arc<dyn UserProfileRepository>,
    context_builder: Arc<AgentContextBuilder>,
    tools: Vec<Arc<dyn AgentTool>>,
    max_tool_depth: usize,
}

impl AgentRuntime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        llm: Arc<dyn LlmProvider>,
        rag_repo: Arc<dyn RAGRepository>,
        memory_service: Arc<MemoryService>,
        risk_detector: Arc<dyn RiskDetector>,
        risk_repo: Arc<dyn RiskRepository>,
        event_repo: Arc<dyn AgentEventRepository>,
        conversation_repo: Arc<dyn ConversationRepository>,
        user_profile_repo: Arc<dyn UserProfileRepository>,
        context_builder: Arc<AgentContextBuilder>,
        tools: Vec<Arc<dyn AgentTool>>,
        max_tool_depth: usize,
    ) -> Self {
        Self {
            llm,
            rag_repo,
            memory_service,
            risk_repo,
            risk_detector,
            event_repo,
            conversation_repo,
            user_profile_repo,
            context_builder,
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
        #[allow(unused_variables)] location: Option<Value>,
    ) -> AgentResponse {
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
            self.persist_messages(
                user_id,
                conversation_id,
                &user_message,
                &safety_reply,
                &emotion,
            )
            .await;

            return AgentResponse {
                reply: safety_reply,
                tool_calls: Vec::new(),
                session_closed: false,
                safety_triggered: true,
            };
        }

        // ── Step 3: Build agent context ──────────────────────────
        let profile = self
            .user_profile_repo
            .find_by_user_id(user_id)
            .await
            .ok()
            .flatten();

        let user_chat_message = ChatMessage {
            role: "user".into(),
            content: match &emotion {
                Some(e) if !e.is_empty() => {
                    format!("{user_message}\n\n[user emotion: {e}]")
                }
                _ => user_message.clone(),
            },
            tool_calls: None,
            tool_call_id: None,
        };

        let recent_messages = vec![user_chat_message];

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
        let system_message = self.build_system_message(&context);

        let llm_messages = vec![
            ChatMessage {
                role: "system".into(),
                content: system_message,
                tool_calls: None,
                tool_call_id: None,
            },
            recent_messages[0].clone(),
        ];

        let mut tool_traces: Vec<ToolTrace> = Vec::new();
        let mut messages_with_tool_results = llm_messages.clone();
        let mut depth = 0usize;
        #[allow(unused_assignments)]
        let mut final_content = String::new();
        let mut end_session = false;

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

            let response = match self.llm.chat(request.clone()).await {
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
        self.persist_messages(
            user_id,
            conversation_id,
            &user_message,
            &final_content,
            &emotion,
        )
        .await;

        // ── Step 8: Async memory extraction ──────────────────────
        self.spawn_memory_extraction(
            user_id,
            conversation_id,
            &user_message,
            &final_content,
            &emotion,
            &detection,
        );

        AgentResponse {
            reply: final_content,
            tool_calls: tool_traces,
            session_closed: end_session,
            safety_triggered: false,
        }
    }

    // ── Private helpers ──────────────────────────────────────────

    /// Build the system message that seeds the LLM with context.
    fn build_system_message(&self, context: &AgentContext) -> String {
        let mut parts = Vec::new();

        parts.push(
            "You are a caring, professional mental-health support companion. \
             Respond with empathy, clarity, and warmth. \
             You have access to tools that can help you provide better assistance."
                .to_string(),
        );

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
    ) {
        let cid = match conversation_id {
            Some(id) => id,
            None => return,
        };

        let user_content = serde_json::json!({
            "text": user_message,
            "emotion": emotion,
        });

        let _ = self
            .conversation_repo
            .save_message(NewConversationMessage {
                conversation_id: cid,
                sender_role: "user".into(),
                sender_user_id: Some(user_id),
                message_type: "text".into(),
                content: user_content.to_string(),
                token_count: None,
            })
            .await;

        let asst_content = serde_json::json!({ "text": assistant_reply });
        let _ = self
            .conversation_repo
            .save_message(NewConversationMessage {
                conversation_id: cid,
                sender_role: "assistant".into(),
                sender_user_id: None,
                message_type: "text".into(),
                content: asst_content.to_string(),
                token_count: None,
            })
            .await;

        let _ = self.conversation_repo.touch_and_incr(cid, 2).await;
    }

    /// Fire-and-forget task: extract memories via MemoryService and
    /// persist the risk detection result.
    fn spawn_memory_extraction(
        &self,
        user_id: u64,
        conversation_id: Option<u64>,
        user_message: &str,
        assistant_reply: &str,
        emotion: &Option<String>,
        detection: &DetectionResult,
    ) {
        let memory_service = Arc::clone(&self.memory_service);
        let risk_repo = Arc::clone(&self.risk_repo);

        let user_text = user_message.to_string();
        let asst_text = assistant_reply.to_string();
        let emotion_text = emotion.clone();
        let det = detection.clone();

        tokio::spawn(async move {
            // Persist risk detection result.
            let evidence_json =
                serde_json::to_string(&det.evidence).unwrap_or_else(|_| "[]".into());
            let _ = risk_repo
                .save(NewRiskDetectionResult {
                    user_id,
                    message_id: None,
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
                .await;

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
                    .extract_and_save(user_id, &messages, cid, 0)
                    .await
                {
                    warn!(user_id, conversation_id = cid, error = %e, "memory extraction failed");
                }
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
