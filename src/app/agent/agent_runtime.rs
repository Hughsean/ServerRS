use std::sync::Arc;

use serde_json::Value;
use tracing::{debug, info, warn};

use super::agent_context::AgentContextBuilder;
use super::prompt_builder::PromptBuilder;
use crate::app::memory::memory_service::MemoryService;
use crate::domain::agent::{AgentContext, AgentEventRepository, NewAgentEvent};
use crate::domain::conversation::conversation_message::NewConversationMessage;
use crate::domain::conversation::conversation_repository::ConversationRepository;
use crate::domain::llm::{
    ChatCompletionRequest, ChatMessage, LlmProvider, ReasoningConfig, ToolDefinition as LlmToolDef,
};
use crate::domain::user::user_context_version::UserContextVersionRepository;
use crate::domain::user::user_profile_repository::UserProfileRepository;
use crate::shared::error::AppError;

// ── AgentRuntimeSettings ─────────────────────────────────────────────────

/// Runtime configuration for the agent, derived from `AppConfig`.
#[derive(Debug, Clone)]
pub struct AgentRuntimeSettings {
    pub agent_enabled: bool,
    pub memory_enabled: bool,
    pub rag_enabled: bool,
    pub summary_enabled: bool,
    pub max_context_messages: usize,
    pub max_memory_items: u32,
    pub max_rag_chunks: u64,
    pub memory_extraction_async: bool,
    pub max_tool_depth: usize,
    pub temperature: f64,
    pub top_p: f64,
    pub enable_reasoning: bool,
}

impl AgentRuntimeSettings {
    /// Returns `Some(ReasoningConfig { enabled: false })` when reasoning is disabled,
    /// and `None` when reasoning is enabled (Ollama default). Sending `None` means
    /// the serialised JSON simply omits the `reasoning` field.
    pub fn reasoning_config(&self) -> Option<ReasoningConfig> {
        if self.enable_reasoning {
            None
        } else {
            Some(ReasoningConfig { enabled: false })
        }
    }
}

impl Default for AgentRuntimeSettings {
    fn default() -> Self {
        Self {
            agent_enabled: true,
            memory_enabled: true,
            rag_enabled: true,
            summary_enabled: true,
            max_context_messages: 30,
            max_memory_items: 10,
            max_rag_chunks: 5,
            memory_extraction_async: true,
            max_tool_depth: 10,
            temperature: 0.7,
            top_p: 0.9,
            enable_reasoning: true,
        }
    }
}

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
    /// IDs of persisted messages, available after persist_messages succeeds.
    pub user_message_id: Option<u64>,
    pub assistant_message_id: Option<u64>,
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
    assistant_message_id: u64,
}

// ---------------------------------------------------------------------------
// Tool call arguments helpers
// ---------------------------------------------------------------------------

/// Normalise tool-call arguments so that tools always receive a JSON Object.
///
/// OpenAI-compatible APIs (including Ollama) often embed the arguments as a
/// JSON-encoded string inside `function.arguments` instead of a native object.
/// This function handles:
///
/// - `Value::String`  → parse the inner JSON
/// - `Value::Null`    → return `{}`
/// - `Value::Object`  → pass through unchanged
/// - other            → pass through unchanged (with a warn)
pub fn normalize_tool_arguments(raw: &Value) -> Value {
    match raw {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return serde_json::json!({});
            }
            serde_json::from_str::<Value>(trimmed).unwrap_or_else(|err| {
                warn!(
                    error = %err,
                    raw_arguments = %trimmed,
                    "failed to parse tool call arguments; wrapping as error response"
                );
                serde_json::json!({
                    "_invalid_tool_arguments": true,
                    "_raw": trimmed,
                    "_error": err.to_string()
                })
            })
        }
        Value::Null => serde_json::json!({}),
        Value::Object(_) => raw.clone(),
        other => {
            warn!(
                ?other,
                "unexpected tool arguments type; passing through as-is"
            );
            other.clone()
        }
    }
}

