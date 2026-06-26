use std::sync::Arc;

use crate::api::{
    AdminState, AppState, AuthState, ChatState, CommunityState, DepressionState, DiaryState,
    InternalState, MusicState, ObjectState, PsychologyState, SignatureState, UserState,
};
use crate::app::agent::agent_context::AgentContextBuilder;
use crate::app::agent::agent_runtime::{AgentRuntime, AgentRuntimeSettings};
use crate::app::agent::tool_registry::{AgentToolDeps, build_default_agent_tools};
use crate::app::auth::auth_service::AuthService;
use crate::app::community::community_service::CommunityService;
use crate::app::depression::depression_service::DepressionService;
use crate::app::diary::diary_service::DiaryService;
use crate::app::memory::memory_extractor::MemoryExtractor;
use crate::app::memory::memory_service::MemoryService;
use crate::app::music::music_service::MusicService;
use crate::app::psychology::psychology_service::PsychologyService;
use crate::app::rag::chunking::ChunkingService;
use crate::app::rag::ingestion_service::IngestionService;
use crate::app::rag::retrieval_service::RetrievalService;
use crate::app::risk::post_conversation_risk_audit_worker::PostConversationRiskAuditWorker;
use crate::app::risk::risk_detection_service::RiskDetectionService;
use crate::app::session::chat_service::ChatService;
use crate::app::session::session_service::SessionService;
use crate::app::storage::object_service::ObjectService;
use crate::app::summary::summary_refresh_handler::SummaryRefreshHandler;
use crate::app::summary::summary_service::SummaryService;
use crate::app::user::user_service::UserService;
use crate::app::web_ingestion::review_service::KnowledgeReviewService;
use crate::bootstrap::infra::InfraContext;
use crate::bootstrap::repos::RepoGraph;
use crate::bootstrap::tasks::TaskContext;
use crate::bootstrap::vector::VectorContext;
use crate::bootstrap::web_ingestion;
use crate::domain::auth::token_service::TokenServiceT;
use crate::domain::conversation::conversation_repository::ConversationRepoT;
use crate::domain::llm::LlmProvider;
use crate::domain::risk::risk_detector::RiskDetector;
use crate::domain::risk::risk_repository::RiskRepoT;
use crate::domain::storage::ObjectStorage;
use crate::domain::tasks::task_handler::TaskHandler;
use crate::infra::detector::rule_based_detector::RuleBasedRiskDetector;
use crate::infra::storage::local_storage::LocalObjectStorage;
use crate::shared::config::AppConfig;

#[derive(Clone)]
pub struct ServiceGraph {
    pub auth: Arc<AuthService>,
    pub user: Arc<UserService>,
    pub query: Arc<SessionService>,
    pub objects: Arc<ObjectService>,
    pub psychology: Arc<PsychologyService>,
    pub depression: Arc<DepressionService>,
    pub diaries: Arc<DiaryService>,
    pub music: Arc<MusicService>,
    pub community: Arc<CommunityService>,
    pub retrieval: Arc<RetrievalService>,
    pub ingestion: Arc<IngestionService>,
    pub memory: Arc<MemoryService>,
    pub agent_runtime: Arc<AgentRuntime>,
    pub knowledge_review: Arc<KnowledgeReviewService>,
    pub chat: Arc<ChatService>,
    pub chat_conv_repo: Arc<dyn ConversationRepoT>,
    pub token_service: Arc<dyn TokenServiceT>,
    pub risk_repo: Arc<dyn RiskRepoT>,
}

