use std::sync::Arc;

use crate::api::{
    AdminState, AppState, AuthState, CommunityState, DepressionState, DiaryState, InternalState,
    MusicState, ObjectState, PsychologyState, SessionState, UserState,
};
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
pub struct ServiceGraph {
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
    pub retrieval: Arc<RetrievalService>,
    pub ingestion: Arc<IngestionService>,
    pub memory: Arc<MemoryService>,
    pub agent_runtime: Arc<AgentRuntime>,
}

pub fn build_state(services: &ServiceGraph) -> AppState {
    AppState {
        auth: AuthState {
            auth: Arc::clone(&services.auth),
        },
        user: UserState {
            user: Arc::clone(&services.user),
        },
        session: SessionState {
            session: Arc::clone(&services.session),
            query: Arc::clone(&services.query),
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
        },
        internal: InternalState {
            retrieval: Arc::clone(&services.retrieval),
            ingestion: Arc::clone(&services.ingestion),
            memory: Arc::clone(&services.memory),
            agent_runtime: Arc::clone(&services.agent_runtime),
        },
    }
}
