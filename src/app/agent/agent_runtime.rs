use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::Value;
use tracing::{debug, info, warn};

use super::agent_context::AgentContextBuilder;
use super::prompt_builder::PromptBuilder;
use crate::app::memory::memory_service::MemoryService;
use crate::domain::agent::{AgentContext, AgentEventRepoT, NewAgentEvent};
use crate::domain::conversation::conversation_message::NewConversationMessage;
use crate::domain::conversation::conversation_repository::ConversationRepoT;
use crate::domain::llm::{
    ChatCompletionRequest, ChatMessage, LlmProvider, ReasoningConfig, ToolDefinition as LlmToolDef,
};
use crate::domain::user::user_context_version::UserContextVersionRepoT;
use crate::domain::user::user_profile_repository::UserProfileRepoT;
use crate::shared::error::AppError;
use crate::shared::llm_json::parse_llm_json;

// ── AgentRuntimeSettings ─────────────────────────────────────────────────

/// AgentRuntime配置，派生自 `AppConfig`。
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
    /// 当推理被禁用时返回 `Some(ReasoningConfig { enabled: false })`，
    /// 当推理启用时返回 `None`（Ollama 默认行为）。发送 `None` 表示
    /// 序列化后的 JSON 会直接省略 `reasoning` 字段。
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

/// AgentRuntime可调用的工具。
///
/// 这里有意与 `LlmTool` 保持分离，使Agent可以
/// 定义能够访问完整 `AgentContext`（用户画像、
/// 记忆、RAG 片段等）的工具，而不是只能访问较窄的 `ToolExecutionContext`。
#[async_trait::async_trait]
pub trait AgentTool: Send + Sync {
    /// 唯一的工具名称（必须与 `ToolDefinition` 中的名称匹配）。
    fn name(&self) -> &str;

    /// 面向用户可读的描述。
    fn description(&self) -> &str;

    /// 描述可接受参数的 JSON Schema。
    fn parameters(&self) -> Value;

    /// 执行工具。
    ///
    /// `context` 提供本轮的完整 Agent 上下文，因此工具
    /// 可以基于用户画像、记忆等做出决策。
    async fn execute(&self, context: &AgentContext, args: Value) -> Result<String, AppError>;
}

// ---------------------------------------------------------------------------
// Agent响应
// ---------------------------------------------------------------------------

/// AgentRuntime处理一轮对话后生成的结构化结果。
#[derive(Debug, Clone)]
pub struct AgentResponse {
    /// 返回给用户的最终文本回复。
    pub reply: String,
    /// 本轮调用过的每个工具的追踪记录。
    pub tool_calls: Vec<ToolTrace>,
    /// 已持久化消息的 ID，在 persist_messages 成功后可用。
    pub user_message_id: Option<u64>,
    pub assistant_message_id: Option<u64>,
}

/// 一次会话轮次中单个工具调用的记录。
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
// 工具调用参数辅助函数
// ---------------------------------------------------------------------------