impl ServiceGraph {
    /// 构造所有业务服务，注册后台 handler。
    #[allow(unused_mut)]
    pub async fn build(
        config: &AppConfig,
        infra: &InfraContext,
        repos: &RepoGraph,
        vector: &VectorContext,
        tasks: &mut TaskContext,
    ) -> Result<Self, std::io::Error> {
        let user_repo = Arc::clone(&repos.user_repo);
        let profile_repo = Arc::clone(&repos.profile_repo);
        let context_version_repo = Arc::clone(&repos.context_version_repo);
        let context_control_repo = Arc::clone(&repos.context_control_repo);
        let conv_repo = Arc::clone(&repos.conv_repo);
        let risk_repo = Arc::clone(&repos.risk_repo);
        let psychology_repo = Arc::clone(&repos.psychology_repo);
        let depression_repo = Arc::clone(&repos.depression_repo);
        let diary_repo = Arc::clone(&repos.diary_repo);
        let music_repo = Arc::clone(&repos.music_repo);
        let community_repo = Arc::clone(&repos.community_repo);
        let agent_event_repo = Arc::clone(&repos.agent_event_repo);
        let stored_object_repo = Arc::clone(&repos.stored_object_repo);
        let rag_repo = Arc::clone(&repos.rag_repo);
        let memory_repo = Arc::clone(&repos.memory_repo);
        let summary_repo = Arc::clone(&repos.summary_repo);

        // ── 认证基础设施 ──
        let auth_graph = crate::bootstrap::auth::build_auth(
            &infra.db,
            &config.jwt,
            &config.auth,
            &user_repo,
            &tasks.task_publisher,
        );

        // ── 风险检测 ──
        let risk_detector: Arc<dyn RiskDetector> = Arc::new(RuleBasedRiskDetector::new());

        let risk_detection_service = Arc::new(RiskDetectionService::new(
            Arc::clone(&risk_repo),
            Arc::clone(&tasks.task_publisher),
            Arc::clone(&risk_detector),
        ));
        let risk_audit_worker: Arc<dyn TaskHandler> =
            Arc::new(PostConversationRiskAuditWorker::new(
                Arc::clone(&conv_repo),
                Arc::clone(&risk_detection_service),
            ));

        // ── 服务 ──
        let auth = Arc::clone(&auth_graph.auth_service);
        let user: Arc<UserService> = Arc::new(UserService::new(
            Arc::clone(&user_repo),
            Arc::clone(&profile_repo),
        ));
        let query: Arc<SessionService> = Arc::new(SessionService::new(
            Arc::clone(&conv_repo),
            Arc::clone(&risk_repo),
        ));

        // ── RAG 与记忆服务 ──
        let mut retrieval_svc = RetrievalService::new(
            Arc::clone(&rag_repo),
            Some(Arc::clone(&vector.embedding_provider)),
        )
        .with_hybrid_weights(
            config.rag.hybrid_vector_weight,
            config.rag.hybrid_keyword_weight,
        );
        if let Some(ref vs) = vector.vector_store {
            retrieval_svc = retrieval_svc
                .with_vector_store(Arc::clone(vs), config.qdrant.rag_collection.clone());
        }
        if config.web_ingestion.enabled {
            retrieval_svc =
                retrieval_svc.with_web_collection(config.web_ingestion.qdrant_collection.clone());
        }
        let retrieval: Arc<RetrievalService> = Arc::new(retrieval_svc);

        let chunking = ChunkingService::new();
        let mut ingestion_svc = IngestionService::new(
            Arc::clone(&rag_repo),
            chunking,
            Some(Arc::clone(&vector.embedding_provider)),
        )
        .with_chunking_config(config.rag.chunk_size, config.rag.chunk_overlap);
        if let Some(ref vi) = vector.vector_index {
            ingestion_svc = ingestion_svc.with_vector_index(Arc::clone(vi));
        }
        let ingestion: Arc<IngestionService> = Arc::new(ingestion_svc);

        let memory_extractor: Arc<MemoryExtractor> =
            Arc::new(MemoryExtractor::new(Arc::clone(&infra.ollama_provider)));
        let mut memory_svc = MemoryService::new(Arc::clone(&memory_repo), memory_extractor)
            .with_personalization_profile_repo(Arc::clone(&profile_repo))
            .with_context_version_repo(Arc::clone(&context_version_repo));
        if let Some(ref vs) = vector.vector_store {
            memory_svc = memory_svc.with_vector_search(
                Arc::clone(vs),
                Arc::clone(&vector.embedding_provider),
                config.qdrant.memory_collection.clone(),
            );
        }
        if let Some(ref vi) = vector.vector_index {
            memory_svc = memory_svc.with_vector_index(Arc::clone(vi));
        }
        let memory_svc: Arc<MemoryService> = Arc::new(memory_svc);

        // ── SummaryService ──
        let summary_service: Arc<SummaryService> = Arc::new(SummaryService::new(
            Arc::clone(&summary_repo),
            vector.vector_index.clone(),
        ));
        let summary_refresh_handler: Arc<dyn TaskHandler> = Arc::new(SummaryRefreshHandler::new(
            config.agent.enabled && config.agent.summary_enabled && config.agent.summary_async,
            Arc::clone(&infra.ollama_provider) as Arc<dyn LlmProvider>,
            Arc::clone(&conv_repo) as Arc<dyn ConversationRepoT>,
            Arc::clone(&summary_service),
            Arc::clone(&context_version_repo),
        ));

        // ── 注册后台任务 ──
        tasks.start_service_handlers(risk_audit_worker, summary_refresh_handler);

        // ── 代理运行时 ──
        let context_builder: Arc<AgentContextBuilder> = Arc::new(AgentContextBuilder::new(
            Arc::clone(&memory_svc),
            Arc::clone(&retrieval),
            Arc::clone(&summary_service),
            Arc::clone(&conv_repo),
            Arc::clone(&profile_repo),
        ));

        let tool_deps = AgentToolDeps {
            retrieval: Arc::clone(&retrieval),
            memory: Arc::clone(&memory_svc),
            diary_repo: Arc::clone(&diary_repo),
            depression_repo: Arc::clone(&depression_repo),
            music_repo: Arc::clone(&music_repo),
            community_repo: Arc::clone(&community_repo),
            plugins: config.plugins.clone(),
        };

        let agent_tools = build_default_agent_tools(&tool_deps, config.agent.enabled)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

        let agent_settings = AgentRuntimeSettings {
            agent_enabled: config.agent.enabled,
            memory_enabled: config.agent.memory_enabled,
            rag_enabled: config.agent.rag_enabled,
            summary_enabled: config.agent.summary_enabled,
            max_context_messages: config.agent.max_context_messages as usize,
            max_memory_items: config.agent.max_memory_items,
            max_rag_chunks: config.agent.max_rag_chunks as u64,
            memory_extraction_async: config.agent.memory_extraction_async,
            max_tool_depth: config.llm.max_tool_depth as usize,
            temperature: config.llm.temperature,
            top_p: config.llm.top_p,
            enable_reasoning: config.llm.enable_reasoning,
        };

        let agent_runtime: Arc<AgentRuntime> = Arc::new(AgentRuntime::new(
            Arc::clone(&infra.ollama_provider),
            Arc::clone(&memory_svc),
            Arc::clone(&agent_event_repo),
            Arc::clone(&conv_repo),
            Arc::clone(&profile_repo),
            Arc::clone(&context_version_repo),
            context_builder,
            agent_tools,
            agent_settings,
        ));

        // ── ChatService ──
        let chat_service: Arc<ChatService> = Arc::new(ChatService::new(
            Arc::clone(&tasks.task_publisher),
            Arc::clone(&conv_repo) as Arc<dyn ConversationRepoT>,
            Arc::clone(&agent_runtime),
            Arc::clone(&memory_svc),
            Arc::clone(&context_control_repo),
            vector.vector_index.clone(),
        ));

        // ── 领域服务 ──
        let psychology: Arc<PsychologyService> =
            Arc::new(PsychologyService::new(Arc::clone(&psychology_repo)));
        let depression: Arc<DepressionService> =
            Arc::new(DepressionService::new(Arc::clone(&depression_repo)));
        let diaries: Arc<DiaryService> = Arc::new(DiaryService::new(
            Arc::clone(&diary_repo),
            Some(Arc::clone(&infra.ollama_client)),
        ));
        let local_storage: Arc<dyn ObjectStorage> = Arc::new(LocalObjectStorage::new(
            std::path::PathBuf::from(&config.storage.base_path),
        ));
        let music: Arc<MusicService> = Arc::new(MusicService::new(Arc::clone(&music_repo)));
        let community: Arc<CommunityService> =
            Arc::new(CommunityService::new(Arc::clone(&community_repo)));
        let objects: Arc<ObjectService> = Arc::new(ObjectService::new(
            Arc::clone(&local_storage),
            Arc::clone(&stored_object_repo),
            config.storage.clone(),
        ));

        // ── QQ Bot ──
        #[cfg(feature = "qq_bot")]
        {
            use crate::bootstrap::qq_bot::init_qq_bot;
            use crate::domain::tts::TtsProvider;
            use crate::infra::qq_bot::repositories::seaorm_agent_turn_repository::SeaOrmAgentTurnRepository;
            use crate::infra::qq_bot::repositories::seaorm_bot_account_repository::SeaOrmBotAccountRepository;
            use crate::infra::qq_bot::repositories::seaorm_external_user_repository::SeaOrmExternalUserRepository;
            use crate::infra::qq_bot::repositories::seaorm_group_member_repository::SeaOrmGroupMemberRepository;
            use crate::infra::qq_bot::repositories::seaorm_group_memory_repository::SeaOrmGroupMemoryRepository;
            use crate::infra::qq_bot::repositories::seaorm_group_message_repository::SeaOrmGroupMessageRepository;
            use crate::infra::qq_bot::repositories::seaorm_group_repository::SeaOrmGroupRepository;
            use crate::infra::qq_bot::repositories::seaorm_group_summary_repository::SeaOrmGroupSummaryRepository;
            use crate::infra::qq_bot::repositories::seaorm_outbox_repository::SeaOrmOutboxRepository;
            use crate::infra::qq_bot::repositories::seaorm_relationship_repository::SeaOrmRelationshipRepository;
            use crate::infra::qq_bot::repositories::seaorm_user_profile_repository::SeaOrmQqUserProfileRepository;
            use crate::infra::tts::volcengine_provider::VolcengineTtsProvider;

            let qq_bot_bot_account_repo =
                Arc::new(SeaOrmBotAccountRepository::new(infra.db.clone()))
                    as Arc<dyn crate::domain::qq_bot::repository::BotAccountRepository>;
            let qq_bot_group_repo = Arc::new(SeaOrmGroupRepository::new(infra.db.clone()))
                as Arc<dyn crate::domain::qq_bot::repository::GroupRepository>;
            let qq_bot_group_member_repo =
                Arc::new(SeaOrmGroupMemberRepository::new(infra.db.clone()))
                    as Arc<dyn crate::domain::qq_bot::repository::GroupMemberRepository>;
            let qq_bot_group_message_repo =
                Arc::new(SeaOrmGroupMessageRepository::new(infra.db.clone()))
                    as Arc<dyn crate::domain::qq_bot::repository::GroupMessageRepository>;
            let qq_bot_group_summary_repo =
                Arc::new(SeaOrmGroupSummaryRepository::new(infra.db.clone()))
                    as Arc<dyn crate::domain::qq_bot::repository::GroupSummaryRepository>;
            let qq_bot_group_memory_repo =
                Arc::new(SeaOrmGroupMemoryRepository::new(infra.db.clone()))
                    as Arc<dyn crate::domain::qq_bot::repository::GroupMemoryRepository>;
            let qq_bot_agent_turn_repo = Arc::new(SeaOrmAgentTurnRepository::new(infra.db.clone()))
                as Arc<dyn crate::domain::qq_bot::repository::AgentTurnRepository>;
            let qq_bot_outbox_repo = Arc::new(SeaOrmOutboxRepository::new(infra.db.clone()))
                as Arc<dyn crate::domain::qq_bot::repository::OutboxRepository>;
            let qq_bot_external_user_repo =
                Arc::new(SeaOrmExternalUserRepository::new(infra.db.clone()))
                    as Arc<dyn crate::domain::qq_bot::repository::ExternalUserRepository>;
            let qq_bot_user_profile_repo =
                Arc::new(SeaOrmQqUserProfileRepository::new(infra.db.clone()))
                    as Arc<
                        dyn crate::domain::qq_bot::qq_profile_repository::QqUserProfileRepository,
                    >;
            let qq_bot_relationship_repo =
                Arc::new(SeaOrmRelationshipRepository::new(infra.db.clone()))
                    as Arc<
                        dyn crate::domain::qq_bot::relationship_repository::RelationshipRepository,
                    >;

            let qq_bot_tts_provider: Option<Arc<dyn TtsProvider>> = if config.qq_bot.enabled
                && config.qq_bot.self_qq_id != 0
                && !config.tts.api_key.is_empty()
            {
                tracing::info!("正在为 QQ 机器人语音消息初始化 VolcengineTtsProvider");
                Some(Arc::new(VolcengineTtsProvider::new(&config.tts)) as Arc<dyn TtsProvider>)
            } else {
                if config.qq_bot.enabled {
                    tracing::warn!("未配置 TTS API 密钥 — 语音消息将不可用");
                }
                None
            };

            let _qq_bot_deps = init_qq_bot(
                config,
                Arc::clone(&infra.ollama_provider),
                qq_bot_tts_provider,
                &mut tasks.background,
                qq_bot_bot_account_repo,
                qq_bot_group_repo,
                qq_bot_group_member_repo,
                qq_bot_group_message_repo,
                qq_bot_group_summary_repo,
                qq_bot_group_memory_repo,
                qq_bot_agent_turn_repo,
                qq_bot_outbox_repo,
                Some(Arc::clone(&user_repo)
                    as Arc<dyn crate::domain::user::user_repository::UserRepoT>),
                Some(Arc::clone(&qq_bot_external_user_repo)),
                Some(Arc::clone(&qq_bot_user_profile_repo)),
                Some(Arc::clone(&qq_bot_relationship_repo)),
            )
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "qq_bot 初始化失败 — 将继续运行而不启动它");
                None
            });
        }

        // ── 网页知识摄取 ──
        let knowledge_review = web_ingestion::init_web_ingestion(
            config,
            &infra.db,
            &vector.vector_store,
            &vector.embedding_provider,
            &rag_repo,
            &mut tasks.background,
        )
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

        Ok(Self {
            auth,
            user,
            query,
            objects,
            psychology,
            depression,
            diaries,
            music,
            community,
            retrieval,
            ingestion,
            memory: memory_svc,
            agent_runtime,
            knowledge_review,
            chat: chat_service,
            chat_conv_repo: Arc::clone(&conv_repo) as Arc<dyn ConversationRepoT>,
            token_service: Arc::clone(&auth_graph.token_service),
            risk_repo: Arc::clone(&risk_repo),
        })
    }
}

