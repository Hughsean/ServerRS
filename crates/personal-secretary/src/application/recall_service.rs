//! 消息撤回应用层：端口与用例编排。
//!
//! 本模块只依赖领域对象（[`crate::recall`]）和抽象端口，不依赖 NapCat、SeaORM、
//! MySQL 或 `qqbot-server`。
//!
//! 用例职责：
//! 1. 记录撤回事件（撤回本身也是可审计 SourceEvent）；
//! 2. 尝试关联被撤回的原消息（`account_id + channel + conversation + platform_message_id`）；
//! 3. 撤回先到时保存 pending tombstone；原消息后到后自动关联、失效且幂等；
//! 4. 重复撤回幂等；
//! 5. 失效传播到派生状态（由外层调用方实现具体的失效传播）。

use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    ClaimedRecallEvent, InboundEventStoreError, RecallCorrelationKey, RecallError, RecallEvent,
    RecallFailureKind, TombstoneRecord, TombstoneStatus,
};

/// 撤回存储端口：基础设施层（MySQL）实现。
#[async_trait]
pub trait RecallStoreT: Send + Sync {
    /// 记录撤回事件并尝试关联原消息。
    ///
    /// 返回 tombstone 状态：
    /// - `Pending`：撤回先到，原消息尚未到达。
    /// - `Applied`：原消息已存在，已标记为被撤回。
    /// - `IdempotentReapply`：相同撤回再次到达，幂等返回已应用。
    async fn record_recall(
        &self,
        recall: &RecallEvent,
    ) -> Result<TombstoneStatus, RecallStoreError>;

    /// 当原消息后到时，尝试关联 pending tombstone。
    ///
    /// 如果存在匹配的 pending tombstone，自动关联并失效；幂等。
    /// 如果不存在 pending tombstone（撤回未先到），返回 `None`。
    async fn apply_pending_tombstone(
        &self,
        correlation: &RecallCorrelationKey,
        source_event_id: &str,
    ) -> Result<Option<TombstoneRecord>, RecallStoreError>;

    /// 读取某关联键的 pending tombstone 列表（用于测试和审计）。
    async fn list_pending_for_correlation(
        &self,
        correlation: &RecallCorrelationKey,
    ) -> Result<Vec<TombstoneRecord>, RecallStoreError>;

    /// 某原消息是否已被撤回（用于 Retriever 过滤和来源回读）。
    async fn is_recalled(
        &self,
        account_id: u64,
        source_event_id: &str,
    ) -> Result<bool, RecallStoreError>;

    /// 读取某账号所有已撤回的 SourceEvent ID 列表（用于批量失效传播）。
    async fn list_recalled_event_ids(
        &self,
        account_id: u64,
    ) -> Result<Vec<String>, RecallStoreError>;

    /// 持久化接收撤回通知。相同 recall_event_id 或关联键幂等。
    async fn enqueue_recall(&self, recall: &RecallEvent) -> Result<(), RecallStoreError>;

    /// 领取一条到期撤回任务，并持久化 lease。过期 lease 会先回收到 pending。
    async fn claim_recall(
        &self,
        lease_secs: u64,
    ) -> Result<Option<ClaimedRecallEvent>, RecallStoreError>;

    async fn mark_recall_applied(
        &self,
        recall_event_id: &str,
        lease_token: &str,
    ) -> Result<(), RecallStoreError>;

    async fn mark_recall_failed(
        &self,
        recall_event_id: &str,
        lease_token: &str,
        error_code: &str,
        kind: RecallFailureKind,
    ) -> Result<(), RecallStoreError>;
}

/// 撤回存储错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum RecallStoreError {
    #[error("recall store invalid data: {0}")]
    InvalidData(String),
    #[error("recall store unavailable: {0}")]
    Unavailable(String),
    #[error("recall store database error: {0}")]
    Database(String),
}

impl From<InboundEventStoreError> for RecallStoreError {
    fn from(err: InboundEventStoreError) -> Self {
        match err {
            InboundEventStoreError::InvalidData(s) => Self::InvalidData(s),
            InboundEventStoreError::Unavailable => {
                Self::Unavailable("inbound event store unavailable".into())
            }
            InboundEventStoreError::Database(s) => Self::Database(s),
            InboundEventStoreError::LeaseLost => Self::Database("lease lost".into()),
        }
    }
}

/// 撤回用例。协议无关，由外层 Worker 或 handler 驱动。
pub struct RecallUseCase {
    store: Arc<dyn RecallStoreT>,
}

impl RecallUseCase {
    pub fn new(store: Arc<dyn RecallStoreT>) -> Self {
        Self { store }
    }

    /// 处理一条撤回事件。
    ///
    /// 撤回先到时保存 pending tombstone；原消息后到后自动关联、失效且幂等。
    /// 重复撤回返回 `IdempotentReapply`。
    pub async fn handle_recall(
        &self,
        recall: &RecallEvent,
    ) -> Result<TombstoneStatus, RecallError> {
        self.store
            .record_recall(recall)
            .await
            .map_err(map_store_error)
    }

    pub async fn enqueue(&self, recall: &RecallEvent) -> Result<(), RecallError> {
        self.store
            .enqueue_recall(recall)
            .await
            .map_err(map_store_error)
    }

    pub async fn claim(&self, lease_secs: u64) -> Result<Option<ClaimedRecallEvent>, RecallError> {
        self.store
            .claim_recall(lease_secs)
            .await
            .map_err(map_store_error)
    }

    pub async fn mark_applied(
        &self,
        recall_event_id: &str,
        lease_token: &str,
    ) -> Result<(), RecallError> {
        self.store
            .mark_recall_applied(recall_event_id, lease_token)
            .await
            .map_err(map_store_error)
    }

    pub async fn mark_failed(
        &self,
        recall_event_id: &str,
        lease_token: &str,
        error_code: &str,
        kind: RecallFailureKind,
    ) -> Result<(), RecallError> {
        self.store
            .mark_recall_failed(recall_event_id, lease_token, error_code, kind)
            .await
            .map_err(map_store_error)
    }

    /// 当原消息入库后，尝试关联 pending tombstone。
    ///
    /// 如果撤回先到（存在 pending tombstone），自动关联并失效。
    /// 如果撤回未先到，返回 `None`（撤回后到时由 `handle_recall` 直接关联）。
    pub async fn on_message_ingested(
        &self,
        correlation: &RecallCorrelationKey,
        source_event_id: &str,
    ) -> Result<Option<TombstoneRecord>, RecallError> {
        self.store
            .apply_pending_tombstone(correlation, source_event_id)
            .await
            .map_err(map_store_error)
    }

    /// 检查某原消息是否已被撤回。
    pub async fn is_recalled(
        &self,
        account_id: u64,
        source_event_id: &str,
    ) -> Result<bool, RecallError> {
        self.store
            .is_recalled(account_id, source_event_id)
            .await
            .map_err(map_store_error)
    }
}

fn map_store_error(error: RecallStoreError) -> RecallError {
    match error {
        RecallStoreError::InvalidData(message) => RecallError::InvalidIdentity(message),
        RecallStoreError::Unavailable(message) | RecallStoreError::Database(message) => {
            RecallError::Store(message)
        }
    }
}
