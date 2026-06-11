use std::sync::Arc;

use crate::domain::conversation::conversation::Conversation;
use crate::domain::conversation::conversation_message::ConversationMessage;
use crate::domain::conversation::conversation_repository::ConversationRepository;
use crate::domain::risk::detection_types::RiskLevel;
use crate::domain::risk::risk_detection_result::RiskDetectionResult;
use crate::domain::risk::risk_repository::RiskRepository;
use crate::shared::error::AppError;

/// Unified session-domain operations: conversations + risk detections.
pub struct SessionService {
    conv_repo: Arc<dyn ConversationRepository>,
    risk_repo: Arc<dyn RiskRepository>,
}

impl SessionService {
    pub fn new(
        conv_repo: Arc<dyn ConversationRepository>,
        risk_repo: Arc<dyn RiskRepository>,
    ) -> Self {
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

    // ── Risk detections ──

    pub async fn list_risk_detections(
        &self,
        user_id: u64,
        page: u64,
        size: u64,
    ) -> Result<(Vec<RiskDetectionResult>, u64), AppError> {
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
        risk_level: Option<RiskLevel>,
    ) -> Result<(Vec<Conversation>, u64), AppError> {
        let offset = (page.saturating_sub(1)) * page_size;
        let (detections, total) = self
            .risk_repo
            .find_all_paginated(page_size, offset, risk_level)
            .await?;
        let mut conv_ids: Vec<u64> = detections
            .iter()
            .filter_map(|d| d.conversation_id)
            .collect();
        conv_ids.sort();
        conv_ids.dedup();

        let mut convs = Vec::new();
        for &cid in &conv_ids {
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

    pub async fn admin_get_conversation_risk_detections(
        &self,
        conversation_id: u64,
    ) -> Result<Vec<RiskDetectionResult>, AppError> {
        self.risk_repo
            .find_by_conversation_id(conversation_id)
            .await
    }

    pub async fn admin_process_risk_detection(
        &self,
        id: u64,
        _admin_user_id: u64,
        notes: Option<String>,
    ) -> Result<RiskDetectionResult, AppError> {
        self.risk_repo.mark_processed(id, notes).await
    }
}
