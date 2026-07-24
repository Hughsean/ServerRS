use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ConnectionEndReason, ConnectionEpochId, InboundMessageEnvelope, IngestionGapId,
    IngestionGapReason, SourceAccountRef,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceEventId(String);

impl SourceEventId {
    pub fn new(value: impl Into<String>) -> Result<Self, InboundEventStoreError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(InboundEventStoreError::InvalidData(
                "source event id must not be empty".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestMessageOutcome {
    Accepted {
        source_event_id: SourceEventId,
        reply_to_event_id: Option<SourceEventId>,
    },
    Duplicate {
        source_event_id: SourceEventId,
    },
}

impl IngestMessageOutcome {
    pub fn source_event_id(&self) -> &SourceEventId {
        match self {
            Self::Accepted {
                source_event_id, ..
            }
            | Self::Duplicate { source_event_id } => source_event_id,
        }
    }
}

#[derive(Debug, Error)]
pub enum InboundEventStoreError {
    #[error("invalid inbound event data: {0}")]
    InvalidData(String),
    #[error("inbound event store is unavailable")]
    Unavailable,
    #[error("inbound event database operation failed: {0}")]
    Database(String),
    #[error("backfill lease ownership was lost")]
    LeaseLost,
}

#[async_trait]
pub trait InboundEventStoreT: Send + Sync {
    /// 原子地保存消息及其结构化内容；重复投递必须返回已有事件 ID。
    async fn insert_message_if_absent(
        &self,
        message: &InboundMessageEnvelope,
    ) -> Result<IngestMessageOutcome, InboundEventStoreError>;
}

#[async_trait]
pub trait IngestionContinuityStoreT: Send + Sync {
    /// 在发起传输连接前创建周期，确保连接失败本身也可观察。
    async fn begin_connection(
        &self,
        account: &SourceAccountRef,
    ) -> Result<ConnectionEpochId, InboundEventStoreError>;

    /// WebSocket 握手成功后标记周期已连接，并结束上一个开放空窗的时间范围。
    async fn mark_connection_connected(
        &self,
        connection_epoch_id: &ConnectionEpochId,
    ) -> Result<(), InboundEventStoreError>;

    /// 结束连接周期。曾成功连接的周期会产生 `uncertain` 空窗，等待历史回补验证。
    async fn finish_connection(
        &self,
        connection_epoch_id: &ConnectionEpochId,
        reason: ConnectionEndReason,
    ) -> Result<Option<IngestionGapId>, InboundEventStoreError>;

    /// 在连接仍存活但已经无法证明事件连续性时，幂等创建该周期的开放空窗。
    async fn mark_connection_uncertain(
        &self,
        connection_epoch_id: &ConnectionEpochId,
        reason: IngestionGapReason,
    ) -> Result<IngestionGapId, InboundEventStoreError>;
}

pub trait PersonalSecretaryStoreT: InboundEventStoreT + IngestionContinuityStoreT {}

impl<T> PersonalSecretaryStoreT for T where T: InboundEventStoreT + IngestionContinuityStoreT {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerBinding {
    pub managed_account: SourceAccountRef,
    pub command_account: SourceAccountRef,
    pub owner_actor_id: String,
}

#[async_trait]
pub trait OwnerBindingStoreT: Send + Sync {
    async fn ensure_owner_binding(
        &self,
        binding: &OwnerBinding,
    ) -> Result<(), InboundEventStoreError>;
}
