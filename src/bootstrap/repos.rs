use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::domain::agent::AgentEventRepoT;
use crate::domain::community::CommunityRepoT;
use crate::domain::conversation::conversation_repository::ConversationRepoT;
use crate::domain::depression::DepressionRepoT;
use crate::domain::diary::DiaryRepoT;
use crate::domain::memory::MemoryRepoT;
use crate::domain::music::MusicRepoT;
use crate::domain::psychology::PsychologyRepoT;
use crate::domain::rag::RAGRepoT;
use crate::domain::risk::risk_repository::RiskRepoT;
use crate::domain::storage::StoredObjectRepoT;
use crate::domain::summary::SummaryRepoT;
use crate::domain::user::user_context_control::UserContextControlRepoT;
use crate::domain::user::user_context_version::UserContextVersionRepoT;
use crate::domain::user::user_profile_repository::UserProfileRepoT;
use crate::domain::user::user_repository::UserRepoT;
use crate::infra::db::seaorm_impl::agent_repo::AgentEventRepo;
use crate::infra::db::seaorm_impl::community_repo::CommunityRepo;
use crate::infra::db::seaorm_impl::conversation_repo::ConversationRepo;
use crate::infra::db::seaorm_impl::conversation_summary_repo::ConversationSummaryRepo;
use crate::infra::db::seaorm_impl::depression_repo::DepressionRepo;
use crate::infra::db::seaorm_impl::diary_repo::DiaryRepo;
use crate::infra::db::seaorm_impl::memory_repo::MemoryRepo;
use crate::infra::db::seaorm_impl::music_repo::MusicRepo;
use crate::infra::db::seaorm_impl::psychology_repo::PsychologyRepo;
use crate::infra::db::seaorm_impl::rag_repo::RAGRepo;
use crate::infra::db::seaorm_impl::risk_repo::RiskRepo;
use crate::infra::db::seaorm_impl::stored_object_repo::StoredObjectRepo;
use crate::infra::db::seaorm_impl::user_context_control_repo::UserContextControlRepo;
use crate::infra::db::seaorm_impl::user_context_version_repo::UserContextVersionRepo;
use crate::infra::db::seaorm_impl::user_profile_repo::UserProfileRepo;
use crate::infra::db::seaorm_impl::user_repo::UserRepo;

pub struct RepoGraph {
    pub user_repo: Arc<dyn UserRepoT>,
    pub profile_repo: Arc<dyn UserProfileRepoT>,
    pub context_version_repo: Arc<dyn UserContextVersionRepoT>,
    pub context_control_repo: Arc<dyn UserContextControlRepoT>,
    pub conv_repo: Arc<dyn ConversationRepoT>,
    pub risk_repo: Arc<dyn RiskRepoT>,
    pub psychology_repo: Arc<dyn PsychologyRepoT>,
    pub depression_repo: Arc<dyn DepressionRepoT>,
    pub diary_repo: Arc<dyn DiaryRepoT>,
    pub music_repo: Arc<dyn MusicRepoT>,
    pub community_repo: Arc<dyn CommunityRepoT>,
    pub agent_event_repo: Arc<dyn AgentEventRepoT>,
    pub stored_object_repo: Arc<dyn StoredObjectRepoT>,
    pub rag_repo: Arc<dyn RAGRepoT>,
    pub memory_repo: Arc<dyn MemoryRepoT>,
    pub summary_repo: Arc<dyn SummaryRepoT>,
}

pub fn build_repos(
    db: &DatabaseConnection,
    memory_collection: &str,
    summary_collection: &str,
) -> RepoGraph {
    RepoGraph {
        user_repo: Arc::new(UserRepo::new(db.clone())),
        profile_repo: Arc::new(UserProfileRepo::new(db.clone())),
        context_version_repo: Arc::new(UserContextVersionRepo::new(db.clone())),
        context_control_repo: Arc::new(UserContextControlRepo::new(
            db.clone(),
            memory_collection.to_string(),
            summary_collection.to_string(),
        )),
        conv_repo: Arc::new(ConversationRepo::new(db.clone())),
        risk_repo: Arc::new(RiskRepo::new(db.clone())),
        psychology_repo: Arc::new(PsychologyRepo::new(db.clone())),
        depression_repo: Arc::new(DepressionRepo::new(db.clone())),
        diary_repo: Arc::new(DiaryRepo::new(db.clone())),
        music_repo: Arc::new(MusicRepo::new(db.clone())),
        community_repo: Arc::new(CommunityRepo::new(db.clone())),
        agent_event_repo: Arc::new(AgentEventRepo::new(db.clone())),
        stored_object_repo: Arc::new(StoredObjectRepo::new(db.clone())),
        rag_repo: Arc::new(RAGRepo::new(db.clone())),
        memory_repo: Arc::new(MemoryRepo::new(db.clone())),
        summary_repo: Arc::new(ConversationSummaryRepo::new(db.clone())),
    }
}
