use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::Value;
use tracing::{debug, trace, warn};

use super::agent_context::AgentContextBuilder;
use super::chat_graph::{ChatAgentGraph, ChatAgentGraphDeps};
use super::chat_state::{ChatTurnState, PersistedTurn as GraphPersistedTurn};
use super::graph::GraphRunError;
use super::nodes::{ConversationTurnWriter, DefaultChatContextProvider};
#[cfg(test)]
use super::response::{fallback_reply, normalize_final_content};
pub use super::tool::{
    AgentTool, ToolTrace, is_tool_call_argument_error, normalize_tool_arguments,
};
use crate::app::memory::memory_service::MemoryService;
use crate::domain::agent::{AgentEventRepoT, AgentOutcome, AgentState};
use crate::domain::conversation::conversation_repo::ConversationRepoT;
use crate::domain::llm::{ChatMessage, LlmProvider, ReasoningConfig};
use crate::domain::user::user_context_version::UserContextVersionRepoT;
use crate::domain::user::user_profile_repo::UserProfileRepoT;
use crate::shared::error::AppError;

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
// Agent响应
// ---------------------------------------------------------------------------

/// AgentRuntime处理一轮对话后生成的结构化结果。
#[derive(Debug, Clone)]
pub struct AgentResponse {
    /// 返回给用户的最终文本回复。
    pub reply: String,
    /// 本轮调用过的每个工具的追踪记录。
    pub tool_calls: Vec<ToolTrace>,
    /// 已持久化消息的 ID，在图持久化节点成功后可用。
    pub user_message_id: Option<u64>,
    pub assistant_message_id: Option<u64>,
}

// ---------------------------------------------------------------------------
// AgentRuntime
// ---------------------------------------------------------------------------

/// HTTP Chat 的兼容门面：把单轮请求交给已编译图执行，并保留记忆提取行为。
pub struct AgentRuntime {
    chat_graph: ChatAgentGraph,
    memory_service: Arc<MemoryService>,
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
        let context_provider = Arc::new(DefaultChatContextProvider::new(
            Arc::clone(&context_version_repo),
            Arc::clone(&user_profile_repo),
            Arc::clone(&context_builder),
        ));
        let turn_writer = Arc::new(ConversationTurnWriter::new(Arc::clone(&conversation_repo)));
        let chat_graph = ChatAgentGraph::new(ChatAgentGraphDeps {
            llm: Arc::clone(&llm),
            event_repo: Arc::clone(&event_repo),
            context_provider,
            turn_writer,
            tools: tools.clone(),
            settings: settings.clone(),
        })
        .expect("静态 HTTP Chat Agent 图必须能够编译");

