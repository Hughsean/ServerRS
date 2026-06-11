use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::domain::agent::AgentEventRepository;
use crate::domain::community::CommunityRepository;
use crate::domain::conversation::conversation_repository::ConversationRepository;
use crate::domain::depression::DepressionRepository;
use crate::domain::diary::DiaryRepository;
use crate::domain::memory::MemoryRepository;
use crate::domain::music::MusicRepository;
use crate::domain::psychology::PsychologyRepository;
use crate::domain::rag::RAGRepository;
use crate::domain::risk::risk_repository::RiskRepository;
use crate::domain::storage::StoredObjectRepository;
use crate::domain::summary::SummaryRepository;
use crate::domain::user::user_profile_repository::UserProfileRepository;
use crate::domain::user::user_repository::UserRepository;
use crate::infrastructure::persistence::implementations::seaorm_agent_repository::SeaOrmAgentEventRepository;
use crate::infrastructure::persistence::implementations::seaorm_community_repository::SeaOrmCommunityRepository;
use crate::infrastructure::persistence::implementations::seaorm_conversation_repository::SeaOrmConversationRepository;
use crate::infrastructure::persistence::implementations::seaorm_conversation_summary_repository::SeaOrmConversationSummaryRepository;
use crate::infrastructure::persistence::implementations::seaorm_depression_repository::SeaOrmDepressionRepository;
use crate::infrastructure::persistence::implementations::seaorm_diary_repository::SeaOrmDiaryRepository;
use crate::infrastructure::persistence::implementations::seaorm_memory_repository::SeaOrmMemoryRepository;
use crate::infrastructure::persistence::implementations::seaorm_music_repository::SeaOrmMusicRepository;
use crate::infrastructure::persistence::implementations::seaorm_psychology_repository::SeaOrmPsychologyRepository;
use crate::infrastructure::persistence::implementations::seaorm_rag_repository::SeaOrmRAGRepository;
use crate::infrastructure::persistence::implementations::seaorm_risk_repository::SeaOrmRiskRepository;
use crate::infrastructure::persistence::implementations::seaorm_stored_object_repository::SeaOrmStoredObjectRepository;
use crate::infrastructure::persistence::implementations::seaorm_user_profile_repository::SeaOrmUserProfileRepository;
use crate::infrastructure::persistence::implementations::seaorm_user_repository::SeaOrmUserRepository;

pub struct RepoGraph {
    pub user_repo: Arc<dyn UserRepository>,
    pub profile_repo: Arc<dyn UserProfileRepository>,
    pub conv_repo: Arc<dyn ConversationRepository>,
    pub risk_repo: Arc<dyn RiskRepository>,
    pub psychology_repo: Arc<dyn PsychologyRepository>,
    pub depression_repo: Arc<dyn DepressionRepository>,
    pub diary_repo: Arc<dyn DiaryRepository>,
    pub music_repo: Arc<dyn MusicRepository>,
    pub community_repo: Arc<dyn CommunityRepository>,
    pub agent_event_repo: Arc<dyn AgentEventRepository>,
    pub stored_object_repo: Arc<dyn StoredObjectRepository>,
    pub rag_repo: Arc<dyn RAGRepository>,
    pub memory_repo: Arc<dyn MemoryRepository>,
    pub summary_repo: Arc<dyn SummaryRepository>,
}

pub fn build_repos(db: &DatabaseConnection) -> RepoGraph {
    RepoGraph {
        user_repo: Arc::new(SeaOrmUserRepository::new(db.clone())),
        profile_repo: Arc::new(SeaOrmUserProfileRepository::new(db.clone())),
        conv_repo: Arc::new(SeaOrmConversationRepository::new(db.clone())),
        risk_repo: Arc::new(SeaOrmRiskRepository::new(db.clone())),
        psychology_repo: Arc::new(SeaOrmPsychologyRepository::new(db.clone())),
        depression_repo: Arc::new(SeaOrmDepressionRepository::new(db.clone())),
        diary_repo: Arc::new(SeaOrmDiaryRepository::new(db.clone())),
        music_repo: Arc::new(SeaOrmMusicRepository::new(db.clone())),
        community_repo: Arc::new(SeaOrmCommunityRepository::new(db.clone())),
        agent_event_repo: Arc::new(SeaOrmAgentEventRepository::new(db.clone())),
        stored_object_repo: Arc::new(SeaOrmStoredObjectRepository::new(db.clone())),
        rag_repo: Arc::new(SeaOrmRAGRepository::new(db.clone())),
        memory_repo: Arc::new(SeaOrmMemoryRepository::new(db.clone())),
        summary_repo: Arc::new(SeaOrmConversationSummaryRepository::new(db.clone())),
    }
}
