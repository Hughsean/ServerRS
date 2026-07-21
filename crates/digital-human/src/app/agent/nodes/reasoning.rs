use crate::app::agent::graph::{
    AgentNode, NodeError, NodeErrorKind, NodeId, NodeResult, RouteKey, Router, RunContext,
    UsageDelta,
};
use crate::app::agent::message_adapter::chat_message_from_agent;
use crate::app::agent::reasoning_state::ReasoningState;
use crate::app::agent::response::fallback_reply;
use crate::app::agent::tool::{
    AgentTool, is_tool_call_argument_error, normalize_tool_arguments, truncate_for_event,
};
use crate::domain::agent::AgentEventRepoT;
use crate::domain::agent::{
    AgentAction, AgentMessage, AgentObservation, AgentState, AgentToolCall, AgentUpdate,
    NewAgentEvent,
};
use crate::domain::llm::{
    ChatCompletionRequest, ChatCompletionResponse, LlmError, LlmProvider, ReasoningConfig,
    ToolDefinition as LlmToolDefinition,
};
use crate::shared::error::AppError;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct ReasoningSettings {
    pub agent_enabled: bool,
    pub max_tool_depth: usize,
    pub temperature: f64,
    pub top_p: f64,
    pub reasoning: Option<ReasoningConfig>,
}

pub struct LlmCallNode {
    id: NodeId,
    llm: Arc<dyn LlmProvider>,
    tool_definitions: Vec<LlmToolDefinition>,
    settings: ReasoningSettings,
}

impl LlmCallNode {
    pub fn new(
        id: NodeId,
        llm: Arc<dyn LlmProvider>,
        tool_definitions: Vec<LlmToolDefinition>,
        settings: ReasoningSettings,
    ) -> Self {
        Self {
            id,
            llm,
            tool_definitions,
            settings,
        }
    }

    fn tools_allowed<B: ReasoningState>(&self, state: &AgentState<B>) -> bool {
        self.settings.agent_enabled
            && !self.tool_definitions.is_empty()
            && state.business().reasoning_tool_depth() < self.settings.max_tool_depth
    }

    fn request<B: ReasoningState>(
        &self,
        state: &AgentState<B>,
        tools_allowed: bool,
    ) -> ChatCompletionRequest {
        ChatCompletionRequest {
            messages: state
                .messages()
                .iter()
                .cloned()
                .map(chat_message_from_agent)
                .collect(),
            temperature: self.settings.temperature,
            top_p: self.settings.top_p,
            max_tokens: None,
            tools: tools_allowed.then(|| self.tool_definitions.clone()),
            reasoning: self.settings.reasoning.clone(),
        }
    }

    async fn call(
        &self,
        request: ChatCompletionRequest,
        tools_allowed: bool,
        context: &RunContext,
    ) -> Result<ChatCompletionResponse, NodeError> {
        context
            .budget()
            .reserve_llm_call()
            .map_err(NodeError::from_graph_run)?;
        if tools_allowed {
            self.llm
                .chat_with_tools(request, self.tool_definitions.clone())
                .await
        } else {
            self.llm.chat(request).await
        }
        .map_err(llm_node_error)
    }

    async fn final_without_tools(
        &self,
        mut request: ChatCompletionRequest,
        context: &RunContext,
    ) -> Result<ChatCompletionResponse, NodeError> {
        request.tools = None;
        request.messages.push(crate::domain::llm::ChatMessage {
            role: "user".into(),
            content: "本轮没有可用工具。请基于已有上下文直接用中文回复用户，不要调用工具。".into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });
        self.call(request, false, context).await
    }
}

