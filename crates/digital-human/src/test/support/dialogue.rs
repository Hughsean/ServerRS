use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::app::agent::agent_context::AgentContextBuilder;
use crate::app::agent::agent_runtime::{AgentRuntime, AgentRuntimeSettings, AgentTool};
use crate::app::agent::chat_effect::ConversationTurnWriter;
use crate::app::agent::chat_graph::{ChatAgentGraph, ChatAgentGraphDeps};
use crate::app::agent::graph::InMemoryCheckpointStore;
use crate::app::agent::memory_extraction::AsyncMemoryExtractionScheduler;
use crate::app::agent::nodes::DefaultChatContextProvider;
use crate::app::agent::tools::get_time_tool::GetTimeTool;
use crate::app::agent::tools::knowledge_search_tool::KnowledgeSearchTool;
use crate::app::agent::tools::memory_search_tool::MemorySearchTool;
use crate::app::context_routing::ContextRoutingService;
use crate::app::fresh_context::config::FreshContextUseCaseConfig;
use crate::app::fresh_context::retrieval::FreshRetrievalService;
use crate::app::memory::memory_extractor::MemoryExtractor;
use crate::app::memory::memory_service::MemoryService;
use crate::app::rag::retrieval_service::RetrievalService;
use crate::app::session::chat_service::ChatService;
use crate::app::summary::summary_service::SummaryService;
use crate::domain::fresh_context::FreshContextRepoT;
use crate::domain::llm::{EmbeddingProvider, LlmProvider};
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::domain::vector_store::VectorStoreT;
use crate::repositories::RepositorySet;
use crate::shared::config::AppConfig;
use crate::test::support::tasks::RecordingTaskPublisher;

pub struct DialogueHarness {
    pub chat_service: ChatService,
    pub task_publisher: Arc<RecordingTaskPublisher>,
}

pub async fn context_builder(
    config: &AppConfig,
    db: &DatabaseConnection,
    repos: &RepositorySet,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    llm_provider: Arc<dyn LlmProvider>,
    vector_store: Arc<dyn VectorStoreT>,
    context_routing_service: Arc<ContextRoutingService>,
) -> AgentContextBuilder {
    AgentContextBuilder::new(
        memory_service(
            config,
            repos,
            Arc::clone(&embedding_provider),
            llm_provider,
            Arc::clone(&vector_store),
        ),
        retrieval_service(
            config,
            repos,
            Arc::clone(&embedding_provider),
            Arc::clone(&vector_store),
        ),
        Arc::new(SummaryService::new(Arc::clone(&repos.summary_repo), None)),
        fresh_retrieval_service(config, db, Arc::clone(&embedding_provider), vector_store),
        Arc::clone(&repos.conv_repo),
        Arc::clone(&repos.profile_repo),
    )
    .with_context_routing_service(Some(context_routing_service))
}

pub async fn chat_service_with_time_tool(
    config: &AppConfig,
    db: &DatabaseConnection,
    repos: &RepositorySet,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    llm_provider: Arc<dyn LlmProvider>,
    vector_store: Arc<dyn VectorStoreT>,
    context_routing_service: Arc<ContextRoutingService>,
) -> DialogueHarness {
    let memory_service = memory_service(
        config,
        repos,
        Arc::clone(&embedding_provider),
        Arc::clone(&llm_provider),
        Arc::clone(&vector_store),
    );
    let context_builder = Arc::new(
        context_builder(
            config,
            db,
            repos,
            embedding_provider,
            Arc::clone(&llm_provider),
            vector_store,
            context_routing_service,
        )
        .await,
    );

    let tools: Vec<Arc<dyn AgentTool>> = vec![Arc::new(GetTimeTool::new())];
    let settings = AgentRuntimeSettings {
        agent_enabled: config.agent.enabled,
        memory_enabled: config.agent.memory_enabled,
        rag_enabled: config.agent.rag_enabled,
        summary_enabled: config.agent.summary_enabled,
        // 工具选择对历史噪声敏感；完整 response 测试只验证当前轮链路。
        max_context_messages: 1,
        max_memory_items: config.agent.max_memory_items,
        max_rag_chunks: u64::from(config.agent.max_rag_chunks),
        memory_extraction_async: false,
        max_tool_depth: config.llm.max_tool_depth as usize,
        temperature: 0.0,
        top_p: 1.0,
        enable_reasoning: false,
        approval_required_tools: Vec::new(),
    };

    let agent_runtime = agent_runtime(
        llm_provider,
        Arc::clone(&memory_service),
        repos,
        context_builder,
        tools,
        settings,
    );

    let task_publisher = Arc::new(RecordingTaskPublisher::default());
    let task_publisher_dyn: Arc<dyn TaskPublisher> = task_publisher.clone();
    let chat_service = ChatService::new(
        task_publisher_dyn,
        Arc::clone(&repos.conv_repo),
        Arc::new(agent_runtime),
        Arc::clone(&memory_service),
        Arc::clone(&repos.context_control_repo),
        None,
    );

    DialogueHarness {
        chat_service,
        task_publisher,
    }
}

