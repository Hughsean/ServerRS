use crate::app::agent::agent_runtime::AgentRuntimeSettings;
use crate::app::agent::chat_effect::{ChatEffectExecutor, TurnWriterT};
use crate::app::agent::chat_state::ChatTurnState;
use crate::app::agent::graph::{
    GraphBuildError, GraphCompileError, GraphDefinition, GraphId, GraphPolicy, GraphRunError,
    GraphRunResult, GraphRuntime, GraphVersion, NodeId, RunBudget, TransitionRule,
};
use crate::app::agent::memory_extraction::MemoryExtractionSchedulerT;
use crate::app::agent::nodes::{
    BuildContextNode, BuildPromptNode, ChatContextOptions, ChatContextProviderT,
    NormalizeReplyNode, PersistTurnNode, PrepareTurnNode, ReasoningSettings,
    ScheduleMemoryExtractionNode,
};
use crate::app::agent::subgraphs::{ReasoningLoopDeps, build_reasoning_loop};
use crate::app::agent::tool::AgentTool;
use crate::domain::agent::{AgentEventRepoT, AgentState, ToolDefinition};
use crate::domain::llm::LlmProvider;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

pub struct ChatAgentGraphDeps {
    pub llm: Arc<dyn LlmProvider>,
    pub event_repo: Arc<dyn AgentEventRepoT>,
    pub context_provider: Arc<dyn ChatContextProviderT>,
    pub turn_writer: Arc<dyn TurnWriterT>,
    pub memory_extraction_scheduler: Arc<dyn MemoryExtractionSchedulerT>,
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub settings: AgentRuntimeSettings,
}

pub struct ChatAgentGraph {
    runtime: GraphRuntime<ChatTurnState>,
    budget: RunBudget,
}

impl ChatAgentGraph {
    pub fn new(dependencies: ChatAgentGraphDeps) -> Result<Self, ChatGraphBuildError> {
        let effect_executor = Arc::new(ChatEffectExecutor::new(
            Arc::clone(&dependencies.turn_writer),
            Arc::clone(&dependencies.memory_extraction_scheduler),
        ));
        let agent_on = dependencies.settings.agent_enabled;
        let tools_available =
            agent_on && !dependencies.tools.is_empty() && dependencies.settings.max_tool_depth > 0;
        let context_tools = dependencies
            .tools
            .iter()
            .map(|tool| ToolDefinition {
                name: tool.name().to_owned(),
                description: tool.description().to_owned(),
                parameters: tool.parameters(),
            })
            .collect();
        let context_options = ChatContextOptions {
            tools: context_tools,
            max_memory_items: dependencies.settings.max_memory_items,
            max_rag_chunks: dependencies.settings.max_rag_chunks,
            summary_enabled: agent_on && dependencies.settings.summary_enabled,
            memory_enabled: agent_on && dependencies.settings.memory_enabled,
            rag_enabled: agent_on && dependencies.settings.rag_enabled,
        };
        let reasoning_settings = ReasoningSettings {
            agent_enabled: dependencies.settings.agent_enabled,
            max_tool_depth: dependencies.settings.max_tool_depth,
            temperature: dependencies.settings.temperature,
            top_p: dependencies.settings.top_p,
            reasoning: dependencies.settings.reasoning_config(),
        };

        let mut definition = GraphDefinition::new_versioned(
            GraphId::try_from("http_chat_agent").expect("static GraphId"),
            GraphVersion::try_from(2).expect("static graph version"),
        );
        definition.add_node(Arc::new(PrepareTurnNode::new(
            node("prepare_turn"),
            dependencies.settings.max_context_messages,
        )))?;
        definition.add_node(Arc::new(BuildContextNode::new(
            node("build_context"),
            dependencies.context_provider,
            context_options,
        )))?;
        definition.add_node(Arc::new(BuildPromptNode::new(
            node("build_prompt"),
            tools_available,
        )))?;
        definition.add_node(Arc::new(NormalizeReplyNode::new(node("normalize_reply"))))?;
        definition.add_node(Arc::new(PersistTurnNode::new(node("persist_turn"))))?;
        definition.add_node(Arc::new(ScheduleMemoryExtractionNode::new(
            node("schedule_memory_extraction"),
            agent_on
                && dependencies.settings.memory_enabled
                && dependencies.settings.memory_extraction_async,
        )))?;

        let reasoning = build_reasoning_loop(ReasoningLoopDeps {
            llm: dependencies.llm,
            event_repo: dependencies.event_repo,
            tools: dependencies.tools,
            settings: reasoning_settings,
        })?;
        let mounted = definition.mount("reasoning", reasoning)?;
        definition.connect_exit(
            mounted
                .exit("final_response")
                .expect("reasoning fragment declares final_response"),
            node("normalize_reply"),
        )?;

        definition.set_entry(node("prepare_turn"));
        definition.set_transition(
            node("prepare_turn"),
            TransitionRule::Goto(node("build_context")),
        )?;
        definition.set_transition(
            node("build_context"),
            TransitionRule::Goto(node("build_prompt")),
        )?;
        definition.set_transition(
            node("build_prompt"),
            TransitionRule::Goto(mounted.entry().clone()),
        )?;
        definition.set_transition(
            node("normalize_reply"),
            TransitionRule::Goto(node("persist_turn")),
        )?;
        definition.set_transition(
            node("persist_turn"),
            TransitionRule::Goto(node("schedule_memory_extraction")),
        )?;
        definition.set_transition(node("schedule_memory_extraction"), TransitionRule::End)?;

        let limits = chat_graph_limits(dependencies.settings.max_tool_depth);
        let compiled = definition.compile(GraphPolicy::new(limits.max_steps))?;
        let budget = RunBudget::new(limits.max_steps, Duration::from_secs(600))
            .with_llm_calls(limits.max_llm_calls)
            .with_tool_calls(u32::MAX)
            .with_tokens(u64::MAX);
        Ok(Self {
            runtime: GraphRuntime::with_effect_executor(compiled, effect_executor),
            budget,
        })
    }

