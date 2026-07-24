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
        limit: u32,
    ) -> Result<FollowUpScanReport, InboundEventStoreError> {
        if !(60..=31_536_000).contains(&horizon_secs) || !(1..=1000).contains(&limit) {
            return Err(InboundEventStoreError::InvalidData(
                "follow-up horizon must be 60..=31536000 seconds and limit 1..=1000".into(),
            ));
        }
        let memories_expired = self.memories.expire_due(now_unix_secs, limit).await?;
        let mut report = self
            .follow_ups
            .scan_commitments(now_unix_secs, horizon_secs, limit)
            .await?;
        report.memories_expired = memories_expired;
        Ok(report)
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