pub async fn chat_service_with_core_test_tools(
    config: &AppConfig,
    db: &DatabaseConnection,
    repos: &RepositorySet,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    llm_provider: Arc<dyn LlmProvider>,
    vector_store: Arc<dyn VectorStoreT>,
    context_routing_service: Arc<ContextRoutingService>,
) -> DialogueHarness {
    let memory_service = memory_service(
        config,
        repos,
        Arc::clone(&embedding_provider),
        Arc::clone(&llm_provider),
        Arc::clone(&vector_store),
    );
    let retrieval_service = retrieval_service(
        config,
        repos,
        Arc::clone(&embedding_provider),
        Arc::clone(&vector_store),
    );
    let context_builder = Arc::new(
        AgentContextBuilder::new(
            Arc::clone(&memory_service),
            Arc::clone(&retrieval_service),
            Arc::new(SummaryService::new(Arc::clone(&repos.summary_repo), None)),
            fresh_retrieval_service(config, db, embedding_provider, vector_store),
            Arc::clone(&repos.conv_repo),
            Arc::clone(&repos.profile_repo),
        )
        .with_context_routing_service(Some(context_routing_service)),
    );

    let tools: Vec<Arc<dyn AgentTool>> = vec![
        Arc::new(KnowledgeSearchTool::new(Arc::clone(&retrieval_service))),
        Arc::new(MemorySearchTool::new(Arc::clone(&memory_service))),
        Arc::new(GetTimeTool::new()),
    ];
    let settings = AgentRuntimeSettings {
        agent_enabled: config.agent.enabled,
        memory_enabled: config.agent.memory_enabled,
        rag_enabled: config.agent.rag_enabled,
        summary_enabled: config.agent.summary_enabled,
        max_context_messages: 6,
        max_memory_items: config.agent.max_memory_items,
        max_rag_chunks: u64::from(config.agent.max_rag_chunks),
        memory_extraction_async: false,
        max_tool_depth: config.llm.max_tool_depth as usize,
        temperature: 0.0,
        top_p: 1.0,
        enable_reasoning: false,
        approval_required_tools: Vec::new(),
    };

    let agent_runtime = agent_runtime(
        llm_provider,
        Arc::clone(&memory_service),
        repos,
        context_builder,
        tools,
        settings,
    );

    let task_publisher = Arc::new(RecordingTaskPublisher::default());
    let task_publisher_dyn: Arc<dyn TaskPublisher> = task_publisher.clone();
    let chat_service = ChatService::new(
        task_publisher_dyn,
        Arc::clone(&repos.conv_repo),
        Arc::new(agent_runtime),
        Arc::clone(&memory_service),
        Arc::clone(&repos.context_control_repo),
        None,
    );

    DialogueHarness {
        chat_service,
        task_publisher,
    }
}

fn agent_runtime(
    llm: Arc<dyn LlmProvider>,
    memory: Arc<MemoryService>,
    repos: &RepositorySet,
    context_builder: Arc<AgentContextBuilder>,
    tools: Vec<Arc<dyn AgentTool>>,
    settings: AgentRuntimeSettings,
) -> AgentRuntime {
    let max_context_messages = settings.max_context_messages;
    let context_provider = Arc::new(DefaultChatContextProvider::new(
        Arc::clone(&repos.context_version_repo),
        Arc::clone(&repos.profile_repo),
        context_builder,
    ));
    let turn_writer = Arc::new(ConversationTurnWriter::new(Arc::clone(&repos.conv_repo)));
    let memory_extraction_scheduler = Arc::new(AsyncMemoryExtractionScheduler::new(memory));
    let graph = ChatAgentGraph::new(ChatAgentGraphDeps {
        llm,
        event_repo: Arc::clone(&repos.agent_event_repo),
        context_provider,
        turn_writer,
        memory_extraction_scheduler,
        checkpoint_store: Arc::new(InMemoryCheckpointStore::new()),
        tools,
        settings,
    })
    .expect("test Chat Agent graph must compile");

    AgentRuntime::from_graph(graph, max_context_messages)
}

fn retrieval_service(
    config: &AppConfig,
    repos: &RepositorySet,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    vector_store: Arc<dyn VectorStoreT>,
) -> Arc<RetrievalService> {
    let mut service = RetrievalService::new(Arc::clone(&repos.rag_repo), Some(embedding_provider))
        .with_hybrid_weights(
            config.rag.hybrid_vector_weight,
            config.rag.hybrid_keyword_weight,
        )
        .with_vector_store(vector_store, config.vector_store.rag_index_name.clone());

    if config.web_ingestion.enabled {
        service = service.with_web_collection(config.web_ingestion.vector_index_name.clone());
    }
    Arc::new(service)
}

fn memory_service(
    config: &AppConfig,
    repos: &RepositorySet,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    llm_provider: Arc<dyn LlmProvider>,
    vector_store: Arc<dyn VectorStoreT>,
) -> Arc<MemoryService> {
    let memory_extractor = Arc::new(MemoryExtractor::new(llm_provider));
    Arc::new(
        MemoryService::new(Arc::clone(&repos.memory_repo), memory_extractor)
            .with_personalization_profile_repo(Arc::clone(&repos.profile_repo))
            .with_context_version_repo(Arc::clone(&repos.context_version_repo))
            .with_vector_search(
                vector_store,
                embedding_provider,
                config.vector_store.memory_index_name.clone(),
            ),
    )
}

fn fresh_retrieval_service(
    config: &AppConfig,
    db: &DatabaseConnection,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    vector_store: Arc<dyn VectorStoreT>,
) -> Option<Arc<FreshRetrievalService>> {
    if !config.fresh_context.enabled {
        return None;
    }

    let fresh_repo: Arc<dyn FreshContextRepoT> =
        Arc::new(crate::infra::repo::seaorm_impl::fresh_context::FreshContextRepo::new(db.clone()));
    Some(Arc::new(FreshRetrievalService::new(
        fresh_repo,
        vector_store,
        embedding_provider,
        FreshContextUseCaseConfig::from(&config.fresh_context),
        config.fresh_context.vector_index_name.clone(),
    )))
}