#[async_trait]
impl<B: ReasoningState> AgentNode<B> for LlmCallNode {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        state: &AgentState<B>,
        context: &RunContext,
    ) -> Result<NodeResult<B::Update, B::Effect, B::SuspendData>, NodeError> {
        if !state.pending_actions().is_empty() {
            return Err(NodeError::new(
                NodeErrorKind::Invariant,
                "LLM 节点执行前仍有未消费的工具动作",
            ));
        }
        let tools_allowed = self.tools_allowed(state);
        let request = self.request(state, tools_allowed);
        let first_response = self.call(request.clone(), tools_allowed, context).await;

        let response = match first_response {
            Ok(response) => response,
            Err(error)
                if !self.tool_definitions.is_empty()
                    && is_tool_call_argument_error(error.message()) =>
            {
                warn!("LLM tool call failed; retrying without tools");
                match self
                    .call(
                        ChatCompletionRequest {
                            tools: None,
                            ..request.clone()
                        },
                        false,
                        context,
                    )
                    .await
                {
                    Ok(response) => {
                        let tokens = response_tokens(&response);
                        if !response.tool_calls.is_empty() {
                            warn!(
                                tool_call_count = response.tool_calls.len(),
                                "ignoring tool calls returned by no-tool fallback"
                            );
                        }
                        return Ok(final_assistant_result(response.content, tokens));
                    }
                    Err(error) if error.kind() == NodeErrorKind::Invariant => return Err(error),
                    Err(error) if error.kind() == NodeErrorKind::Cancelled => return Err(error),
                    Err(error) => {
                        warn!(error = %error, "LLM fallback without tools also failed");
                        return Ok(final_assistant_result(fallback_reply(), 0));
                    }
                }
            }
            Err(error) if error.kind() == NodeErrorKind::Invariant => return Err(error),
            Err(error) if error.kind() == NodeErrorKind::Cancelled => return Err(error),
            Err(error) => {
                warn!(error = %error, "LLM chat failed; using friendly fallback");
                return Ok(final_assistant_result(fallback_reply(), 0));
            }
        };
        let mut tokens = response_tokens(&response);

        if response.tool_calls.is_empty() {
            return Ok(final_assistant_result(response.content, tokens));
        }

        if !tools_allowed {
            if !response.content.trim().is_empty() {
                return Ok(final_assistant_result(response.content, tokens));
            }
            let final_response = match self.final_without_tools(request, context).await {
                Ok(response) => response,
                Err(error) if error.kind() == NodeErrorKind::Invariant => return Err(error),
                Err(error) if error.kind() == NodeErrorKind::Cancelled => return Err(error),
                Err(error) => {
                    warn!(error = %error, "final chat without tools failed");
                    return Ok(final_assistant_result(fallback_reply(), tokens));
                }
            };
            tokens = tokens.saturating_add(response_tokens(&final_response));
            return Ok(final_assistant_result(final_response.content, tokens));
        }

        let calls: Vec<AgentToolCall> = response
            .tool_calls
            .into_iter()
            .map(|call| AgentToolCall {
                id: call.id,
                name: call.name,
                arguments: call.arguments,
            })
            .collect();
        let actions = calls.iter().cloned().map(AgentAction::CallTool).collect();
        Ok(NodeResult::new(
            vec![
                AgentUpdate::AppendMessages(vec![AgentMessage::assistant(response.content, calls)]),
                AgentUpdate::ReplacePendingActions(actions),
            ],
            UsageDelta { tokens },
        ))
    }
}

pub struct ExecuteToolsNode {
    id: NodeId,
    tools: Vec<Arc<dyn AgentTool>>,
    event_repo: Arc<dyn AgentEventRepoT>,
}

impl ExecuteToolsNode {
    pub fn new(
        id: NodeId,
        tools: Vec<Arc<dyn AgentTool>>,
        event_repo: Arc<dyn AgentEventRepoT>,
    ) -> Self {
        Self {
            id,
            tools,
            event_repo,
        }
    }

    async fn execute_tool(
        &self,
        agent_context: &crate::domain::agent::AgentContext,
        name: &str,
        arguments: Value,
    ) -> Result<String, AppError> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.name() == name)
            .ok_or_else(|| AppError::Internal(format!("Unknown tool: {name}")))?;
        tool.execute(agent_context, arguments).await
    }
}