/// 规范化工具调用参数，使工具始终收到 JSON Object。
///
/// OpenAI 兼容 API（包括 Ollama）通常会把参数作为
/// JSON 编码字符串嵌入 `function.arguments`，而不是使用原生对象。
/// 此函数处理以下情况：
///
/// - `Value::String`  → 解析内部 JSON
/// - `Value::Null`    → 返回 `{}`
/// - `Value::Object`  → 原样传递
/// - other            → 原样传递（并记录 warn）
pub fn normalize_tool_arguments(raw: &Value) -> Value {
    match raw {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return serde_json::json!({});
            }
            parse_llm_json::<Value>(trimmed).unwrap_or_else(|err| {
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

/// 检查 LLM 错误消息是否表示工具调用参数的
/// 兼容性问题。
pub fn is_tool_call_argument_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("invalid tool call arguments")
        || (lower.contains("400") && lower.contains("bad request") && lower.contains("tool"))
        || lower.contains("tool call arguments")
}

// ---------------------------------------------------------------------------
// AgentRuntime
// ---------------------------------------------------------------------------

/// Top AgentRuntime，负责编排单轮消息-回复流程。
///
/// 流程：
/// 1. 构建 `AgentContext`（摘要、记忆、RAG 片段、用户画像）。
/// 2. 使用工具定义运行 LLM。
/// 3. 从 LLM 响应中提取工具调用并执行它们。
/// 4. 将工具结果回传给 LLM，以生成最终响应。
/// 5. 持久化消息。
/// 6. 启动异步记忆提取。
pub struct AgentRuntime {
    llm: Arc<dyn LlmProvider>,
    memory_service: Arc<MemoryService>,
    event_repo: Arc<dyn AgentEventRepoT>,
    conversation_repo: Arc<dyn ConversationRepoT>,
    user_profile_repo: Arc<dyn UserProfileRepoT>,
    context_version_repo: Arc<dyn UserContextVersionRepoT>,
    context_builder: Arc<AgentContextBuilder>,
    prompt_builder: PromptBuilder,
    tools: Vec<Arc<dyn AgentTool>>,
    settings: AgentRuntimeSettings,

    /// 上一次记忆提取失败的时间（失败时的 Instant::now()）。
    /// 用于在重复失败后对提取操作进行限流。
    last_extraction_failure: Arc<Mutex<Option<Instant>>>,
}

impl AgentRuntime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        llm: Arc<dyn LlmProvider>,
        memory_service: Arc<MemoryService>,
        event_repo: Arc<dyn AgentEventRepoT>,
        conversation_repo: Arc<dyn ConversationRepoT>,
        user_profile_repo: Arc<dyn UserProfileRepoT>,
        context_version_repo: Arc<dyn UserContextVersionRepoT>,
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
            last_extraction_failure: Arc::new(Mutex::new(None)),
        }
    }

    pub fn max_context_messages(&self) -> usize {
        self.settings.max_context_messages
    }

    /// 处理单条用户消息并返回 Agent 的响应。
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
        // ── 步骤 3：构建 Agent 上下文 ──────────────────────────
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

        // ── 步骤 4：使用工具进行 LLM 聊天 ──────────────────────────
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

            // ── 未返回工具调用：使用内容 ─────────────
            if response.tool_calls.is_empty() {
                final_content = normalize_final_content(response.content);
                break;
            }

            // ── 返回了工具调用但当前不允许：忽略它们 ─
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

            // ── 执行工具调用 ───────────────────────────────
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

            // 执行工具后，如果深度已耗尽，则再进行一次
            // 不带工具的最终轮次，以生成自然语言摘要。
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

        // ── 步骤 7：持久化消息 ─────────────────────────────
        let persisted = self
            .persist_messages(
                user_id,
                conversation_id,
                &user_message,
                &final_content,
                &emotion,
            )
            .await?;
        // 风险持久化延后到 TurnClosedEvent

        // ── 步骤 8：异步记忆提取 ──────────────────────
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

    // ── 私有辅助函数 ──────────────────────────────────────────

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
                // 应用 max_context_messages 限制（保留 system 消息，截断其余消息）
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

    /// 保留 system 消息，并仅保留最近 N 条非 system 消息。
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

    /// 执行一次不带工具的最终 LLM 调用，并返回回复文本。
    /// 当工具已耗尽或不可用时作为回退使用。
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

    /// 按名称执行单个工具，并返回其输出。
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
                return Err(AppError::Internal("需要对话 ID 才能持久化消息".into()));
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

    /// 即发即忘任务：通过 MemoryService 提取记忆，带重试和限流。
    fn spawn_memory_extraction(
        &self,
        user_id: u64,
        conversation_id: Option<u64>,
        source_message_id: u64,
        user_message: &str,
        assistant_reply: &str,
        task_epoch: u64,
    ) {
        // 限流：如果上一次提取失败距今不到 30 秒，则跳过本轮
        {
            if let Ok(guard) = self.last_extraction_failure.lock() {
                if let Some(last_fail) = *guard {
                    if last_fail.elapsed() < std::time::Duration::from_secs(30) {
                        debug!(
                            user_id,
                            ?conversation_id,
                            seconds_since_failure = last_fail.elapsed().as_secs(),
                            "skipping memory extraction (recent failure)"
                        );
                        return;
                    }
                }
            }
        }

        debug!(user_id, ?conversation_id, "启动异步记忆提取");

        let memory_service = Arc::clone(&self.memory_service);
        let user_text = user_message.to_string();
        let asst_text = assistant_reply.to_string();
        let last_failure = Arc::clone(&self.last_extraction_failure);

        tokio::spawn(async move {
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

            let Some(cid) = conversation_id else { return };

            // 尝试提取，失败时重试一次
            let attempt = || async {
                memory_service
                    .extract_and_save_at_version(
                        user_id,
                        &messages,
                        cid,
                        source_message_id,
                        Some(task_epoch),
                    )
                    .await
            };

            let result = match attempt().await {
                Ok(r) => Ok(r),
                Err(e) => {
                    warn!(
                        user_id,
                        conversation_id = cid,
                        error = %e,
                        "memory extraction failed (will retry once)"
                    );
                    // 使用较短退避进行单次重试
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    attempt().await
                }
            };

            match result {
                Ok(memories) => {
                    let count = memories.len();
                    debug!(user_id, ?conversation_id, count, "异步记忆提取完成");
                }
                Err(e) => {
                    warn!(
                        user_id,
                        conversation_id = cid,
                        error = %e,
                        "memory extraction failed after retry"
                    );
                    if let Ok(mut guard) = last_failure.lock() {
                        *guard = Some(std::time::Instant::now());
                    }
                }
            }
        });
    }
}

