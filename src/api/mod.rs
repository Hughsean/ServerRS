pub mod dto;
pub mod handlers;
pub mod middleware;
pub mod response;
pub mod router;
pub mod routes;

use std::sync::Arc;

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

#[derive(Clone)]
pub struct ApiState {
    pub auth: Arc<AuthService>,
    pub user: Arc<UserService>,
    pub session: Arc<SessionManager>,
    pub query: Arc<SessionService>,
    pub objects: Arc<ObjectService>,
    pub psychology: Arc<PsychologyService>,
    pub depression: Arc<DepressionService>,
    pub diaries: Arc<DiaryService>,
    pub music: Arc<MusicService>,
    pub community: Arc<CommunityService>,
    // ── New agent / RAG / memory services ──
    pub retrieval: Arc<RetrievalService>,
    pub ingestion: Arc<IngestionService>,
    pub memory: Arc<MemoryService>,
    pub agent_runtime: Arc<AgentRuntime>,
}