#[async_trait]
impl<B: ReasoningState> AgentNode<B> for ExecuteToolsNode {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        state: &AgentState<B>,
        context: &RunContext,
    ) -> Result<NodeResult<B::Update, B::Effect, B::SuspendData>, NodeError> {
        let calls: Vec<AgentToolCall> = state
            .pending_actions()
            .iter()
            .map(|action| match action {
                AgentAction::CallTool(call) => Ok(call.clone()),
                _ => Err(NodeError::new(
                    NodeErrorKind::Invariant,
                    "工具节点收到非 CallTool 动作",
                )),
            })
            .collect::<Result<_, _>>()?;
        if calls.is_empty() {
            return Err(NodeError::new(
                NodeErrorKind::Invariant,
                "工具节点没有待执行调用",
            ));
        }
        let call_count = u32::try_from(calls.len())
            .map_err(|_| NodeError::new(NodeErrorKind::Invariant, "单节点工具调用数量溢出"))?;
        context
            .budget()
            .reserve_tool_calls(call_count)
            .map_err(NodeError::from_graph_run)?;

        let turn = state.business();
        let agent_context = turn.reasoning_context().ok_or_else(|| {
            NodeError::new(NodeErrorKind::Invariant, "执行工具前 AgentContext 尚未设置")
        })?;
        let mut messages = Vec::with_capacity(calls.len());
        let mut observations = Vec::with_capacity(calls.len());

        for call in calls {
            if context.cancellation().is_cancelled() {
                return Err(NodeError::new(NodeErrorKind::Cancelled, "图运行已取消"));
            }
            let normalized_arguments = normalize_tool_arguments(&call.arguments);
            debug!(
                tool_name = %call.name,
                arguments_are_object = normalized_arguments.is_object(),
                "processing graph tool call"
            );
            let result = self
                .execute_tool(agent_context, &call.name, normalized_arguments.clone())
                .await;
            let succeeded = result.is_ok();
            let result_string = match &result {
                Ok(value) => value.clone(),
                Err(error) => format!("Tool error: {error}"),
            };
            let result_preview = truncate_for_event(&result_string, 2000);

            let _ = self
                .event_repo
                .log_event(NewAgentEvent {
                    user_id: turn.reasoning_user_id(),
                    conversation_id: turn.reasoning_conversation_id(),
                    event_type: "tool_call".into(),
                    tool_name: Some(call.name.clone()),
                    payload: serde_json::json!({
                        "tool": call.name,
                        "arguments": normalized_arguments,
                        "raw_arguments": call.arguments,
                        "ok": succeeded,
                        "result_preview": result_preview,
                        "error": result.as_ref().err().map(ToString::to_string),
                    }),
                })
                .await;

            info!(tool = %call.name, succeeded, "graph agent tool completed");
            messages.push(AgentMessage::tool(
                call.id.clone(),
                call.name.clone(),
                result_string.clone(),
            ));
            observations.push(AgentObservation {
                call: AgentToolCall {
                    id: call.id,
                    name: call.name,
                    arguments: normalized_arguments,
                },
                result: result_string,
                succeeded,
            });
        }

        Ok(NodeResult::new(
            vec![
                AgentUpdate::AppendMessages(messages),
                AgentUpdate::AppendObservations(observations),
                AgentUpdate::ReplacePendingActions(Vec::new()),
                AgentUpdate::Business(B::increment_reasoning_tool_depth()),
            ],
            UsageDelta::default(),
        ))
    }
}

pub struct FinalWithoutToolsNode {
    id: NodeId,
    llm: Arc<dyn LlmProvider>,
    settings: ReasoningSettings,
}

impl FinalWithoutToolsNode {
    pub fn new(id: NodeId, llm: Arc<dyn LlmProvider>, settings: ReasoningSettings) -> Self {
        Self { id, llm, settings }
    }
}

#[async_trait]
impl<B: ReasoningState> AgentNode<B> for FinalWithoutToolsNode {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        state: &AgentState<B>,
        context: &RunContext,
    ) -> Result<NodeResult<B::Update, B::Effect, B::SuspendData>, NodeError> {
        context
            .budget()
            .reserve_llm_call()
            .map_err(NodeError::from_graph_run)?;
        let mut messages: Vec<_> = state
            .messages()
            .iter()
            .cloned()
            .map(chat_message_from_agent)
            .collect();
        messages.push(crate::domain::llm::ChatMessage {
            role: "user".into(),
            content:
                "本轮工具已经用完。请基于已有上下文和工具结果，直接用中文回复用户，不要再调用工具。"
                    .into(),
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
            reasoning: self.settings.reasoning.clone(),
        };

        match self.llm.chat(request).await {
            Ok(response) => {
                let tokens = response_tokens(&response);
                Ok(final_assistant_result(response.content, tokens))
            }
            Err(error) => {
                warn!(error = %error, "final chat without tools failed");
                Ok(final_assistant_result(fallback_reply(), 0))
            }
        }
    }
}

pub struct CompletionNode {
    id: NodeId,
}

impl CompletionNode {
    pub fn new(id: NodeId) -> Self {
        Self { id }
    }
}

#[async_trait]
impl<B: ReasoningState> AgentNode<B> for CompletionNode {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        _state: &AgentState<B>,
        _context: &RunContext,
    ) -> Result<NodeResult<B::Update, B::Effect, B::SuspendData>, NodeError> {
        Ok(NodeResult::empty())
    }
}

pub struct LlmResultRouter;

