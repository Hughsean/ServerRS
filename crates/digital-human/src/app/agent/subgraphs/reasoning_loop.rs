use crate::app::agent::graph::{GraphBuildError, GraphFragment, NodeId, RouteKey, TransitionRule};
use crate::app::agent::nodes::reasoning::{
    ApprovalGateNode, CompletionNode, ExecuteToolsNode, FinalResponseRouter, FinalWithoutToolsNode,
    LlmCallNode, LlmResultRouter, ReasoningSettings, ToolDepthRouter,
};
use crate::app::agent::reasoning_state::ReasoningState;
use crate::app::agent::tool::AgentTool;
use crate::domain::agent::AgentEventRepoT;
use crate::domain::llm::{LlmProvider, ToolDefinition};
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct ReasoningLoopDeps {
    pub llm: Arc<dyn LlmProvider>,
    pub event_repo: Arc<dyn AgentEventRepoT>,
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub settings: ReasoningSettings,
    pub approval_required_tools: Vec<String>,
}

pub fn build_reasoning_loop<B: ReasoningState>(
    dependencies: ReasoningLoopDeps,
) -> Result<GraphFragment<B>, GraphBuildError> {
    let tool_definitions: Vec<ToolDefinition> = dependencies
        .tools
        .iter()
        .map(|tool| ToolDefinition {
            name: tool.name().to_owned(),
            description: tool.description().to_owned(),
            parameters: tool.parameters(),
        })
        .collect();
    let mut fragment: GraphFragment<B> = GraphFragment::new();
    fragment.add_node(Arc::new(LlmCallNode::new(
        node("llm"),
        Arc::clone(&dependencies.llm),
        tool_definitions,
        dependencies.settings.clone(),
    )))?;
    fragment.add_node(Arc::new(ExecuteToolsNode::new(
        node("tools"),
        dependencies.tools,
        dependencies.event_repo,
    )))?;
    fragment.add_node(Arc::new(ApprovalGateNode::new(
        node("approval_gate"),
        dependencies.approval_required_tools,
    )))?;
    fragment.add_node(Arc::new(FinalWithoutToolsNode::new(
        node("final_without_tools"),
        dependencies.llm,
        dependencies.settings.clone(),
    )))?;
    fragment.add_node(Arc::new(CompletionNode::new(node("complete"))))?;
    fragment.set_entry(node("llm"));

    fragment.set_transition(
        node("llm"),
        TransitionRule::Branch {
            router: Arc::new(LlmResultRouter),
            targets: BTreeMap::from([
                (route("tools_requested"), node("approval_gate")),
                (route("final_response"), node("complete")),
            ]),
        },
    )?;
    fragment.set_transition(node("approval_gate"), TransitionRule::Goto(node("tools")))?;
    fragment.set_transition(
        node("tools"),
        TransitionRule::Branch {
            router: Arc::new(ToolDepthRouter::new(dependencies.settings.max_tool_depth)),
            targets: BTreeMap::from([
                (route("continue"), node("llm")),
                (route("depth_exhausted"), node("final_without_tools")),
            ]),
        },
    )?;
    fragment.set_transition(
        node("final_without_tools"),
        TransitionRule::Goto(node("complete")),
    )?;
    fragment.set_transition(
        node("complete"),
        TransitionRule::Branch {
            router: Arc::new(FinalResponseRouter),
            targets: BTreeMap::new(),
        },
    )?;
    fragment.declare_exit("final_response", node("complete"), route("final_response"))?;
    Ok(fragment)
}

fn node(value: &str) -> NodeId {
    NodeId::try_from(value).expect("reasoning node IDs are static and valid")
}

