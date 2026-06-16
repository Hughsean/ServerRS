use async_trait::async_trait;

use super::post_conversation_risk_audit::{
    NewPostConversationRiskAudit, PostConversationRiskAudit, PostRiskAuditResult,
};
use crate::shared::error::AppError;

/// `post_conversation_risk_audits` 的仓库。
///
/// 这是**唯一**的风险数据存储 in the new architecture. It is strictly
/// decoupled from the conversation generation path: ChatService, AgentRuntime,
/// PromptBuilder, Persona, Memory and Summary never read from here.
#[async_trait]
pub trait RiskRepository: Send + Sync {
    /// Create a pending audit row and return it.
    async fn create_pending(
        &self,
        new_audit: NewPostConversationRiskAudit,
    ) -> Result<PostConversationRiskAudit, AppError>;

    /// Fetch up to `limit` audits still in `pending` status.
    async fn fetch_pending(&self, limit: u64) -> Result<Vec<PostConversationRiskAudit>, AppError>;

    /// Transition an audit to `running`.
    async fn mark_running(&self, audit_id: u64) -> Result<(), AppError>;

    /// Transition an audit to `completed` and record the detector result.
    async fn mark_completed(
        &self,
        audit_id: u64,
        result: PostRiskAuditResult,
    ) -> Result<(), AppError>;

    /// Transition an audit to `failed` with an error message.
    async fn mark_failed(&self, audit_id: u64, error_message: String) -> Result<(), AppError>;

    /// Delete every audit for a user (used by `forget`). Returns rows affected.
    async fn delete_for_user(&self, user_id: u64) -> Result<u64, AppError>;

    /// Delete every audit for a conversation (used by `transcript/clear`).
    async fn delete_for_conversation(&self, conversation_id: u64) -> Result<u64, AppError>;

    // ── Admin / audit-dashboard reads ──────────────────────────────────────────

    /// Page over audits for a user, newest first.
    async fn find_by_user_id_paginated(
        &self,
        user_id: u64,
        limit: u64,
        offset: u64,
    ) -> Result<(Vec<PostConversationRiskAudit>, u64), AppError>;

    /// All audits for a conversation (newest first).
    async fn find_by_conversation_id(
        &self,
        conversation_id: u64,
    ) -> Result<Vec<PostConversationRiskAudit>, AppError>;

    /// Page over all audits, optionally filtered by risk level.
    async fn find_all_paginated(
        &self,
        limit: u64,
        offset: u64,
        risk_level: Option<&str>,
    ) -> Result<(Vec<PostConversationRiskAudit>, u64), AppError>;

    /// Distinct conversation ids that have at least one audit (optionally
    /// filtered by risk level), newest-audit-first. Used by the admin
    /// "risk conversations" list.
    async fn find_conversation_ids_paginated(
        &self,
        limit: u64,
        offset: u64,
        risk_level: Option<&str>,
    ) -> Result<(Vec<u64>, u64), AppError>;
}