impl<B: ReasoningState> Router<B> for LlmResultRouter {
    fn known_routes(&self) -> Vec<RouteKey> {
        vec![route("tools_requested"), route("final_response")]
    }

    fn select(&self, state: &AgentState<B>) -> Result<RouteKey, NodeError> {
        if state.pending_actions().is_empty() {
            Ok(route("final_response"))
        } else {
            Ok(route("tools_requested"))
        }
    }
}

pub struct ToolDepthRouter {
    max_tool_depth: usize,
}

impl ToolDepthRouter {
    pub fn new(max_tool_depth: usize) -> Self {
        Self { max_tool_depth }
    }
}

impl<B: ReasoningState> Router<B> for ToolDepthRouter {
    fn known_routes(&self) -> Vec<RouteKey> {
        vec![route("continue"), route("depth_exhausted")]
    }

    fn select(&self, state: &AgentState<B>) -> Result<RouteKey, NodeError> {
        if state.business().reasoning_tool_depth() >= self.max_tool_depth {
            Ok(route("depth_exhausted"))
        } else {
            Ok(route("continue"))
        }
    }
}

pub struct FinalResponseRouter;

impl<B: ReasoningState> Router<B> for FinalResponseRouter {
    fn known_routes(&self) -> Vec<RouteKey> {
        vec![route("final_response")]
    }

    fn select(&self, _state: &AgentState<B>) -> Result<RouteKey, NodeError> {
        Ok(route("final_response"))
    }
}

fn route(value: &str) -> RouteKey {
    RouteKey::try_from(value).expect("reasoning route keys are static and valid")
}

fn response_tokens(response: &ChatCompletionResponse) -> u64 {
    response
        .usage
        .as_ref()
        .map(|usage| u64::from(usage.total_tokens))
        .unwrap_or(0)
}

fn llm_node_error(error: LlmError) -> NodeError {
    let kind = match &error {
        LlmError::Connection(_) => NodeErrorKind::Transient,
        LlmError::Timeout(_) => NodeErrorKind::Timeout,
        LlmError::RateLimited(_) => NodeErrorKind::RateLimited,
        LlmError::InvalidResponse(_) | LlmError::ProviderError(_) | LlmError::EmbeddingError(_) => {
            NodeErrorKind::Permanent
        }
    };
    NodeError::new(kind, format!("LLM 调用失败: {error}"))
}

fn final_assistant_result<U, E, S>(content: String, tokens: u64) -> NodeResult<U, E, S> {
    NodeResult::new(
        vec![
            AgentUpdate::AppendMessages(vec![AgentMessage::assistant(content, Vec::new())]),
            AgentUpdate::ReplacePendingActions(Vec::new()),
        ],
        UsageDelta { tokens },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agent::chat_state::{ChatTurnState, ChatTurnUpdate};
    use crate::app::agent::graph::Router;
    use crate::domain::agent::{AgentAction, AgentState, AgentToolCall, AgentUpdate};
    use crate::domain::llm::LlmError;

    fn state() -> AgentState<ChatTurnState> {
        AgentState::new(ChatTurnState::new(1, 2, "hello".into(), None, None, vec![]))
    }

    #[test]
    fn llm_router_uses_semantic_pending_action_route() {
        let mut state = state();
        state
            .apply_updates(vec![AgentUpdate::ReplacePendingActions(vec![
                AgentAction::CallTool(AgentToolCall {
                    id: "call-1".into(),
                    name: "clock".into(),
                    arguments: serde_json::json!({}),
                }),
            ])])
            .unwrap();

        let route = LlmResultRouter.select(&state).unwrap();
        assert_eq!(route.as_str(), "tools_requested");
    }

    #[test]
    fn tool_depth_router_observes_incremented_business_state() {
        let mut state = state();
        state
            .apply_updates(vec![AgentUpdate::Business(
                ChatTurnUpdate::IncrementToolDepth,
            )])
            .unwrap();

        let route = ToolDepthRouter::new(1).select(&state).unwrap();
        assert_eq!(route.as_str(), "depth_exhausted");
    }

    #[test]
    fn llm_errors_keep_their_runtime_classification() {
        assert_eq!(
            llm_node_error(LlmError::Timeout("slow".into())).kind(),
            NodeErrorKind::Timeout
        );
        assert_eq!(
            llm_node_error(LlmError::RateLimited("busy".into())).kind(),
            NodeErrorKind::RateLimited
        );
        assert_eq!(
            llm_node_error(LlmError::InvalidResponse("bad json".into())).kind(),
            NodeErrorKind::Permanent
        );
    }
}