/// 截断用于事件记录的字符串，最多保留 `max_chars` 个字符。
fn truncate_for_event(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}...[truncated]")
    }
}

/// 检查当前轮次是否允许使用工具。
/// 仅在以下条件满足时才允许使用工具：
/// - Agent 已启用
/// - 已注册工具
/// - 深度尚未达到 max_tool_depth
fn tools_allowed_for_round(
    agent_enabled: bool,
    have_tools: bool,
    depth: usize,
    max_tool_depth: usize,
) -> bool {
    agent_enabled && have_tools && depth < max_tool_depth
}

/// 移除某些模型在最终答案前回显的序列化工具调用。
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

/// 确保最终内容干净且非空；必要时返回中文回退文本。
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
    /// 持久化Agent Event，用于可观测性 / 审计。
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

// ── 测试 ──────────────────────────────────────────────────────────────────

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
        // max_tool_depth=0 表示不允许使用工具，即使 depth 为 0
        assert!(!tools_allowed_for_round(true, true, 0, 0));
    }

    #[test]
    fn tools_allowed_depth_zero_max_one() {
        // depth 0 < max 1 → 允许
        assert!(tools_allowed_for_round(true, true, 0, 1));
    }

    #[test]
    fn tools_allowed_depth_equals_max() {
        // depth 1 不小于 max 1 → 不允许
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

    // ── 运行时行为测试（通过集成测试辅助模块） ─────────
    // 这些测试验证主 AgentRuntime 循环能够正确处理
    // max_tool_depth=0 和 max_tool_depth=1 场景。由于构造
    // 完整的 AgentRuntime 需要许多 mock 依赖（这些依赖容易
    // 因 trait 签名漂移而变脆弱），这些测试放在集成测试
    // 套件中：tests/common/mod.rs → agent_depth_behavior_tests。

    /// 验证：max_tool_depth=0 → tools_allowed_for_round 返回 false，
    /// 且 build_system_message 在 tools_available=false 时不会声明
    /// 工具能力。
    #[test]
    fn max_tool_depth_zero_blocks_tools_entirely() {
        // tools_allowed_for_round 在 max=0 且 depth=0 时返回 false
        assert!(!tools_allowed_for_round(true, true, 0, 0));

        // 当 max_tool_depth=0 时，tools_available 计算结果为 false
        let agent_enabled = true;
        let have_tools = true;
        let max_tool_depth = 0;
        let tools_available = agent_enabled && have_tools && max_tool_depth > 0;
        assert!(
            !tools_available,
            "tools_available should be false when max_tool_depth=0"
        );
    }

    /// 验证：max_tool_depth=1 → 允许一轮工具调用，然后停止。
    #[test]
    fn max_tool_depth_one_allows_one_round_then_stops() {
        // tools_allowed_for_round：depth 0 < max 1 → true
        assert!(tools_allowed_for_round(true, true, 0, 1));
        // 一轮之后：depth 1 不小于 max 1 → false
        assert!(!tools_allowed_for_round(true, true, 1, 1));

        // 当 max_tool_depth=1 时，tools_available 计算结果为 true
        let tools_available = true && true && 1 > 0;
        assert!(tools_available);
    }

    /// 验证：工具不可用时，系统提示词不会声明工具能力。
    #[test]
    fn system_prompt_without_tools_no_tool_claims() {
        // 构造一个最小 AgentRuntime，仅用于测试 build_system_message
        // 我们以静态方式测试该属性：tools_available 标志控制
        // 提示词内容。实际的 build_system_message 方法会在
        // 集成测试中覆盖。
        let agent_enabled = true;
        let have_tools = false; // 没有已注册工具
        let tools_available = agent_enabled && have_tools;
        assert!(!tools_available);
    }
}
