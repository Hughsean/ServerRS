use crate::app::agent::agent_context::AgentContextBuilder;
use crate::app::agent::chat_state::{ChatTurnState, ChatTurnUpdate};
use crate::app::agent::error_adapter::node_error_from_application;
use crate::app::agent::graph::{
    AgentNode, NodeError, NodeErrorKind, NodeId, NodeResult, RunContext, UsageDelta,
};
use crate::app::agent::message_adapter::agent_message_from_chat;
use crate::app::agent::prompt_builder::PromptBuilder;
use crate::domain::agent::{
    AgentBusinessState, AgentContext, AgentMessage, AgentState, AgentUpdate, ToolDefinition,
};
use crate::domain::llm::ChatMessage;
use crate::domain::user::user_context_version::UserContextVersionRepoT;
use crate::domain::user::user_profile_repo::UserProfileRepoT;
use crate::shared::error::AppError;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ChatContextRequest {
    pub user_id: u64,
    pub conversation_id: u64,
    pub recent_messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDefinition>,
    pub location: Option<Value>,
    pub max_memory_items: u32,
    pub max_rag_chunks: u64,
    pub summary_enabled: bool,
    pub memory_enabled: bool,
    pub rag_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct LoadedChatContext {
    pub context: AgentContext,
    pub context_version: u64,
}

#[async_trait]
pub trait ChatContextProviderT: Send + Sync {
    async fn load(&self, request: ChatContextRequest) -> Result<LoadedChatContext, AppError>;
}

/// 将现有仓储和 `AgentContextBuilder` 适配为节点所需的窄端口。
pub struct DefaultChatContextProvider {
    context_version_repo: Arc<dyn UserContextVersionRepoT>,
    user_profile_repo: Arc<dyn UserProfileRepoT>,
    context_builder: Arc<AgentContextBuilder>,
}

impl DefaultChatContextProvider {
    pub fn new(
        context_version_repo: Arc<dyn UserContextVersionRepoT>,
        user_profile_repo: Arc<dyn UserProfileRepoT>,
        context_builder: Arc<AgentContextBuilder>,
    ) -> Self {
        Self {
            context_version_repo,
            user_profile_repo,
            context_builder,
        }
    }
}

#[async_trait]
impl ChatContextProviderT for DefaultChatContextProvider {
    async fn load(&self, request: ChatContextRequest) -> Result<LoadedChatContext, AppError> {
        let context_version = self
            .context_version_repo
            .get_or_create(request.user_id)
            .await?
            .version;
        let profile = self
            .user_profile_repo
            .find_by_user_id(request.user_id)
            .await
            .ok()
            .flatten();
        let context = self
            .context_builder
            .build(
                request.user_id,
                Some(request.conversation_id),
                request.recent_messages,
                profile,
                request.tools,
                request.location,
                request.max_memory_items,
                request.max_rag_chunks,
                request.summary_enabled,
                request.memory_enabled,
                request.rag_enabled,
            )
            .await;

        Ok(LoadedChatContext {
            context,
            context_version,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ChatContextOptions {
    pub tools: Vec<ToolDefinition>,
    pub max_memory_items: u32,
    pub max_rag_chunks: u64,
    pub summary_enabled: bool,
    pub memory_enabled: bool,
    pub rag_enabled: bool,
}

pub struct BuildContextNode {
    id: NodeId,
    provider: Arc<dyn ChatContextProviderT>,
    options: ChatContextOptions,
}

impl BuildContextNode {
    pub fn new(
        id: NodeId,
        provider: Arc<dyn ChatContextProviderT>,
        options: ChatContextOptions,
    ) -> Self {
        Self {
            id,
            provider,
            options,
        }
    }
}

#[async_trait]
impl AgentNode<ChatTurnState> for BuildContextNode {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        state: &AgentState<ChatTurnState>,
        _context: &RunContext,
    ) -> Result<NodeResult<ChatTurnUpdate, <ChatTurnState as AgentBusinessState>::Effect>, NodeError>
    {
        let turn = state.business();
        if !turn.messages_prepared() {
            return Err(NodeError::new(
                NodeErrorKind::Invariant,
                "构建 AgentContext 前消息尚未预处理",
            ));
        }
        let request = ChatContextRequest {
            user_id: turn.user_id(),
            conversation_id: turn.conversation_id(),
            recent_messages: turn.recent_messages().to_vec(),
            tools: self.options.tools.clone(),
            location: turn.location().cloned(),
            max_memory_items: self.options.max_memory_items,
            max_rag_chunks: self.options.max_rag_chunks,
            summary_enabled: self.options.summary_enabled,
            memory_enabled: self.options.memory_enabled,
            rag_enabled: self.options.rag_enabled,
        };
        let loaded = self
            .provider
            .load(request)
            .await
            .map_err(node_error_from_application)?;

        Ok(NodeResult::new(
            vec![AgentUpdate::Business(ChatTurnUpdate::SetContext {
                context: loaded.context,
                context_version: loaded.context_version,
            })],
            UsageDelta::default(),
        ))
    }
}

pub struct BuildPromptNode {
    id: NodeId,
    prompt_builder: PromptBuilder,
    tools_available: bool,
}

impl BuildPromptNode {
    pub fn new(id: NodeId, tools_available: bool) -> Self {
        Self {
            id,
            prompt_builder: PromptBuilder::new(),
            tools_available,
        }
    }
}

#[async_trait]
impl AgentNode<ChatTurnState> for BuildPromptNode {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        state: &AgentState<ChatTurnState>,
        _context: &RunContext,
    ) -> Result<NodeResult<ChatTurnUpdate, <ChatTurnState as AgentBusinessState>::Effect>, NodeError>
    {
        if !state.messages().is_empty() {
            return Err(NodeError::new(
                NodeErrorKind::Invariant,
                "BuildPromptNode 只能初始化一次标准消息",
            ));
        }
        let turn = state.business();
        let context = turn.context().ok_or_else(|| {
            NodeError::new(
                NodeErrorKind::Invariant,
                "构建 Prompt 前 AgentContext 尚未设置",
            )
        })?;
        let system_message = self
            .prompt_builder
            .build_system_message(context, self.tools_available);

        let mut messages = Vec::with_capacity(turn.recent_messages().len() + 1);
        messages.push(AgentMessage::system(system_message));
        for message in turn
            .recent_messages()
            .iter()
            .filter(|message| message.role != "system")
        {
            messages.push(agent_message_from_chat(message.clone()).map_err(|error| {
                NodeError::new(
                    NodeErrorKind::Invariant,
                    format!("历史消息无法转换为 AgentMessage: {error}"),
                )
            })?);
        }

        Ok(NodeResult::new(
            vec![AgentUpdate::AppendMessages(messages)],
            UsageDelta::default(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agent::chat_state::{ChatTurnState, ChatTurnUpdate};
    use crate::app::agent::graph::{AgentNode, NodeId, RunBudget, RunContext, RunTrace};
    use crate::domain::agent::{
        AgentBusinessState, AgentContext, AgentMessage, AgentState, ToolDefinition,
    };
    use crate::domain::llm::ChatMessage;
    use async_trait::async_trait;
    use std::num::NonZeroU32;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    struct FakeContextProvider {
        requests: Mutex<Vec<ChatContextRequest>>,
    }

    struct FailingContextProvider;

    #[async_trait]
    impl ChatContextProviderT for FakeContextProvider {
        async fn load(
            &self,
            request: ChatContextRequest,
        ) -> Result<LoadedChatContext, crate::shared::error::AppError> {
            let context = AgentContext {
                user_id: request.user_id,
                conversation_id: Some(request.conversation_id),
                recent_messages: request.recent_messages.clone(),
                summary: Some("summary".into()),
                memories: vec![],
                rag_chunks: vec![],
                fresh_chunks: vec![],
                user_profile: None,
                tools: request.tools.clone(),
                location: request.location.clone(),
            };
            self.requests.lock().unwrap().push(request);
            Ok(LoadedChatContext {
                context,
                context_version: 17,
            })
        }
    }

    #[async_trait]
    impl ChatContextProviderT for FailingContextProvider {
        async fn load(
            &self,
            _request: ChatContextRequest,
        ) -> Result<LoadedChatContext, crate::shared::error::AppError> {
            Err(crate::shared::error::AppError::Infrastructure(
                "context offline".into(),
            ))
        }
    }

    fn node_id(value: &str) -> NodeId {
        NodeId::try_from(value).unwrap()
    }

    fn run_context() -> RunContext {
        RunContext::new(
            RunBudget::new(NonZeroU32::new(8).unwrap(), Duration::from_secs(30)),
            CancellationToken::new(),
            RunTrace::default(),
        )
    }

    fn chat_message(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    fn state_with_prepared_history() -> AgentState<ChatTurnState> {
        let mut business = ChatTurnState::new(
            7,
            9,
            "hello".into(),
            None,
            Some(serde_json::json!({"city": "合肥"})),
            vec![],
        );
        business
            .apply_update(ChatTurnUpdate::SetRecentMessages(vec![chat_message(
                "user", "hello",
            )]))
            .unwrap();
        AgentState::new(business)
    }

    #[tokio::test]
    async fn chat_context_node_sets_context_and_version_from_narrow_provider() {
        let provider = Arc::new(FakeContextProvider {
            requests: Mutex::new(Vec::new()),
        });
        let options = ChatContextOptions {
            tools: vec![ToolDefinition {
                name: "clock".into(),
                description: "time".into(),
                parameters: serde_json::json!({}),
            }],
            max_memory_items: 4,
            max_rag_chunks: 3,
            summary_enabled: true,
            memory_enabled: false,
            rag_enabled: true,
        };
        let node = BuildContextNode::new(node_id("context"), provider.clone(), options);
        let mut state = state_with_prepared_history();

        let result = node.execute(&state, &run_context()).await.unwrap();
        state.apply_updates(result.into_updates()).unwrap();

        assert_eq!(state.business().context_version(), Some(17));
        assert_eq!(
            state.business().context().unwrap().summary.as_deref(),
            Some("summary")
        );
        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests[0].max_memory_items, 4);
        assert!(!requests[0].memory_enabled);
        assert_eq!(requests[0].recent_messages[0].content, "hello");
    }

    #[tokio::test]
    async fn chat_context_node_preserves_application_error_variant() {
        let node = BuildContextNode::new(
            node_id("context"),
            Arc::new(FailingContextProvider),
            ChatContextOptions {
                tools: vec![],
                max_memory_items: 4,
                max_rag_chunks: 3,
                summary_enabled: true,
                memory_enabled: true,
                rag_enabled: true,
            },
        );

        let error = node
            .execute(&state_with_prepared_history(), &run_context())
            .await
            .unwrap_err();

        assert!(matches!(
            error.source_ref::<crate::shared::error::AppError>(),
            Some(crate::shared::error::AppError::Infrastructure(message))
                if message == "context offline"
        ));
    }

    #[tokio::test]
    async fn chat_prompt_node_prepends_exactly_one_system_message() {
        let mut state = state_with_prepared_history();
        let context = AgentContext {
            user_id: 7,
            conversation_id: Some(9),
            recent_messages: vec![
                chat_message("system", "stale system"),
                chat_message("user", "hello"),
            ],
            summary: Some("remember this".into()),
            memories: vec![],
            rag_chunks: vec![],
            fresh_chunks: vec![],
            user_profile: None,
            tools: vec![],
            location: None,
        };
        state
            .apply_updates(vec![crate::domain::agent::AgentUpdate::Business(
                ChatTurnUpdate::SetContext {
                    context,
                    context_version: 2,
                },
            )])
            .unwrap();
        let node = BuildPromptNode::new(node_id("prompt"), false);

        let result = node.execute(&state, &run_context()).await.unwrap();
        state.apply_updates(result.into_updates()).unwrap();

        assert!(matches!(state.messages()[0], AgentMessage::System { .. }));
        assert!(state.messages()[0].content().contains("remember this"));
        assert_eq!(
            state
                .messages()
                .iter()
                .filter(|message| matches!(message, AgentMessage::System { .. }))
                .count(),
            1
        );
        assert_eq!(state.messages().len(), 2);
        assert_eq!(state.messages()[1].content(), "hello");
    }
}