    pub async fn run(
        &self,
        state: AgentState<ChatTurnState>,
    ) -> Result<GraphRunResult<ChatTurnState>, GraphRunError> {
        self.runtime.run(state, self.budget).await
    }

    pub fn node_ids(&self) -> impl Iterator<Item = &NodeId> {
        self.runtime.graph().node_ids()
    }

    #[cfg(test)]
    fn graph_version(&self) -> GraphVersion {
        self.runtime.graph().version()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ChatGraphBuildError {
    #[error("构建 Chat Agent 图失败: {0}")]
    Build(#[from] GraphBuildError),
    #[error("编译 Chat Agent 图失败: {0}")]
    Compile(#[from] GraphCompileError),
}

struct ChatGraphLimits {
    max_steps: NonZeroU32,
    max_llm_calls: u32,
}

fn chat_graph_limits(max_tool_depth: usize) -> ChatGraphLimits {
    let depth = u64::try_from(max_tool_depth).unwrap_or(u64::MAX);
    let max_steps = depth
        .saturating_mul(2)
        .saturating_add(12)
        .min(u64::from(u32::MAX)) as u32;
    let max_llm_calls = depth.saturating_add(4).min(u64::from(u32::MAX)) as u32;
    ChatGraphLimits {
        max_steps: NonZeroU32::new(max_steps).expect("Chat graph step limit is always nonzero"),
        max_llm_calls,
    }
}

fn node(value: &str) -> NodeId {
    NodeId::try_from(value).expect("Chat graph node IDs are static and valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agent::agent_runtime::AgentRuntimeSettings;
    use crate::app::agent::chat_effect::TurnWriterT;
    use crate::app::agent::chat_state::{ChatTurnState, PersistedTurn};
    use crate::app::agent::memory_extraction::{
        MemoryExtractionDispatch, MemoryExtractionRequest, MemoryExtractionSchedulerT,
    };
    use crate::app::agent::nodes::{ChatContextProviderT, ChatContextRequest, LoadedChatContext};
    use crate::domain::agent::{
        AgentContext, AgentEvent, AgentEventRepoT, AgentOutcome, AgentState, NewAgentEvent,
    };
    use crate::domain::conversation::conversation_message::NewConversationMessage;
    use crate::domain::llm::{
        ChatCompletionRequest, ChatCompletionResponse, LlmError, LlmProvider, ToolDefinition,
    };
    use crate::shared::error::AppError;
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::{Arc, Mutex};

    struct FakeContextProvider;

    #[async_trait]
    impl ChatContextProviderT for FakeContextProvider {
        async fn load(&self, request: ChatContextRequest) -> Result<LoadedChatContext, AppError> {
            Ok(LoadedChatContext {
                context: AgentContext {
                    user_id: request.user_id,
                    conversation_id: Some(request.conversation_id),
                    recent_messages: request.recent_messages,
                    summary: None,
                    memories: vec![],
                    rag_chunks: vec![],
                    fresh_chunks: vec![],
                    user_profile: None,
                    tools: request.tools,
                    location: request.location,
                },
                context_version: 23,
            })
        }
    }

    #[derive(Default)]
    struct FakeTurnWriter {
        calls: Mutex<usize>,
    }

    #[async_trait]
    impl TurnWriterT for FakeTurnWriter {
        async fn save_turn_atomic(
            &self,
            _conversation_id: u64,
            _user_id: u64,
            _user: NewConversationMessage,
            _assistant: NewConversationMessage,
        ) -> Result<PersistedTurn, AppError> {
            *self.calls.lock().unwrap() += 1;
            Ok(PersistedTurn::new(101, 102))
        }
    }

    #[derive(Default)]
    struct RecordingMemoryScheduler {
        requests: Mutex<Vec<MemoryExtractionRequest>>,
    }

    impl MemoryExtractionSchedulerT for RecordingMemoryScheduler {
        fn schedule(&self, request: MemoryExtractionRequest) -> MemoryExtractionDispatch {
            self.requests.lock().unwrap().push(request);
            MemoryExtractionDispatch::Scheduled
        }
    }

    struct TextLlm;

    #[async_trait]
    impl LlmProvider for TextLlm {
        async fn chat(
            &self,
            _request: ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, LlmError> {
            Ok(ChatCompletionResponse {
                content: "graph reply".into(),
                tool_calls: vec![],
                finish_reason: "stop".into(),
                usage: None,
            })
        }

        async fn chat_with_tools(
            &self,
            request: ChatCompletionRequest,
            _tools: Vec<ToolDefinition>,
        ) -> Result<ChatCompletionResponse, LlmError> {
            self.chat(request).await
        }
    }

    struct FakeEventRepo;

    #[async_trait]
    impl AgentEventRepoT for FakeEventRepo {
        async fn log_event(&self, event: NewAgentEvent) -> AgentEvent {
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

    fn graph(
        memory_extraction_async: bool,
    ) -> (
        ChatAgentGraph,
        Arc<FakeTurnWriter>,
        Arc<RecordingMemoryScheduler>,
    ) {
        let writer = Arc::new(FakeTurnWriter::default());
        let memory_scheduler = Arc::new(RecordingMemoryScheduler::default());
        let settings = AgentRuntimeSettings {
            agent_enabled: true,
            memory_enabled: true,
            rag_enabled: false,
            summary_enabled: false,
            max_context_messages: 10,
            max_memory_items: 0,
            max_rag_chunks: 0,
            memory_extraction_async,
            max_tool_depth: 2,
            temperature: 0.0,
            top_p: 1.0,
            enable_reasoning: false,
        };
        let graph = ChatAgentGraph::new(ChatAgentGraphDeps {
            llm: Arc::new(TextLlm),
            event_repo: Arc::new(FakeEventRepo),
            context_provider: Arc::new(FakeContextProvider),
            turn_writer: writer.clone(),
            memory_extraction_scheduler: memory_scheduler.clone(),
            tools: vec![],
            settings,
        })
        .unwrap();
        (graph, writer, memory_scheduler)
    }

    #[test]
    fn chat_graph_compiles_with_expected_namespaced_nodes() {
        let (graph, _, _) = graph(false);
        let node_ids: Vec<_> = graph.node_ids().map(ToString::to_string).collect();

        assert_eq!(
            node_ids,
            vec![
                "build_context",
                "build_prompt",
                "normalize_reply",
                "persist_turn",
                "prepare_turn",
                "reasoning.complete",
                "reasoning.final_without_tools",
                "reasoning.llm",
                "reasoning.tools",
                "schedule_memory_extraction",
            ]
        );
        assert_eq!(graph.graph_version().get(), 2);
    }

    #[tokio::test]
    async fn chat_graph_runs_the_complete_turn_and_persists_it() {
        let (graph, writer, memory_scheduler) = graph(false);
        let state = AgentState::new(ChatTurnState::new(7, 9, "hello".into(), None, None, vec![]));

        let result = graph.run(state).await.unwrap();

        assert_eq!(
            result.state.outcome().and_then(AgentOutcome::response_text),
            Some("graph reply")
        );
        assert_eq!(result.state.business().context_version(), Some(23));
        assert_eq!(
            result
                .state
                .business()
                .persisted_turn()
                .unwrap()
                .assistant_message_id(),
            102
        );
        assert_eq!(*writer.calls.lock().unwrap(), 1);
        assert!(memory_scheduler.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn chat_graph_schedules_memory_extraction_after_turn_persistence() {
        let (graph, writer, memory_scheduler) = graph(true);
        let state = AgentState::new(ChatTurnState::new(7, 9, "hello".into(), None, None, vec![]));

        let result = graph.run(state).await.unwrap();

        assert_eq!(*writer.calls.lock().unwrap(), 1);
        let requests = memory_scheduler.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].user_id, 7);
        assert_eq!(requests[0].conversation_id, 9);
        assert_eq!(requests[0].source_message_id, 101);
        assert_eq!(requests[0].user_message, "hello");
        assert_eq!(requests[0].assistant_reply, "graph reply");
        assert_eq!(requests[0].context_version, 23);
        assert_eq!(
            result.visited.last(),
            Some(&node("schedule_memory_extraction"))
        );
    }
}