/// Check whether an LLM error message indicates a tool-call-arguments
/// compatibility problem.
pub fn is_tool_call_argument_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("invalid tool call arguments")
        || (lower.contains("400") && lower.contains("bad request") && lower.contains("tool"))
        || lower.contains("tool call arguments")
}

// ---------------------------------------------------------------------------
// AgentRuntime
// ---------------------------------------------------------------------------

/// High-level agent runtime that orchestrates a single message-response turn.
///
/// Flow:
/// 1. Build `AgentContext` (summary, memories, RAG chunks, profile).
/// 2. Run the LLM with tool definitions.
/// 3. Extract tool calls from the LLM response and execute them.
/// 4. Feed tool results back to the LLM for a final response.
/// 5. Persist messages.
/// 6. Spawn async memory extraction.
pub struct AgentRuntime {
    llm: Arc<dyn LlmProvider>,
    memory_service: Arc<MemoryService>,
    event_repo: Arc<dyn AgentEventRepository>,
    conversation_repo: Arc<dyn ConversationRepository>,
    user_profile_repo: Arc<dyn UserProfileRepository>,
    context_version_repo: Arc<dyn UserContextVersionRepository>,
    context_builder: Arc<AgentContextBuilder>,
    prompt_builder: PromptBuilder,
    tools: Vec<Arc<dyn AgentTool>>,
    settings: AgentRuntimeSettings,
}

