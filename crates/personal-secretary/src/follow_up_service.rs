use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    ClaimedOwnerNotification, FollowUpScanReport, InboundEventStoreError, MemoryStoreT,
    NotificationFailureKind, NotificationId, NotificationLeaseToken,
};

#[async_trait]
pub trait FollowUpStoreT: Send + Sync {
    async fn scan_commitments(
        &self,
        now_unix_secs: i64,
        horizon_secs: i64,
        limit: u32,
    ) -> Result<FollowUpScanReport, InboundEventStoreError>;

    async fn scan_response_expectations(
        &self,
        now_unix_secs: i64,
        horizon_secs: i64,
        response_timeout_secs: i64,
        limit: u32,
    ) -> Result<FollowUpScanReport, InboundEventStoreError>;

    async fn scan_project_blockers(
        &self,
        now_unix_secs: i64,
        horizon_secs: i64,
        blocker_escalation_secs: i64,
        limit: u32,
    ) -> Result<FollowUpScanReport, InboundEventStoreError>;

    async fn claim_due_notification(
        &self,
        account: &crate::SourceAccountRef,
        now_unix_secs: i64,
        lease_secs: u64,
    ) -> Result<Option<ClaimedOwnerNotification>, InboundEventStoreError>;

    async fn mark_notification_delivered(
        &self,
        notification_id: &NotificationId,
        lease_token: &NotificationLeaseToken,
        platform_message_id: &str,
    ) -> Result<(), InboundEventStoreError>;

    async fn mark_notification_failed(
        &self,
        notification_id: &NotificationId,
        lease_token: &NotificationLeaseToken,
        error_code: &str,
        kind: NotificationFailureKind,
    ) -> Result<(), InboundEventStoreError>;

