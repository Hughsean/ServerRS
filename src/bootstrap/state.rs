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
use crate::bootstrap::graph::{
    BootstrapContext, build_agent_services, build_domain_services, build_identity_services,
    build_integration_services, build_memory_services, build_rag_services, build_risk_services,
    build_session_services, build_summary_services,
};
use crate::bootstrap::infra::InfraContext;
use crate::bootstrap::repos::RepoGraph;
use crate::bootstrap::tasks::TaskContext;
use crate::bootstrap::vector::VectorContext;
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
        let ctx = BootstrapContext {
            config,
            infra,
            repos,
            vector,
        };
        let risk = build_risk_services(&ctx, Arc::clone(&tasks.task_publisher));

        // ── 身份服务 ──
        let identity = build_identity_services(&ctx, auth_graph);

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

        // ── 会话服务 ──
        let session = build_session_services(
            &ctx,
            Arc::clone(&tasks.task_publisher),
            Arc::clone(&agent_runtime),
            Arc::clone(&memory_svc),
        );

        // ── 领域服务 ──
        let domain = build_domain_services(&ctx);

        // ── 集成子系统 ──
        let integrations = build_integration_services(&ctx, &mut tasks.background).await?;
        let knowledge_review = integrations.knowledge_review;

        Ok(Self {
            auth: identity.auth,
            user: identity.user,
            query: session.query,
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
            chat: session.chat,
            chat_conv_repo: session.conv_repo,
            token_service: identity.token_service,
            risk_repo: Arc::clone(&repos.risk_repo),
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