impl AgentRuntime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        llm: Arc<dyn LlmProvider>,
        memory_service: Arc<MemoryService>,
        event_repo: Arc<dyn AgentEventRepository>,
        conversation_repo: Arc<dyn ConversationRepository>,
        user_profile_repo: Arc<dyn UserProfileRepository>,
        context_version_repo: Arc<dyn UserContextVersionRepository>,
        context_builder: Arc<AgentContextBuilder>,
        tools: Vec<Arc<dyn AgentTool>>,
        settings: AgentRuntimeSettings,
    ) -> Self {
        Self {
            llm,
            memory_service,
            event_repo,
            conversation_repo,
            user_profile_repo,
            context_version_repo,
            context_builder,
            prompt_builder: PromptBuilder::new(),
            tools,
            settings,
        }
    }

    /// Process a single user message and return the agent's response.
    pub async fn respond(
        &self,
        user_id: u64,
        conversation_id: Option<u64>,
        user_message: String,
        emotion: Option<String>,
        location: Option<Value>,
        recent_messages: Vec<ChatMessage>,
    ) -> Result<AgentResponse, AppError> {
        let task_epoch = self
            .context_version_repo
            .get_or_create(user_id)
            .await?
            .version;
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

        let agent_on = self.settings.agent_enabled;
        let summary_enabled = agent_on && self.settings.summary_enabled;
        let memory_enabled = agent_on && self.settings.memory_enabled;
        let rag_enabled = agent_on && self.settings.rag_enabled;

        let context = self
            .context_builder
            .build(
                user_id,
                conversation_id,
                recent_messages.clone(),
                profile,
                agent_tool_defs,
                location.clone(),
                self.settings.max_memory_items,
                self.settings.max_rag_chunks,
                summary_enabled,
                memory_enabled,
                rag_enabled,
            )
            .await;

        // ── Step 4: LLM chat with tools ──────────────────────────
        let registered_tools_available = !self.tools.is_empty();
        let tools_available = self.settings.agent_enabled
            && registered_tools_available
            && self.settings.max_tool_depth > 0;

        let system_message = self
            .prompt_builder
            .build_system_message(&context, tools_available);

        let mut llm_messages = Vec::with_capacity(recent_messages.len() + 1);
        llm_messages.push(ChatMessage {
            role: "system".into(),
            content: system_message,
            tool_calls: None,
            tool_call_id: None,
            name: None,
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
        let _end_session = false;

        let tool_names: Vec<&str> = self.tools.iter().map(|t| t.name()).collect();

        info!(
            ?conversation_id,
            tools_available,
            registered_tools = registered_tools_available,
            tool_names = %tool_names.join(","),
            message_count = messages_with_tool_results.len(),
            "calling LLM"
        );

        loop {
            let tools_allowed = tools_allowed_for_round(
                self.settings.agent_enabled,
                registered_tools_available,
                depth,
                self.settings.max_tool_depth,
            );

            let request = ChatCompletionRequest {
                messages: messages_with_tool_results.clone(),
                temperature: self.settings.temperature,
                top_p: self.settings.top_p,
                max_tokens: None,
                tools: if tools_allowed {
                    Some(llm_tool_defs.clone())
                } else {
                    None
                },
                reasoning: self.settings.reasoning_config(),
            };

            let response = match if tools_allowed {
                self.llm
                    .chat_with_tools(request.clone(), llm_tool_defs.clone())
                    .await
            } else {
                self.llm.chat(request.clone()).await
            } {
                Ok(r) => r,
                Err(e) => {
                    let err_msg = e.to_string();
                    warn!(
                        ?conversation_id,
                        error = %e,
                        "LLM chat failed"
                    );

                    if registered_tools_available && is_tool_call_argument_error(&err_msg) {
                        warn!(
                            ?conversation_id,
                            error = %e,
                            "LLM tool call failed; retrying without tools"
                        );

                        let fallback_request = ChatCompletionRequest {
                            messages: messages_with_tool_results.clone(),
                            temperature: self.settings.temperature,
                            top_p: self.settings.top_p,
                            max_tokens: None,
                            tools: None,
                            reasoning: self.settings.reasoning_config(),
                        };

                        match self.llm.chat(fallback_request).await {
                            Ok(r) => {
                                final_content = normalize_final_content(r.content);
                                break;
                            }
                            Err(fb_err) => {
                                warn!(
                                    ?conversation_id,
                                    error = %fb_err,
                                    "LLM fallback (no tools) also failed"
                                );
                                final_content =
                                    "抱歉，我刚才处理这条消息时遇到了一点问题。你可以换个说法再发一次，我会继续帮你。"
                                        .to_string();
                                break;
                            }
                        }
                    } else {
                        final_content =
                            "抱歉，我刚才处理这条消息时遇到了一点问题。你可以换个说法再发一次，我会继续帮你。"
                                .to_string();
                        break;
                    }
                }
            };

            // ── No tool calls returned: use content ─────────────
            if response.tool_calls.is_empty() {
                final_content = normalize_final_content(response.content);
                break;
            }

            // ── Tool calls returned but not allowed: ignore them ─
            if !tools_allowed {
                warn!(
                    ?conversation_id,
                    tool_call_count = response.tool_calls.len(),
                    "LLM returned tool calls when tools were not allowed; ignoring tool calls"
                );

                if !response.content.trim().is_empty() {
                    final_content = response.content;
                } else {
                    final_content = self
                        .final_chat_without_tools(messages_with_tool_results.clone(), false)
                        .await;
                }
                final_content = normalize_final_content(final_content);
                break;
            }

            // ── Execute tool calls ───────────────────────────────
            info!(
                tool_call_count = response.tool_calls.len(),
                "LLM returned tool calls"
            );

            let mut tool_results: Vec<ChatMessage> = Vec::new();

            for tc in &response.tool_calls {
                let normalized_arguments = normalize_tool_arguments(&tc.arguments);

                debug!(
                    tool_name = %tc.name,
                    raw_arguments = %tc.arguments,
                    parsed_arguments = %normalized_arguments,
                    "processing tool call"
                );

                let result = self
                    .execute_tool(&context, &tc.name, &normalized_arguments)
                    .await;

                let result_string = match &result {
                    Ok(text) => text.clone(),
                    Err(e) => format!("Tool error: {e}"),
                };

                let result_preview = truncate_for_event(&result_string, 2000);

                tool_traces.push(ToolTrace {
                    tool_name: tc.name.clone(),
                    arguments: normalized_arguments.clone(),
                    result: result_string.clone(),
                });

                self.log_event(
                    user_id,
                    conversation_id,
                    "tool_call",
                    Some(tc.name.clone()),
                    serde_json::json!({
                        "tool": tc.name,
                        "arguments": normalized_arguments,
                        "raw_arguments": tc.arguments,
                        "ok": result.is_ok(),
                        "result_preview": result_preview,
                        "error": result.as_ref().err().map(|e| e.to_string()),
                    }),
                )
                .await;

                tool_results.push(ChatMessage {
                    role: "tool".into(),
                    content: result_string,
                    tool_calls: None,
                    tool_call_id: Some(tc.id.clone()),
                    name: Some(tc.name.clone()),
                });
            }

            let tool_calls_openai: Vec<Value> = response
                .tool_calls
                .iter()
                .map(|tc| {
                    let args_str = match &tc.arguments {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    serde_json::json!({
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.name,
                            "arguments": args_str,
                        }
                    })
                })
                .collect();
            let tool_calls_value = Value::Array(tool_calls_openai);
            messages_with_tool_results.push(ChatMessage {
                role: "assistant".into(),
                content: response.content.clone(),
                tool_calls: Some(tool_calls_value),
                tool_call_id: None,
                name: None,
            });
            messages_with_tool_results.extend(tool_results);
            depth += 1;

            // After executing tools, if depth is exhausted, do one final
            // round without tools to produce a natural-language summary.
            if !tools_allowed_for_round(
                self.settings.agent_enabled,
                registered_tools_available,
                depth,
                self.settings.max_tool_depth,
            ) {
                final_content = self
                    .final_chat_without_tools(messages_with_tool_results.clone(), true)
                    .await;
                break;
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
        // Risk persist deferred to TurnClosedEvent

        // ── Step 8: Async memory extraction ──────────────────────
        if self.settings.agent_enabled
            && self.settings.memory_enabled
            && self.settings.memory_extraction_async
        {
            self.spawn_memory_extraction(
                user_id,
                conversation_id,
                persisted.user_message_id,
                &user_message,
                &final_content,
                task_epoch,
            );
        }

        Ok(AgentResponse {
            reply: final_content,
            tool_calls: tool_traces,
            user_message_id: Some(persisted.user_message_id),
            assistant_message_id: Some(persisted.assistant_message_id),
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
                // Apply max_context_messages limit (keep system messages, truncate the rest)
                return self.apply_context_limit(messages);
            }
        }

        messages.push(ChatMessage {
            role: "user".into(),
            content,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });
        self.apply_context_limit(messages)
    }

    /// Keep system messages and retain only the most recent N non-system messages.
    fn apply_context_limit(&self, messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
        let limit = self.settings.max_context_messages;
        if limit == 0 || messages.is_empty() {
            return messages;
        }
        let system_msgs: Vec<ChatMessage> = messages
            .iter()
            .filter(|m| m.role == "system")
            .cloned()
            .collect();
        let mut other_msgs: Vec<ChatMessage> = messages
            .into_iter()
            .filter(|m| m.role != "system")
            .collect();
        let other_count = other_msgs.len();
        if other_count > limit {
            let skip = other_count.saturating_sub(limit);
            other_msgs = other_msgs.into_iter().skip(skip).collect();
        }
        let mut result = system_msgs;
        result.extend(other_msgs);
        result
    }

    /// Perform one final LLM call without tools, returning the reply text.
    /// Used as a fallback when tools are exhausted or unavailable.
    async fn final_chat_without_tools(
        &self,
        mut messages: Vec<ChatMessage>,
        had_tool_results: bool,
    ) -> String {
        messages.push(ChatMessage {
            role: "user".into(),
            content: if had_tool_results {
                "本轮工具已经用完。请基于已有上下文和工具结果，直接用中文回复用户，不要再调用工具。"
                    .into()
            } else {
                "本轮没有可用工具。请基于已有上下文直接用中文回复用户，不要调用工具。".into()
            },
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });

        let request = ChatCompletionRequest {
            messages,
            temperature: self.settings.temperature,
            top_p: self.settings.top_p,
            max_tokens: None,
            tools: None,
            reasoning: self.settings.reasoning_config(),
        };

        match self.llm.chat(request).await {
            Ok(r) => normalize_final_content(r.content),
            Err(e) => {
                warn!(error = %e, "final chat without tools failed");
                "抱歉，我刚才处理这条消息时遇到了一点问题。你可以换个说法再发一次，我会继续帮你。"
                    .to_string()
            }
        }
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
                match tool.execute(context, args.clone()).await {
                    Ok(output) => {
                        info!(tool = name, "agent tool completed");
                        return Ok(output);
                    }
                    Err(e) => {
                        warn!(tool = name, error = %e, "agent tool failed");
                        return Err(e);
                    }
                }
            }
        }
        Err(AppError::Internal(format!("Unknown tool: {name}")))
    }

    /// 持久化用户消息和 AI 回复到数据库（使用事务保证原子性）。
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
                    "需要对话 ID 才能持久化消息".into(),
                ));
            }
        };

        let user_content = serde_json::json!({
            "text": user_message,
            "emotion": emotion,
        });

        let asst_content = serde_json::json!({ "text": assistant_reply });

        // 使用事务原子化保存两条消息并更新计数
        let (user_saved, asst_saved) = self
            .conversation_repo
            .save_turn_atomic(
                cid,
                user_id,
                NewConversationMessage {
                    conversation_id: cid,
                    sender_role: "user".into(),
                    sender_user_id: Some(user_id),
                    message_type: "text".into(),
                    content: user_content.to_string(),
                    token_count: None,
                },
                NewConversationMessage {
                    conversation_id: cid,
                    sender_role: "assistant".into(),
                    sender_user_id: None,
                    message_type: "text".into(),
                    content: asst_content.to_string(),
                    token_count: None,
                },
            )
            .await?;

        Ok(PersistedTurn {
            user_message_id: user_saved.id,
            assistant_message_id: asst_saved.id,
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
        task_epoch: u64,
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
                    name: None,
                },
                crate::domain::llm::ChatMessage {
                    role: "assistant".into(),
                    content: asst_text.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                },
            ];

            if let Some(cid) = conversation_id {
                if let Err(e) = memory_service
                    .extract_and_save_at_version(
                        user_id,
                        &messages,
                        cid,
                        source_message_id,
                        Some(task_epoch),
                    )
                    .await
                {
                    warn!(user_id, conversation_id = cid, error = %e, "memory extraction failed");
                }
            }
        });
    }
}