    /// 启动屏障：在任何平台投递任务启动前，同步处理历史来源直写 Outbox。
    ///
    /// 活跃 `claimed` 行不可安全改写，必须返回阻塞错误；过期租约会终态化为
    /// `unknown_commit`。本方法不会从旧 payload 推断或重建候选。
    async fn reconcile_legacy_notifications(
        &self,
        config: &LegacyNotificationReconciliationConfig,
    ) -> Result<LegacyNotificationReconciliationReport, InboundEventStoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyNotificationReconciliationConfig {
    pub worker_id: String,
    pub lease_secs: u64,
    pub page_size: u32,
    pub max_rows: u32,
    pub deadline_secs: u64,
}

impl LegacyNotificationReconciliationConfig {
    pub fn validate(&self) -> Result<(), InboundEventStoreError> {
        if self.worker_id.trim().is_empty() || self.worker_id.len() > 128 {
            return Err(InboundEventStoreError::InvalidData(
                "legacy reconciliation worker_id must be non-empty and at most 128 bytes".into(),
            ));
        }
        if !(1..=3600).contains(&self.lease_secs)
            || !(1..=1000).contains(&self.page_size)
            || !(1..=100_000).contains(&self.max_rows)
            || !(1..=300).contains(&self.deadline_secs)
        {
            return Err(InboundEventStoreError::InvalidData(
                "legacy reconciliation bounds are invalid".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LegacyNotificationReconciliationReport {
    pub rows_scanned: u64,
    pub legacy_outbox_suppressed: u64,
    pub legacy_sources_rebuilt: u64,
    pub legacy_sources_unverifiable: u64,
    pub candidates_created: u64,
    pub requests_created: u64,
    pub sources_skipped_stale: u64,
    pub active_claimed: u64,
    pub expired_claims_marked_unknown_commit: u64,
    pub completed: bool,
    pub blocked: bool,
}

pub struct FollowUpUseCase {
    follow_ups: Arc<dyn FollowUpStoreT>,
    memories: Arc<dyn MemoryStoreT>,
}

impl FollowUpUseCase {
    pub fn new(follow_ups: Arc<dyn FollowUpStoreT>, memories: Arc<dyn MemoryStoreT>) -> Self {
        Self {
            follow_ups,
            memories,
        }
    }

    pub async fn scan(
        &self,
        now_unix_secs: i64,
        horizon_secs: i64,
        response_timeout_secs: i64,
        blocker_escalation_secs: i64,
        limit: u32,
    ) -> Result<FollowUpScanReport, InboundEventStoreError> {
        if !(60..=31_536_000).contains(&horizon_secs)
            || !(300..=2_592_000).contains(&response_timeout_secs)
            || !(3_600..=31_536_000).contains(&blocker_escalation_secs)
            || !(1..=1000).contains(&limit)
        {
            return Err(InboundEventStoreError::InvalidData(
                "follow-up horizon/response timeout/limit are outside safe bounds".into(),
            ));
        }
        let memories_expired = self.memories.expire_due(now_unix_secs, limit).await?;
        let mut report = self
            .follow_ups
            .scan_commitments(now_unix_secs, horizon_secs, limit)
            .await?;
        let response_report = self
            .follow_ups
            .scan_response_expectations(now_unix_secs, horizon_secs, response_timeout_secs, limit)
            .await?;
        report.response_expectations_materialized =
            response_report.response_expectations_materialized;
        report.response_expectations_resolved = response_report.response_expectations_resolved;
        report.notification_candidates_created = report
            .notification_candidates_created
            .saturating_add(response_report.notification_candidates_created);
        report.notification_evaluation_requests_created = report
            .notification_evaluation_requests_created
            .saturating_add(response_report.notification_evaluation_requests_created);
        let blocker_report = self
            .follow_ups
            .scan_project_blockers(now_unix_secs, horizon_secs, blocker_escalation_secs, limit)
            .await?;
        report.project_blockers_materialized = blocker_report.project_blockers_materialized;
        report.notification_candidates_created = report
            .notification_candidates_created
            .saturating_add(blocker_report.notification_candidates_created);
        report.notification_evaluation_requests_created = report
            .notification_evaluation_requests_created
            .saturating_add(blocker_report.notification_evaluation_requests_created);
        report.memories_expired = memories_expired;
        Ok(report)
    }

    pub async fn reconcile_legacy_notifications(
        &self,
        config: &LegacyNotificationReconciliationConfig,
    ) -> Result<LegacyNotificationReconciliationReport, InboundEventStoreError> {
        config.validate()?;
        self.follow_ups.reconcile_legacy_notifications(config).await
    }

    pub async fn claim_due_notification(
        &self,
        account: &crate::SourceAccountRef,
        now_unix_secs: i64,
        lease_secs: u64,
    ) -> Result<Option<ClaimedOwnerNotification>, InboundEventStoreError> {
        if !(1..=3600).contains(&lease_secs) {
            return Err(InboundEventStoreError::InvalidData(
                "notification lease_secs must be in 1..=3600".into(),
            ));
        }
        self.follow_ups
            .claim_due_notification(account, now_unix_secs, lease_secs)
            .await
    }

    pub async fn mark_notification_delivered(
        &self,
        notification_id: &NotificationId,
        lease_token: &NotificationLeaseToken,
        platform_message_id: &str,
    ) -> Result<(), InboundEventStoreError> {
        if platform_message_id.trim().is_empty() || platform_message_id.len() > 512 {
            return Err(InboundEventStoreError::InvalidData(
                "platform message id must contain 1..=512 bytes".into(),
            ));
        }
        self.follow_ups
            .mark_notification_delivered(notification_id, lease_token, platform_message_id)
            .await
    }

    pub async fn mark_notification_failed(
        &self,
        notification_id: &NotificationId,
        lease_token: &NotificationLeaseToken,
        error_code: &str,
        kind: NotificationFailureKind,
    ) -> Result<(), InboundEventStoreError> {
        if error_code.trim().is_empty() || error_code.len() > 64 {
            return Err(InboundEventStoreError::InvalidData(
                "notification error code must contain 1..=64 bytes".into(),
            ));
        }
        self.follow_ups
            .mark_notification_failed(notification_id, lease_token, error_code, kind)
            .await
    }
}