fn route(value: &str) -> RouteKey {
    RouteKey::try_from(value).expect("reasoning route keys are static and valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agent::chat_state::{ChatTurnState, ChatTurnUpdate};
    use crate::app::agent::graph::{
        GraphDefinition, GraphId, GraphPolicy, GraphRuntime, NoEffect, NodeId, RunBudget,
        TransitionRule,
    };
    use crate::app::agent::nodes::NormalizeReplyNode;
    use crate::app::agent::reasoning_state::ReasoningState;
    use crate::app::agent::tool::AgentTool;
    use crate::domain::agent::{
        AgentBusinessState, AgentContext, AgentEvent, AgentEventRepoT, AgentOutcome, AgentState,
        AgentStateError, AgentUpdate, NewAgentEvent,
    };
    use crate::domain::llm::{
        ChatCompletionRequest, ChatCompletionResponse, LlmError, LlmProvider, TokenUsage, ToolCall,
        ToolDefinition,
    };
    use crate::shared::error::AppError;
    use async_trait::async_trait;
    use chrono::Utc;
    use serde_json::Value;
    use std::collections::VecDeque;
    use std::num::NonZeroU32;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Debug, Clone)]
    struct RecordedLlmCall {
        with_tools: bool,
        messages: Vec<crate::domain::llm::ChatMessage>,
    }

    struct ScriptedLlm {
        responses: Mutex<VecDeque<Result<ChatCompletionResponse, LlmError>>>,
        calls: Mutex<Vec<RecordedLlmCall>>,
    }

    impl ScriptedLlm {
        fn new(responses: Vec<Result<ChatCompletionResponse, LlmError>>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn record_and_pop(
            &self,
            request: ChatCompletionRequest,
            with_tools: bool,
        ) -> Result<ChatCompletionResponse, LlmError> {
            self.calls.lock().unwrap().push(RecordedLlmCall {
                with_tools,
                messages: request.messages,
            });
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted LLM response")
        }
    }

    #[async_trait]
    impl LlmProvider for ScriptedLlm {
        async fn chat(
            &self,
            request: ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, LlmError> {
            self.record_and_pop(request, false)
        }

        async fn chat_with_tools(
            &self,
            request: ChatCompletionRequest,
            _tools: Vec<ToolDefinition>,
        ) -> Result<ChatCompletionResponse, LlmError> {
            self.record_and_pop(request, true)
        }
    }

    struct FakeTool {
        name: String,
        result: Result<String, AppError>,
        arguments: Mutex<Vec<Value>>,
    }

    impl FakeTool {
        fn ok(name: &str, result: &str) -> Self {
            Self {
                name: name.into(),
                result: Ok(result.into()),
                arguments: Mutex::new(Vec::new()),
            }
        }

        fn failing(name: &str) -> Self {
            Self {
                name: name.into(),
                result: Err(AppError::Internal("tool failed".into())),
                arguments: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl AgentTool for FakeTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "fake"
        }

        fn parameters(&self) -> Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(&self, _context: &AgentContext, args: Value) -> Result<String, AppError> {
            self.arguments.lock().unwrap().push(args);
            self.result.clone()
        }
    }

    #[derive(Default)]
    struct FakeEventRepo {
        events: Mutex<Vec<NewAgentEvent>>,
    }

    #[async_trait]
    impl AgentEventRepoT for FakeEventRepo {
        async fn log_event(&self, event: NewAgentEvent) -> AgentEvent {
            self.events.lock().unwrap().push(event.clone());
            AgentEvent {
                event_id: 1,
                user_id: event.user_id,
                conversation_id: event.conversation_id,
                trace_id: None,
                event_type: event.event_type,
                tool_name: event.tool_name,
                payload: event.payload,
                created_at: Utc::now(),
            }
        }
    }

    struct RunHarness {
        result: crate::app::agent::graph::GraphRunResult<ChatTurnState>,
        llm: Arc<ScriptedLlm>,
        events: Arc<FakeEventRepo>,
    }

    #[derive(Clone)]
    struct AlternateReasoningState {
        context: AgentContext,
        depth: usize,
    }

    enum AlternateReasoningUpdate {
        IncrementDepth,
    }

    impl AgentBusinessState for AlternateReasoningState {
        type Update = AlternateReasoningUpdate;
        type Effect = NoEffect<AlternateReasoningUpdate>;
        type SuspendData = ();
        type ResumeInput = ();

        fn resume_updates(
            _input: Self::ResumeInput,
        ) -> Vec<crate::domain::agent::AgentUpdate<Self::Update>> {
            Vec::new()
        }

        fn apply_update(&mut self, update: Self::Update) -> Result<(), AgentStateError> {
            match update {
                AlternateReasoningUpdate::IncrementDepth => self.depth += 1,
            }
            Ok(())
        }
    }

    impl ReasoningState for AlternateReasoningState {
        fn reasoning_context(&self) -> Option<&AgentContext> {
            Some(&self.context)
        }

        fn reasoning_tool_depth(&self) -> usize {
            self.depth
        }

        fn reasoning_user_id(&self) -> u64 {
            self.context.user_id
        }

        fn reasoning_conversation_id(&self) -> Option<u64> {
            self.context.conversation_id
        }

        fn increment_reasoning_tool_depth() -> Self::Update {
            AlternateReasoningUpdate::IncrementDepth
        }
    }

    async fn run_reasoning(
        responses: Vec<Result<ChatCompletionResponse, LlmError>>,
        tools: Vec<Arc<dyn AgentTool>>,
        max_tool_depth: usize,
        agent_enabled: bool,
    ) -> RunHarness {
        let llm = Arc::new(ScriptedLlm::new(responses));
        let events = Arc::new(FakeEventRepo::default());
        let settings = ReasoningSettings {
            agent_enabled,
            max_tool_depth,
            temperature: 0.2,
            top_p: 0.8,
            reasoning: None,
        };
        let fragment = build_reasoning_loop(ReasoningLoopDeps {
            llm: llm.clone(),
            event_repo: events.clone(),
            tools,
            settings,
            approval_required_tools: Vec::new(),
        })
        .unwrap();

        let mut graph = GraphDefinition::new(GraphId::try_from("reasoning-test").unwrap());
        graph
            .add_node(Arc::new(NormalizeReplyNode::new(node_id("normalize"))))
            .unwrap();
        let mounted = graph.mount("reasoning", fragment).unwrap();
        graph
            .connect_exit(
                mounted.exit("final_response").unwrap(),
                node_id("normalize"),
            )
            .unwrap();
        graph.set_entry(mounted.entry().clone());
        graph
            .set_transition(node_id("normalize"), TransitionRule::End)
            .unwrap();
        let compiled = graph
            .compile(GraphPolicy::new(NonZeroU32::new(32).unwrap()))
            .unwrap();

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
                context_version: 1,
            })
            .unwrap();
        let mut state = AgentState::new(business);
        state
            .apply_updates(vec![AgentUpdate::AppendMessages(vec![
                crate::domain::agent::AgentMessage::system("system"),
                crate::domain::agent::AgentMessage::user("hello"),
            ])])
            .unwrap();

        let budget = RunBudget::new(NonZeroU32::new(32).unwrap(), Duration::from_secs(30))
            .with_llm_calls(8)
            .with_tool_calls(8)
            .with_tokens(10_000);
        let result = GraphRuntime::new(compiled)
            .run(state, budget)
            .await
            .unwrap();
        RunHarness {
            result,
            llm,
            events,
        }
    }

    fn node_id(value: &str) -> NodeId {
        NodeId::try_from(value).unwrap()
    }

    fn text_response(content: &str) -> Result<ChatCompletionResponse, LlmError> {
        Ok(ChatCompletionResponse {
            content: content.into(),
            tool_calls: vec![],
            finish_reason: "stop".into(),
            usage: Some(TokenUsage {
                prompt_tokens: 2,
                completion_tokens: 3,
                total_tokens: 5,
            }),
        })
    }

    fn tool_response(calls: &[(&str, &str, Value)]) -> Result<ChatCompletionResponse, LlmError> {
        Ok(ChatCompletionResponse {
            content: String::new(),
            tool_calls: calls
                .iter()
                .map(|(id, name, arguments)| ToolCall {
                    id: (*id).into(),
                    name: (*name).into(),
                    arguments: arguments.clone(),
                })
                .collect(),
            finish_reason: "tool_calls".into(),
            usage: None,
        })
    }

    fn tool(name: &str) -> Arc<FakeTool> {
        Arc::new(FakeTool::ok(name, "12:00"))
    }

    #[test]
    fn reasoning_fragment_accepts_an_alternate_business_state() {
        let fragment: GraphFragment<AlternateReasoningState> =
            build_reasoning_loop::<AlternateReasoningState>(ReasoningLoopDeps {
                llm: Arc::new(ScriptedLlm::new(vec![])),
                event_repo: Arc::new(FakeEventRepo::default()),
                tools: vec![],
                settings: ReasoningSettings {
                    agent_enabled: true,
                    max_tool_depth: 2,
                    temperature: 0.2,
                    top_p: 0.8,
                    reasoning: None,
                },
                approval_required_tools: Vec::new(),
            })
            .unwrap();
        let mut graph = GraphDefinition::new(GraphId::try_from("alternate-reasoning").unwrap());
        let mounted = graph.mount("reasoning", fragment).unwrap();

        assert_eq!(mounted.entry(), &node_id("reasoning.llm"));
        assert!(mounted.exit("final_response").is_some());
    }

    #[tokio::test]
    async fn no_tool_response_exits_with_assistant_content() {
        let clock = tool("clock");
        let harness = run_reasoning(vec![text_response("done")], vec![clock], 3, true).await;

        assert_eq!(
            harness
                .result
                .state
                .outcome()
                .and_then(AgentOutcome::response_text),
            Some("done")
        );
        assert!(harness.result.state.observations().is_empty());
        assert_eq!(harness.result.usage.llm_calls, 1);
        assert_eq!(harness.result.usage.tokens, 5);
        assert!(harness.llm.calls.lock().unwrap()[0].with_tools);
    }

    #[tokio::test]
    async fn one_tool_round_returns_to_llm() {
        let clock = tool("clock");
        let harness = run_reasoning(
            vec![
                tool_response(&[("call-1", "clock", serde_json::json!("{}"))]),
                text_response("done"),
            ],
            vec![clock.clone()],
            3,
            true,
        )
        .await;

        assert_eq!(harness.result.state.observations().len(), 1);
        assert_eq!(harness.result.state.business().tool_depth(), 1);
        assert_eq!(clock.arguments.lock().unwrap()[0], serde_json::json!({}));
        assert_eq!(harness.llm.calls.lock().unwrap().len(), 2);
        assert_eq!(
            harness
                .result
                .state
                .outcome()
                .and_then(AgentOutcome::response_text),
            Some("done")
        );
    }

    #[tokio::test]
    async fn multiple_tool_calls_are_executed_and_logged() {
        let first = tool("first");
        let second = tool("second");
        let harness = run_reasoning(
            vec![
                tool_response(&[
                    ("call-1", "first", serde_json::json!({"a": 1})),
                    ("call-2", "second", serde_json::json!({"b": 2})),
                ]),
                text_response("done"),
            ],
            vec![first, second],
            3,
            true,
        )
        .await;

        assert_eq!(harness.result.state.observations().len(), 2);
        assert_eq!(harness.result.usage.tool_calls, 2);
        let events = harness.events.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "tool_call");
        assert_eq!(events[0].payload["ok"], true);
    }

    #[tokio::test]
    async fn unknown_tool_is_an_observation_not_a_graph_failure() {
        let known = tool("known");
        let harness = run_reasoning(
            vec![
                tool_response(&[("call-1", "unknown", serde_json::json!({}))]),
                text_response("recovered"),
            ],
            vec![known],
            3,
            true,
        )
        .await;

        assert!(!harness.result.state.observations()[0].succeeded);
        assert!(
            harness.result.state.observations()[0]
                .result
                .contains("Unknown tool")
        );
    }

    #[tokio::test]
    async fn tool_error_is_returned_to_the_llm_as_an_observation() {
        let failing: Arc<dyn AgentTool> = Arc::new(FakeTool::failing("clock"));
        let harness = run_reasoning(
            vec![
                tool_response(&[("call-1", "clock", serde_json::json!({}))]),
                text_response("handled"),
            ],
            vec![failing],
            3,
            true,
        )
        .await;

        let observation = &harness.result.state.observations()[0];
        assert!(!observation.succeeded);
        assert!(observation.result.contains("Tool error"));
    }

    #[tokio::test]
    async fn depth_zero_never_sends_tool_definitions() {
        let harness =
            run_reasoning(vec![text_response("done")], vec![tool("clock")], 0, true).await;

        assert!(!harness.llm.calls.lock().unwrap()[0].with_tools);
        assert_eq!(harness.result.state.business().tool_depth(), 0);
    }

    #[tokio::test]
    async fn depth_exhaustion_uses_final_call_without_tools() {
        let harness = run_reasoning(
            vec![
                tool_response(&[("call-1", "clock", serde_json::json!({}))]),
                text_response("final summary"),
            ],
            vec![tool("clock")],
            1,
            true,
        )
        .await;

        let calls = harness.llm.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].with_tools);
        assert!(!calls[1].with_tools);
        assert!(
            calls[1]
                .messages
                .last()
                .unwrap()
                .content
                .contains("工具已经用完")
        );
        assert_eq!(
            harness
                .result
                .state
                .outcome()
                .and_then(AgentOutcome::response_text),
            Some("final summary")
        );
    }

    #[tokio::test]
    async fn invalid_tool_argument_error_retries_without_tools() {
        let harness = run_reasoning(
            vec![
                Err(LlmError::InvalidResponse(
                    "invalid tool call arguments".into(),
                )),
                text_response("fallback success"),
            ],
            vec![tool("clock")],
            3,
            true,
        )
        .await;

        let calls = harness.llm.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].with_tools);
        assert!(!calls[1].with_tools);
        assert_eq!(
            harness
                .result
                .state
                .outcome()
                .and_then(AgentOutcome::response_text),
            Some("fallback success")
        );
    }

    #[tokio::test]
    async fn no_tool_fallback_response_never_executes_returned_tool_calls() {
        let clock = tool("clock");
        let fallback_with_tool_call = Ok(ChatCompletionResponse {
            content: "fallback success".into(),
            tool_calls: vec![ToolCall {
                id: "call-ignored".into(),
                name: "clock".into(),
                arguments: serde_json::json!({}),
            }],
            finish_reason: "tool_calls".into(),
            usage: None,
        });
        let harness = run_reasoning(
            vec![
                Err(LlmError::InvalidResponse(
                    "invalid tool call arguments".into(),
                )),
                fallback_with_tool_call,
                text_response("unexpected extra call"),
            ],
            vec![clock.clone()],
            1,
            true,
        )
        .await;

        assert_eq!(
            harness
                .result
                .state
                .outcome()
                .and_then(AgentOutcome::response_text),
            Some("fallback success")
        );
        assert!(harness.result.state.observations().is_empty());
        assert!(clock.arguments.lock().unwrap().is_empty());
        assert_eq!(harness.llm.calls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn llm_failure_becomes_the_stable_friendly_reply() {
        let harness = run_reasoning(
            vec![Err(LlmError::Connection("offline".into()))],
            vec![tool("clock")],
            3,
            true,
        )
        .await;

        assert!(
            harness
                .result
                .state
                .outcome()
                .and_then(AgentOutcome::response_text)
                .unwrap()
                .contains("抱歉")
        );
    }
}
