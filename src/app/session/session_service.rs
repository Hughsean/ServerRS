use std::sync::Arc;

use crate::domain::conversation::conversation::Conversation;
use crate::domain::conversation::conversation_message::ConversationMessage;
use crate::domain::conversation::conversation_repository::ConversationRepoT;
use crate::domain::risk::post_conversation_risk_audit::PostConversationRiskAudit;
use crate::domain::risk::risk_repository::RiskRepoT;
use crate::shared::error::AppError;

/// Unified session-domain operations: conversations + post-conversation risk audits.
///
/// Risk data here comes from `post_conversation_risk_audits` — it never enters
/// the conversation generation path (PromptBuilder/Persona/Memory/Summary).
pub struct SessionService {
    conv_repo: Arc<dyn ConversationRepoT>,
    risk_repo: Arc<dyn RiskRepoT>,
}

impl SessionService {
    pub fn new(conv_repo: Arc<dyn ConversationRepoT>, risk_repo: Arc<dyn RiskRepoT>) -> Self {
        Self {
            conv_repo,
            risk_repo,
        }
    }

    // ── Conversations ──

    pub async fn list_conversations(&self, user_id: u64) -> Result<Vec<Conversation>, AppError> {
        self.conv_repo.find_by_user_id(user_id).await
    }

    pub async fn list_messages(
        &self,
        conversation_id: u64,
        requesting_user_id: u64,
    ) -> Result<Vec<ConversationMessage>, AppError> {
        let conv = self
            .conv_repo
            .find_by_id(conversation_id)
            .await?
            .ok_or(AppError::NotFound("conversation not found".into()))?;
        if conv.user_id != requesting_user_id {
            return Err(AppError::Forbidden("not your conversation".into()));
        }
        self.conv_repo
            .find_messages_by_conversation_id(conversation_id)
            .await
    }

    // ── Post-conversation risk audits (user view) ──

    pub async fn list_risk_audits(
        &self,
        user_id: u64,
        page: u64,
        size: u64,
    ) -> Result<(Vec<PostConversationRiskAudit>, u64), AppError> {
        let offset = (page.saturating_sub(1)) * size;
        self.risk_repo
            .find_by_user_id_paginated(user_id, size, offset)
            .await
    }

    // ── Admin methods ──

    pub async fn admin_list_risk_conversations(
        &self,
        page: u64,
        page_size: u64,
        risk_level: Option<String>,
    ) -> Result<(Vec<Conversation>, u64), AppError> {
        let page = page.max(1);
        let page_size = page_size.clamp(1, 100);
        let offset = (page.saturating_sub(1)) * page_size;
        let (conv_ids, total) = self
            .risk_repo
            .find_conversation_ids_paginated(page_size, offset, risk_level.as_deref())
            .await?;

        let mut convs = Vec::new();
        for cid in conv_ids {
            if let Some(c) = self.conv_repo.find_by_id(cid).await? {
                convs.push(c);
            }
        }
        Ok((convs, total))
    }

    pub async fn admin_get_conversation(&self, id: u64) -> Result<Option<Conversation>, AppError> {
        self.conv_repo.find_by_id(id).await
    }

    pub async fn admin_get_conversation_messages(
        &self,
        id: u64,
    ) -> Result<Vec<ConversationMessage>, AppError> {
        self.conv_repo.find_messages_by_conversation_id(id).await
    }

    pub async fn admin_get_conversation_risk_audits(
        &self,
        conversation_id: u64,
    ) -> Result<Vec<PostConversationRiskAudit>, AppError> {
        self.risk_repo
            .find_by_conversation_id(conversation_id)
            .await
    }
}