pub fn build_state(services: &ServiceGraph) -> AppState {
    AppState {
        auth: AuthState {
            auth: Arc::clone(&services.auth),
        },
        user: UserState {
            user: Arc::clone(&services.user),
        },
        chat: ChatState {
            chat: Arc::clone(&services.chat),
            conv_repo: Arc::clone(&services.chat_conv_repo),
        },
        object: ObjectState {
            objects: Arc::clone(&services.objects),
        },
        psychology: PsychologyState {
            psychology: Arc::clone(&services.psychology),
        },
        depression: DepressionState {
            depression: Arc::clone(&services.depression),
        },
        diary: DiaryState {
            diaries: Arc::clone(&services.diaries),
        },
        music: MusicState {
            music: Arc::clone(&services.music),
        },
        community: CommunityState {
            community: Arc::clone(&services.community),
        },
        admin: AdminState {
            user: Arc::clone(&services.user),
            query: Arc::clone(&services.query),
            knowledge_review: Arc::clone(&services.knowledge_review),
            music: Arc::clone(&services.music),
            risk: Arc::clone(&services.risk_repo),
        },
        internal: InternalState {
            retrieval: Arc::clone(&services.retrieval),
            ingestion: Arc::clone(&services.ingestion),
            memory: Arc::clone(&services.memory),
            agent_runtime: Arc::clone(&services.agent_runtime),
        },
        signature: SignatureState {
            token_service: Arc::clone(&services.token_service),
        },
    }
}