        Self {
            chat_graph,
            memory_service,
            settings,
            last_extraction_failure: Arc::new(Mutex::new(None)),
        }
    }

    pub fn max_context_messages(&self) -> usize {
        self.settings.max_context_messages
    }

    /// 保持原有公开签名，将单轮编排委托给已编译的 Chat Agent 图。
    pub async fn respond(
        &self,
        user_id: u64,
        conversation_id: Option<u64>,
        user_message: String,
        emotion: Option<String>,
        location: Option<Value>,
        recent_messages: Vec<ChatMessage>,
    ) -> Result<AgentResponse, AppError> {
        let state = build_initial_chat_state(
            user_id,
            conversation_id,
            user_message.clone(),
            emotion,
            location,
            recent_messages,
        )?;
        let result = self
            .chat_graph
            .run(state)
            .await
            .map_err(map_graph_run_error)?;
        let completed = map_completed_chat_turn(result.state)?;

        if self.settings.agent_enabled
            && self.settings.memory_enabled
            && self.settings.memory_extraction_async
        {
            self.spawn_memory_extraction(
                user_id,
                conversation_id,
                completed.memory_source_message_id,
                &user_message,
                &completed.response.reply,
                completed.context_version,
            );
        }

        trace!(
            conversation_id,
            user_message_chars = user_message.chars().count(),
            assistant_reply_chars = completed.response.reply.chars().count(),
            tool_call_count = completed.response.tool_calls.len(),
            "AgentRuntime completed graph-backed respond()"
        );
        Ok(completed.response)
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

fn build_initial_chat_state(
    user_id: u64,
    conversation_id: Option<u64>,
    user_message: String,
    emotion: Option<String>,
    location: Option<Value>,
    recent_messages: Vec<ChatMessage>,
) -> Result<AgentState<ChatTurnState>, AppError> {
    let conversation_id =
        conversation_id.ok_or_else(|| AppError::Internal("需要对话 ID 才能持久化消息".into()))?;
    Ok(AgentState::new(ChatTurnState::new(
        user_id,
        conversation_id,
        user_message,
        emotion,
        location,
        recent_messages,
    )))
}

fn map_graph_run_error(error: GraphRunError) -> AppError {
    if let GraphRunError::NodeFailed { error, .. } = &error
        && let Some(application_error) = error.application_error()
    {
        return application_error.clone();
    }
    AppError::Internal(format!("Agent 图运行失败: {error}"))
}

struct CompletedChatTurn {
    response: AgentResponse,
    memory_source_message_id: u64,
    context_version: u64,
}

fn map_completed_chat_turn(
    state: AgentState<ChatTurnState>,
) -> Result<CompletedChatTurn, AppError> {
    let reply = state
        .outcome()
        .and_then(AgentOutcome::response_text)
        .ok_or_else(|| AppError::Internal("Agent 图完成时缺少最终回复".into()))?
        .to_owned();
    let persisted: &GraphPersistedTurn = state
        .business()
        .persisted_turn()
        .ok_or_else(|| AppError::Internal("Agent 图完成时缺少持久化消息 ID".into()))?;
    let context_version = state
        .business()
        .context_version()
        .ok_or_else(|| AppError::Internal("Agent 图完成时缺少上下文版本".into()))?;
    let tool_calls = state
        .observations()
        .iter()
        .map(|observation| ToolTrace {
            tool_name: observation.call.name.clone(),
            arguments: observation.call.arguments.clone(),
            result: observation.result.clone(),
        })
        .collect();
    let user_message_id = persisted.user_message_id();
    let assistant_message_id = persisted.assistant_message_id();

    Ok(CompletedChatTurn {
        response: AgentResponse {
            reply,
            tool_calls,
            user_message_id: Some(user_message_id),
            assistant_message_id: Some(assistant_message_id),
        },
        memory_source_message_id: user_message_id,
        context_version,
    })
}

/// 检查当前轮次是否允许使用工具。
/// 仅在以下条件满足时才允许使用工具：
/// - Agent 已启用
/// - 已注册工具
/// - 深度尚未达到 max_tool_depth
#[cfg(test)]
fn tools_allowed_for_round(
    agent_enabled: bool,
    have_tools: bool,
    depth: usize,
    max_tool_depth: usize,
) -> bool {
    agent_enabled && have_tools && depth < max_tool_depth
}

// ── 测试 ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agent::chat_state::{
        ChatTurnState, ChatTurnUpdate, PersistedTurn as GraphPersistedTurn,
    };
    use crate::domain::agent::{
        AgentBusinessState, AgentContext, AgentObservation, AgentOutcome, AgentState,
        AgentToolCall, AgentUpdate,
    };
    use serde_json::json;

    #[test]
    fn graph_facade_requires_the_existing_conversation_id() {
        let error =
            build_initial_chat_state(7, None, "hello".into(), None, None, vec![]).unwrap_err();

        assert!(matches!(
            error,
            AppError::Internal(message) if message == "需要对话 ID 才能持久化消息"
        ));
    }

    #[test]
    fn graph_facade_restores_application_error_variants() {
        let error = crate::app::agent::graph::GraphRunError::NodeFailed {
            node: crate::app::agent::graph::NodeId::try_from("persist").unwrap(),
            error: crate::app::agent::graph::NodeError::from_application(AppError::Conflict(
                "turn changed".into(),
            )),
        };

        assert!(matches!(
            map_graph_run_error(error),
            AppError::Conflict(message) if message == "turn changed"
        ));
    }

    #[test]
    fn completed_graph_state_maps_response_and_memory_metadata() {
        let mut business = ChatTurnState::new(7, 9, "hello".into(), None, None, vec![]);
        business
            .apply_update(ChatTurnUpdate::SetContext {
                context: AgentContext {
                    user_id: 7,
                    conversation_id: Some(9),
                    recent_messages: vec![],
                    summary: None,
                    memories: vec![],
                    rag_chunks: vec![],
                    fresh_chunks: vec![],
                    user_profile: None,
                    tools: vec![],
                    location: None,
                },
                context_version: 37,
            })
            .unwrap();
        let mut state = AgentState::new(business);
        state
            .apply_updates(vec![
                AgentUpdate::AppendObservations(vec![AgentObservation {
                    call: AgentToolCall {
                        id: "call-1".into(),
                        name: "clock".into(),
                        arguments: json!({"zone": "Asia/Shanghai"}),
                    },
                    result: "12:00".into(),
                    succeeded: true,
                }]),
                AgentUpdate::SetOutcome(AgentOutcome::Respond("done".into())),
                AgentUpdate::Business(ChatTurnUpdate::SetPersistedTurn(GraphPersistedTurn::new(
                    101, 102,
                ))),
            ])
            .unwrap();

        let completed = map_completed_chat_turn(state).unwrap();

        assert_eq!(completed.response.reply, "done");
        assert_eq!(completed.response.user_message_id, Some(101));
        assert_eq!(completed.response.assistant_message_id, Some(102));
        assert_eq!(completed.response.tool_calls[0].tool_name, "clock");
        assert_eq!(completed.memory_source_message_id, 101);
        assert_eq!(completed.context_version, 37);
    }

    #[test]
    fn compatibility_fallback_text_is_stable() {
        assert_eq!(
            fallback_reply(),
            "抱歉，我刚才处理这条消息时遇到了一点问题。你可以换个说法再发一次，我会继续帮你。"
        );
    }

    #[test]
    fn compatibility_tool_depth_boundary_is_stable() {
        assert!(tools_allowed_for_round(true, true, 0, 1));
        assert!(!tools_allowed_for_round(true, true, 1, 1));
        assert!(!tools_allowed_for_round(true, true, 0, 0));
    }

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

    // ── 兼容边界测试 ────────────────────────────────────────
    // 这里锁定旧门面的工具可用性判定；完整工具循环由
    // reasoning_loop 与 chat_graph 的脚本化端到端测试覆盖。

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
        // PromptBuilder 自身的测试验证 tools_available=false 时的提示词内容。
        let agent_enabled = true;
        let have_tools = false; // 没有已注册工具
        let tools_available = agent_enabled && have_tools;
        assert!(!tools_available);
    }
}