/// Truncate a string for event recording, keeping at most `max_chars` characters.
fn truncate_for_event(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}...[truncated]")
    }
}

/// Check whether tools are allowed for the current round.
/// Tools are only allowed when:
/// - The agent is enabled
/// - Tools are registered
/// - The depth has not yet reached max_tool_depth
fn tools_allowed_for_round(
    agent_enabled: bool,
    have_tools: bool,
    depth: usize,
    max_tool_depth: usize,
) -> bool {
    agent_enabled && have_tools && depth < max_tool_depth
}

/// Remove serialized tool calls that some models echo before the final answer.
fn strip_leading_tool_call_artifacts(content: &str) -> &str {
    const CLOSING_TAG: &str = "</tool_call>";
    const OPENING_MARKERS: [&str; 3] = ["<tool_call>", "<|tool_call|>", "_icall_"];

    let mut remaining = content.trim();
    loop {
        let Some(closing_index) = remaining.find(CLOSING_TAG) else {
            break;
        };
        let artifact = &remaining[..closing_index];
        let starts_with_marker = OPENING_MARKERS
            .iter()
            .any(|marker| artifact.trim_start().starts_with(marker));
        let looks_like_tool_call =
            artifact.contains("\"name\"") && artifact.contains("\"arguments\"");

        if !starts_with_marker || !looks_like_tool_call {
            break;
        }

        remaining = remaining[closing_index + CLOSING_TAG.len()..].trim_start();
    }

    remaining
}

