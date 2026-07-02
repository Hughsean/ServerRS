use std::sync::Arc;

use crate::api::{
    AdminState, AppState, AuthState, ChatState, CommunityState, DepressionState, DiaryState,
    InternalState, MusicState, ObjectState, PsychologyState, SignatureState, UserState,
};
use crate::app::agent::agent_runtime::AgentRuntime;
use crate::app::auth::auth_service::AuthService;
use crate::app::community::community_service::CommunityService;
use crate::app::depression::depression_service::DepressionService;
use crate::app::diary::diary_service::DiaryService;
use crate::app::memory::memory_service::MemoryService;
use crate::app::music::music_service::MusicService;
use crate::app::psychology::psychology_service::PsychologyService;
use crate::app::rag::ingestion_service::IngestionService;
use crate::app::rag::retrieval_service::RetrievalService;
use crate::app::session::chat_service::ChatService;
use crate::app::session::session_service::SessionService;
use crate::app::storage::object_service::ObjectService;
use crate::app::user::user_service::UserService;
use crate::app::web_ingestion::review_service::KnowledgeReviewService;
use crate::bootstrap::auth::AuthGraph;
use crate::bootstrap::fresh_context;
use crate::bootstrap::graph::{
    BootstrapContext, build_agent_services, build_domain_services, build_memory_services,
    build_rag_services, build_risk_services, build_summary_services,
};
use crate::bootstrap::infra::InfraContext;
use crate::bootstrap::repos::RepoGraph;
use crate::bootstrap::tasks::TaskContext;
use crate::bootstrap::vector::VectorContext;
use crate::bootstrap::web_ingestion;
use crate::domain::auth::token_service::TokenServiceT;
use crate::domain::conversation::conversation_repository::ConversationRepoT;
use crate::domain::risk::risk_repository::RiskRepoT;
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
        auth_graph: &AuthGraph,
    ) -> Result<Self, std::io::Error> {
        let user_repo = Arc::clone(&repos.user_repo);
        let profile_repo = Arc::clone(&repos.profile_repo);
        let context_control_repo = Arc::clone(&repos.context_control_repo);
        let conv_repo = Arc::clone(&repos.conv_repo);
        let risk_repo = Arc::clone(&repos.risk_repo);
        let rag_repo = Arc::clone(&repos.rag_repo);
        let ctx = BootstrapContext {
            config,
            infra,
            repos,
            vector,
        };
        let risk = build_risk_services(&ctx, Arc::clone(&tasks.task_publisher));

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

        // ── RAG、记忆、摘要服务 ──
        let rag = build_rag_services(&ctx);
        let retrieval = Arc::clone(&rag.retrieval);
        let ingestion = Arc::clone(&rag.ingestion);

        let memory = build_memory_services(&ctx);
        let memory_svc = Arc::clone(&memory.memory);

        let summary = build_summary_services(&ctx);

        // ── 注册后台任务 ──
        tasks.start_service_handlers(risk.risk_audit_worker, summary.summary_refresh_handler);

        // ── 代理运行时 ──
        let agent = build_agent_services(
            &ctx,
            Arc::clone(&retrieval),
            Arc::clone(&memory_svc),
            Arc::clone(&summary.summary),
        )
        .await?;
        let agent_runtime = Arc::clone(&agent.runtime);

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
        let domain = build_domain_services(&ctx);

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

        // ── Fresh Context 短期上下文 ──
        fresh_context::init_fresh_context(
            config,
            &infra.db,
            &vector.vector_store,
            &vector.embedding_provider,
            &mut tasks.background,
        )
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

        Ok(Self {
            auth,
            user,
            query,
            objects: domain.objects,
            psychology: domain.psychology,
            depression: domain.depression,
            diaries: domain.diaries,
            music: domain.music,
            community: domain.community,
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
