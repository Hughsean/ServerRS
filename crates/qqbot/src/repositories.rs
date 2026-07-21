//! Stable repository assembly API for QQ business persistence.

use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::domain::qq_bot::qq_profile_repo::QqUserProfileRepoT;
use crate::domain::qq_bot::relationship_repo::RelationshipRepoT;
use crate::domain::qq_bot::repository::{
    AgentTurnRepoT, BotAccountRepoT, ExternalUserRepoT, GroupMemberRepoT, GroupMemoryRepoT,
    GroupMessageRepoT, GroupRepoT, GroupSummaryRepoT, OutboxRepoT,
};
use crate::infra::qq_bot::repo::seaorm_impl::agent_turn::AgentTurnRepo;
use crate::infra::qq_bot::repo::seaorm_impl::bot_account::BotAccountRepo;
use crate::infra::qq_bot::repo::seaorm_impl::external_user::ExternalUserRepo;
use crate::infra::qq_bot::repo::seaorm_impl::group::GroupRepo;
use crate::infra::qq_bot::repo::seaorm_impl::group_member::GroupMemberRepo;
use crate::infra::qq_bot::repo::seaorm_impl::group_memory::GroupMemoryRepo;
use crate::infra::qq_bot::repo::seaorm_impl::group_message::GroupMessageRepo;
use crate::infra::qq_bot::repo::seaorm_impl::group_summary::GroupSummaryRepo;
use crate::infra::qq_bot::repo::seaorm_impl::outbox::OutboxRepo;
use crate::infra::qq_bot::repo::seaorm_impl::relationship::RelationshipRepo;
use crate::infra::qq_bot::repo::seaorm_impl::user_profile::QqUserProfileRepo;

pub struct RepositorySet {
    pub bot_account: Arc<dyn BotAccountRepoT>,
    pub group: Arc<dyn GroupRepoT>,
    pub group_member: Arc<dyn GroupMemberRepoT>,
    pub group_message: Arc<dyn GroupMessageRepoT>,
    pub group_summary: Arc<dyn GroupSummaryRepoT>,
    pub group_memory: Arc<dyn GroupMemoryRepoT>,
    pub agent_turn: Arc<dyn AgentTurnRepoT>,
    pub outbox: Arc<dyn OutboxRepoT>,
    pub external_user: Arc<dyn ExternalUserRepoT>,
    pub user_profile: Arc<dyn QqUserProfileRepoT>,
    pub relationship: Arc<dyn RelationshipRepoT>,
}

pub fn build_repositories(db: &DatabaseConnection) -> RepositorySet {
    RepositorySet {
        bot_account: Arc::new(BotAccountRepo::new(db.clone())),
        group: Arc::new(GroupRepo::new(db.clone())),
        group_member: Arc::new(GroupMemberRepo::new(db.clone())),
        group_message: Arc::new(GroupMessageRepo::new(db.clone())),
        group_summary: Arc::new(GroupSummaryRepo::new(db.clone())),
        group_memory: Arc::new(GroupMemoryRepo::new(db.clone())),
        agent_turn: Arc::new(AgentTurnRepo::new(db.clone())),
        outbox: Arc::new(OutboxRepo::new(db.clone())),
        external_user: Arc::new(ExternalUserRepo::new(db.clone())),
        user_profile: Arc::new(QqUserProfileRepo::new(db.clone())),
        relationship: Arc::new(RelationshipRepo::new(db.clone())),
    }
}