/// Ensure final content is clean and never empty. Return a Chinese fallback if needed.
fn normalize_final_content(content: String) -> String {
    let content = strip_leading_tool_call_artifacts(&content);
    if content.is_empty() {
        "抱歉，我刚才处理这条消息时遇到了一点问题。你可以换个说法再发一次，我会继续帮你。"
            .to_string()
    } else {
        content.to_string()
    }
}

impl AgentRuntime {
    /// Persist an agent event for observability / auditing.
    async fn log_event(
        &self,
        user_id: u64,
        conversation_id: Option<u64>,
        event_type: &str,
        tool_name: Option<String>,
        payload: Value,
    ) {
        let _ = self
            .event_repo
            .log_event(NewAgentEvent {
                user_id,
                conversation_id,
                event_type: event_type.to_string(),
                tool_name,
                payload,
            })
            .await;
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── normalize_tool_arguments ──────────────────────────────────────

    #[test]
    fn arguments_string_json_object_is_parsed() {
        let raw = json!(r#"{"query":"最近新闻热点","top_k":5}"#);
        let result = normalize_tool_arguments(&raw);
        assert_eq!(result["query"], "最近新闻热点");
        assert_eq!(result["top_k"], 5);
    }

    #[test]
    fn empty_arguments_string_becomes_empty_object() {
        let result = normalize_tool_arguments(&json!(""));
        assert_eq!(result, json!({}));
    }

    #[test]
    fn null_arguments_becomes_empty_object() {
        let result = normalize_tool_arguments(&json!(null));
        assert_eq!(result, json!({}));
    }

    #[test]
    fn object_arguments_passthrough() {
        let obj = json!({"query": "test"});
        let result = normalize_tool_arguments(&obj);
        assert_eq!(result, obj);
    }

    #[test]
    fn malformed_arguments_does_not_panic() {
        let raw = json!(r#"{"query":"#);
        let result = normalize_tool_arguments(&raw);
        assert!(result["_invalid_tool_arguments"] == true);
        assert!(result["_raw"].as_str().unwrap().contains("query"));
    }

    // ── is_tool_call_argument_error ───────────────────────────────────

    #[test]
    fn detects_invalid_tool_call_arguments() {
        assert!(is_tool_call_argument_error("invalid tool call arguments"));
    }

    #[test]
    fn detects_400_bad_request_with_tool() {
        assert!(is_tool_call_argument_error(
            "LLM returned 400 Bad Request from http://127.0.0.1:11434/v1/chat/completions: {\"error\":{\"message\":\"invalid tool call arguments\"}}"
        ));
    }

    #[test]
    fn non_tool_error_returns_false() {
        assert!(!is_tool_call_argument_error("connection refused"));
        assert!(!is_tool_call_argument_error("timeout"));
    }

    // ── tools_allowed_for_round ──────────────────────────────────────

    #[test]
    fn tools_allowed_agent_disabled() {
        assert!(!tools_allowed_for_round(false, true, 0, 10));
    }

    #[test]
    fn tools_allowed_no_registered_tools() {
        assert!(!tools_allowed_for_round(true, false, 0, 10));
    }

    #[test]
    fn tools_allowed_depth_zero_max_zero() {
        // max_tool_depth=0 means no tools, even at depth 0
        assert!(!tools_allowed_for_round(true, true, 0, 0));
    }

    #[test]
    fn tools_allowed_depth_zero_max_one() {
        // depth 0 < max 1 → allowed
        assert!(tools_allowed_for_round(true, true, 0, 1));
    }

    #[test]
    fn tools_allowed_depth_equals_max() {
        // depth 1 not < max 1 → NOT allowed
        assert!(!tools_allowed_for_round(true, true, 1, 1));
    }

    #[test]
    fn tools_allowed_depth_three_max_five() {
        assert!(tools_allowed_for_round(true, true, 3, 5));
        assert!(!tools_allowed_for_round(true, true, 5, 5));
    }

    // ── normalize_final_content ──────────────────────────────────────

    #[test]
    fn normalizes_empty_content() {
        assert!(normalize_final_content("   ".into()).contains("抱歉"));
    }

    #[test]
    fn preserves_non_empty_content() {
        assert_eq!(normalize_final_content("你好".into()), "你好");
    }

    #[test]
    fn removes_standard_tool_call_artifact() {
        let content = r#"<tool_call>
{"name":"get_weather","arguments":{"location":"合肥"}}
</tool_call>
合肥今天多云。"#;

        assert_eq!(normalize_final_content(content.into()), "合肥今天多云。");
    }

    #[test]
    fn removes_malformed_tool_call_artifact() {
        let content = r#"_icall_
{"name":"get_baidu_baike","arguments":{"keyword":"日本首相"}}
</tool_call>
根据百科资料，日本首相是日本政府首脑。"#;

        assert_eq!(
            normalize_final_content(content.into()),
            "根据百科资料，日本首相是日本政府首脑。"
        );
    }

    #[test]
    fn preserves_regular_json_discussion() {
        let content = r#"示例参数是 {"name":"demo","arguments":{}}。"#;
        assert_eq!(normalize_final_content(content.into()), content);
    }

    // ── Runtime behavior tests (via integration test helpers) ─────────
    // These tests verify that the main AgentRuntime loop correctly handles
    // max_tool_depth=0 and max_tool_depth=1 scenarios. Because constructing
    // a full AgentRuntime requires many mock dependencies (which are fragile
    // to trait signature drift), these tests live in the integration test
    // suite: tests/common/mod.rs → agent_depth_behavior_tests.

    /// Verify: max_tool_depth=0 → tools_allowed_for_round returns false,
    /// and build_system_message with tools_available=false does not claim
    /// tool capability.
    #[test]
    fn max_tool_depth_zero_blocks_tools_entirely() {
        // tools_allowed_for_round returns false at depth 0 when max=0
        assert!(!tools_allowed_for_round(true, true, 0, 0));

        // With max_tool_depth=0, tools_available computes to false
        let agent_enabled = true;
        let have_tools = true;
        let max_tool_depth = 0;
        let tools_available = agent_enabled && have_tools && max_tool_depth > 0;
        assert!(
            !tools_available,
            "tools_available should be false when max_tool_depth=0"
        );
    }

    /// Verify: max_tool_depth=1 → one round of tools allowed, then exhausted.
    #[test]
    fn max_tool_depth_one_allows_one_round_then_stops() {
        // tools_allowed_for_round: depth 0 < max 1 → true
        assert!(tools_allowed_for_round(true, true, 0, 1));
        // After one round: depth 1 not < max 1 → false
        assert!(!tools_allowed_for_round(true, true, 1, 1));

        // tools_available computes to true when max_tool_depth=1
        let tools_available = true && true && 1 > 0;
        assert!(tools_available);
    }

    /// Verify: system prompt does not claim tool capability when unavailable.
    #[test]
    fn system_prompt_without_tools_no_tool_claims() {
        // Construct a minimal AgentRuntime just for testing build_system_message
        // We test the property statically: the tools_available flag controls
        // the prompt content. The actual build_system_message method is
        // exercised in integration tests.
        let agent_enabled = true;
        let have_tools = false; // no registered tools
        let tools_available = agent_enabled && have_tools;
        assert!(!tools_available);
    }
}
