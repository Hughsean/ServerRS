use std::sync::Arc;

use tracing::{info, warn};

use crate::domain::qq_bot::QqBotError;
use crate::domain::qq_bot::config::{ExternalUser, GroupMember};
use crate::domain::qq_bot::repository::{ExternalUserRepoT, GroupMemberRepoT};
use crate::infra::qq_bot::napcat::api::NapCatApiClient;

use super::listener::GroupNoticeHandler;

/// Handles OneBot group notice events (member join/leave) by syncing to database.
pub struct NapCatGroupNoticeHandler {
    member_repo: Arc<dyn GroupMemberRepoT>,
    external_user_repo: Arc<dyn ExternalUserRepoT>,
    napcat_api: Option<Arc<NapCatApiClient>>,
}

impl NapCatGroupNoticeHandler {
    pub fn new(
        member_repo: Arc<dyn GroupMemberRepoT>,
        external_user_repo: Arc<dyn ExternalUserRepoT>,
        napcat_api: Option<Arc<NapCatApiClient>>,
    ) -> Self {
        Self {
            member_repo,
            external_user_repo,
            napcat_api,
        }
    }
}

#[async_trait::async_trait]
impl GroupNoticeHandler for NapCatGroupNoticeHandler {
    async fn handle_group_increase(
        &self,
        group_id: i64,
        user_id: i64,
        operator_id: Option<i64>,
    ) -> Result<(), QqBotError> {
        info!(
            group_id,
            user_id,
            ?operator_id,
            "group_increase: member joined"
        );

        // 1. Ensure external_user exists (create placeholder if needed)
        let external = self
            .external_user_repo
            .find_by_qq_user_id(user_id)
            .await
            .map_err(|e| QqBotError::Internal(format!("find external user: {e}")))?;

        if external.is_none() {
            // Create minimal external user record
            let new_user = ExternalUser {
                qq_user_id: user_id,
                internal_user_id: None,
                nickname: None,
                avatar_url: None,
                last_seen_at: None,
                memory_enabled: false,
                persona_enabled: false,
            };
            self.external_user_repo
                .upsert(&new_user)
                .await
                .map_err(|e| QqBotError::Internal(format!("create external user: {e}")))?;
            info!(user_id, "created placeholder external_user for new member");
        }

        // 2. Try to fetch rich member info from NapCat API if available
        let (card, nickname, role, title, join_time) = if let Some(ref api) = self.napcat_api {
            match api.get_group_member_info(group_id, user_id).await {
                Ok(info) => (
                    info.card,
                    Some(info.nickname),
                    info.role,
                    info.title,
                    info.join_time,
                ),
                Err(e) => {
                    warn!(
                        group_id,
                        user_id,
                        error = %e,
                        "get_group_member_info failed, using defaults"
                    );
                    (None, None, None, None, None)
                }
            }
        } else {
            (None, None, None, None, None)
        };

        // 3. Upsert group member
        let member = GroupMember {
            qq_group_id: group_id,
            qq_user_id: user_id,
            card,
            nickname,
            role,
            title,
            join_time,
            last_seen_at: None,
            status: "active".to_string(),
        };

        self.member_repo
            .upsert(&member)
            .await
            .map_err(|e| QqBotError::Internal(format!("upsert group member: {e}")))?;

        info!(group_id, user_id, "group member upserted (active)");
        Ok(())
    }

    async fn handle_group_decrease(
        &self,
        group_id: i64,
        user_id: i64,
        sub_type: &str,
    ) -> Result<(), QqBotError> {
        let status = match sub_type {
            "kick" | "kick_me" => "kicked",
            _ => "left", // "leave" or anything else
        };

        info!(
            group_id,
            user_id,
            sub_type,
            new_status = status,
            "group_decrease: member removed"
        );

        // Fetch existing member record to update status
        let existing = self
            .member_repo
            .find(group_id, user_id)
            .await
            .map_err(|e| QqBotError::Internal(format!("find group member: {e}")))?;

        if let Some(mut member) = existing {
            member.status = status.to_string();
            self.member_repo
                .upsert(&member)
                .await
                .map_err(|e| QqBotError::Internal(format!("upsert group member: {e}")))?;
            info!(
                group_id,
                user_id,
                new_status = status,
                "group member status updated"
            );
        } else {
            // Member not in our DB yet — create a record with left/kicked status
            let member = GroupMember {
                qq_group_id: group_id,
                qq_user_id: user_id,
                card: None,
                nickname: None,
                role: None,
                title: None,
                join_time: None,
                last_seen_at: None,
                status: status.to_string(),
            };
            self.member_repo
                .upsert(&member)
                .await
                .map_err(|e| QqBotError::Internal(format!("upsert group member: {e}")))?;
            info!(
                group_id,
                user_id,
                new_status = status,
                "group member created (was not in DB)"
            );
        }

        Ok(())
    }
}
