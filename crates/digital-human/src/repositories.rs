//! Stable repository assembly API for the digital-human boundary.
//!
//! Concrete SeaORM repository types stay inside this crate. Hosts receive the
//! domain ports as a single aggregate and do not need to know which adapter
//! implements each port.

use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::app::agent::chat_state::ChatTurnState;
use crate::app::agent::graph::CheckpointStore;
use crate::domain::agent::{AgentEventRepoT, ChatApprovalAuditT, ChatApprovalQueryT};
use crate::domain::auth::refresh_token_store::RefreshTokenStoreT;
use crate::domain::community::CommunityRepoT;
use crate::domain::conversation::conversation_repo::ConversationRepoT;
use crate::domain::depression::DepressionRepoT;
use crate::domain::diary::DiaryRepoT;
use crate::domain::fresh_context::FreshContextRepoT;
use crate::domain::memory::MemoryRepoT;
use crate::domain::music::MusicRepoT;
use crate::domain::psychology::PsychologyRepoT;
use crate::domain::rag::RAGRepoT;
use crate::domain::risk::risk_repo::RiskRepoT;
use crate::domain::storage::StoredObjectRepoT;
use crate::domain::summary::SummaryRepoT;
use crate::domain::user::user_context_control::UserContextControlRepoT;
use crate::domain::user::user_context_version::UserContextVersionRepoT;
use crate::domain::user::user_profile_repo::UserProfileRepoT;
use crate::domain::user::user_repo::UserRepoT;
use crate::domain::vector_index::VectorIndexRepoT;
use crate::infra::repo::seaorm_impl::agent::{AgentEventRepo, ChatApprovalAuditRepo};
use crate::infra::repo::seaorm_impl::agent_checkpoint::MySqlCheckpointStore;
use crate::infra::repo::seaorm_impl::community::CommunityRepo;
use crate::infra::repo::seaorm_impl::conversation::ConversationRepo;
use crate::infra::repo::seaorm_impl::conversation_summary::ConversationSummaryRepo;
use crate::infra::repo::seaorm_impl::depression::DepressionRepo;
use crate::infra::repo::seaorm_impl::diary::DiaryRepo;
use crate::infra::repo::seaorm_impl::fresh_context::FreshContextRepo;
use crate::infra::repo::seaorm_impl::memory::MemoryRepo;
use crate::infra::repo::seaorm_impl::music::MusicRepo;
use crate::infra::repo::seaorm_impl::psychology::PsychologyRepo;
use crate::infra::repo::seaorm_impl::rag::RAGRepo;
use crate::infra::repo::seaorm_impl::refresh_token_store::RefreshTokenStoreImpl;
use crate::infra::repo::seaorm_impl::risk::RiskRepo;
use crate::infra::repo::seaorm_impl::stored_object::StoredObjectRepo;
use crate::infra::repo::seaorm_impl::user::UserRepo;
use crate::infra::repo::seaorm_impl::user_context_control::UserContextControlRepo;
use crate::infra::repo::seaorm_impl::user_context_version::UserContextVersionRepo;
use crate::infra::repo::seaorm_impl::user_profile::UserProfileRepo;
use crate::infra::repo::seaorm_impl::vector_index::VectorIndexRepo;

pub struct RepositorySet {
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
    pub chat_approval_query: Arc<dyn ChatApprovalQueryT>,
    pub chat_approval_audit: Arc<dyn ChatApprovalAuditT>,
    pub stored_object_repo: Arc<dyn StoredObjectRepoT>,
    pub rag_repo: Arc<dyn RAGRepoT>,
    pub memory_repo: Arc<dyn MemoryRepoT>,
    pub summary_repo: Arc<dyn SummaryRepoT>,
}

pub fn build_repositories(
    db: &DatabaseConnection,
    memory_collection: &str,
    summary_collection: &str,
) -> RepositorySet {
    RepositorySet {
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
        chat_approval_query: Arc::new(MySqlCheckpointStore::<ChatTurnState>::for_approval_query(
            db.clone(),
        )),
        chat_approval_audit: Arc::new(ChatApprovalAuditRepo::new(db.clone())),
        stored_object_repo: Arc::new(StoredObjectRepo::new(db.clone())),
        rag_repo: Arc::new(RAGRepo::new(db.clone())),
        memory_repo: Arc::new(MemoryRepo::new(db.clone())),
        summary_repo: Arc::new(ConversationSummaryRepo::new(db.clone())),
    }
}

pub fn build_refresh_token_store(
    db: &DatabaseConnection,
    refresh_ttl_secs: u64,
) -> Arc<dyn RefreshTokenStoreT> {
    Arc::new(RefreshTokenStoreImpl::new(db.clone(), refresh_ttl_secs))
}

pub fn build_fresh_context_repository(db: &DatabaseConnection) -> Arc<dyn FreshContextRepoT> {
    Arc::new(FreshContextRepo::new(db.clone()))
}

pub fn build_vector_index_repository(db: &DatabaseConnection) -> Arc<dyn VectorIndexRepoT> {
    Arc::new(VectorIndexRepo::new(db.clone()))
}

pub fn build_chat_checkpoint_store(
    db: &DatabaseConnection,
    ttl_secs: u64,
) -> Arc<dyn CheckpointStore<ChatTurnState>> {
    Arc::new(MySqlCheckpointStore::<ChatTurnState>::new(
        db.clone(),
        ttl_secs,
    ))
}
