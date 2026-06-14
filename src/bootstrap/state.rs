use std::sync::Arc;

use crate::api::{
    AdminState, AppState, AuthState, ChatState, CommunityState, DepressionState, DiaryState,
    InternalState, MusicState, ObjectState, PsychologyState, UserState,
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
use crate::domain::conversation::conversation_repository::ConversationRepository;

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
    pub chat_conv_repo: Arc<dyn ConversationRepository>,
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
            chat_service: Arc::clone(&services.chat),
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
        },
        internal: InternalState {
            retrieval: Arc::clone(&services.retrieval),
            ingestion: Arc::clone(&services.ingestion),
            memory: Arc::clone(&services.memory),
            agent_runtime: Arc::clone(&services.agent_runtime),
        },
    }
}
