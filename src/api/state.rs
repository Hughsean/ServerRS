use std::sync::Arc;

use axum::extract::FromRef;

use crate::application::agent::agent_runtime::AgentRuntime;
use crate::application::auth::auth_service::AuthService;
use crate::application::community::community_service::CommunityService;
use crate::application::depression::depression_service::DepressionService;
use crate::application::diary::diary_service::DiaryService;
use crate::application::memory::memory_service::MemoryService;
use crate::application::music::music_service::MusicService;
use crate::application::psychology::psychology_service::PsychologyService;
use crate::application::rag::ingestion_service::IngestionService;
use crate::application::rag::retrieval_service::RetrievalService;
use crate::application::session::session_manager::SessionManager;
use crate::application::session::session_service::SessionService;
use crate::application::storage::object_service::ObjectService;
use crate::application::user::user_service::UserService;
use crate::application::web_ingestion::review_service::KnowledgeReviewService;

#[derive(Clone, FromRef)]
pub struct AppState {
    pub auth: AuthState,
    pub user: UserState,
    pub session: SessionState,
    pub object: ObjectState,
    pub psychology: PsychologyState,
    pub depression: DepressionState,
    pub diary: DiaryState,
    pub music: MusicState,
    pub community: CommunityState,
    pub admin: AdminState,
    pub internal: InternalState,
}

#[derive(Clone)]
pub struct AuthState {
    pub auth: Arc<AuthService>,
}

#[derive(Clone)]
pub struct UserState {
    pub user: Arc<UserService>,
}

#[derive(Clone)]
pub struct SessionState {
    pub session: Arc<SessionManager>,
    pub query: Arc<SessionService>,
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
}

#[derive(Clone)]
pub struct InternalState {
    pub retrieval: Arc<RetrievalService>,
    pub ingestion: Arc<IngestionService>,
    pub memory: Arc<MemoryService>,
    pub agent_runtime: Arc<AgentRuntime>,
}
