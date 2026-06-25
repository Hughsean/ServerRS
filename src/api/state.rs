use std::sync::Arc;

use axum::extract::FromRef;

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
use crate::domain::auth::token_service::TokenService;
use crate::domain::conversation::conversation_repository::ConversationRepository;
use crate::domain::risk::risk_repository::RiskRepository;

#[derive(Clone, FromRef)]
pub struct AppState {
    pub auth: AuthState,
    pub user: UserState,
    pub chat: ChatState,
    pub object: ObjectState,
    pub psychology: PsychologyState,
    pub depression: DepressionState,
    pub diary: DiaryState,
    pub music: MusicState,
    pub community: CommunityState,
    pub admin: AdminState,
    pub internal: InternalState,
    pub signature: SignatureState,
}

#[derive(Clone)]
pub struct AuthState {
    pub auth: Arc<AuthService>,
}

#[derive(Clone)]
pub struct SignatureState {
    pub token_service: Arc<dyn TokenService>,
}

#[derive(Clone)]
pub struct UserState {
    pub user: Arc<UserService>,
}

#[derive(Clone)]
pub struct ChatState {
    pub chat: Arc<ChatService>,
    pub conv_repo: Arc<dyn ConversationRepository>,
}

#[derive(Clone)]
pub struct ObjectState {
    pub objects: Arc<ObjectService>,
}

#[derive(Clone)]
pub struct PsychologyState {
    pub psychology: Arc<PsychologyService>,
}

#[derive(Clone)]
pub struct DepressionState {
    pub depression: Arc<DepressionService>,
}

#[derive(Clone)]
pub struct DiaryState {
    pub diaries: Arc<DiaryService>,
}

#[derive(Clone)]
pub struct MusicState {
    pub music: Arc<MusicService>,
}

#[derive(Clone)]
pub struct CommunityState {
    pub community: Arc<CommunityService>,
}

#[derive(Clone)]
pub struct AdminState {
    pub user: Arc<UserService>,
    pub query: Arc<SessionService>,
    pub knowledge_review: Arc<KnowledgeReviewService>,
    pub music: Arc<MusicService>,
    pub risk: Arc<dyn RiskRepository>,
}

#[derive(Clone)]
pub struct InternalState {
    pub retrieval: Arc<RetrievalService>,
    pub ingestion: Arc<IngestionService>,
    pub memory: Arc<MemoryService>,
    pub agent_runtime: Arc<AgentRuntime>,
}
