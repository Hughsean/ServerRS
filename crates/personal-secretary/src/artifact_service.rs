//! Artifact 引用应用层：端口与用例编排。
//!
//! 本模块只依赖领域对象（[`crate::artifact`]）和抽象端口，不依赖 NapCat、SeaORM、
//! MySQL 或 `qqbot-server`。
//!
//! 用例职责：
//! 1. 创建 Artifact 信封（不自动下载）；
//! 2. 按 source_event_id 按需检索 Artifact；
//! 3. 撤回/TTL/Owner 删除/内容策略变化传播失效；
//! 4. `never_long_term` 不生成长期 Artifact；`envelope_only` 只保存最小信封。

use std::sync::Arc;

use async_trait::async_trait;

use crate::{ArtifactEnvelope, ArtifactError, ArtifactId, SourceAccountRef, SourceEventId};

/// Artifact 存储端口：基础设施层（MySQL）实现。
#[async_trait]
pub trait ArtifactStoreT: Send + Sync {
    /// 创建 Artifact 信封（幂等：相同 artifact_id 不重复写入）。
    async fn create_artifact(&self, envelope: &ArtifactEnvelope) -> Result<(), ArtifactStoreError>;

    /// 读取单个 Artifact 信封。
    async fn load_artifact(
        &self,
        artifact_id: &ArtifactId,
    ) -> Result<Option<ArtifactEnvelope>, ArtifactStoreError>;

    /// 按 source_event_id 检索所有可用 Artifact（严格账号隔离）。
    async fn list_for_event(
        &self,
        account: &SourceAccountRef,
        source_event_id: &SourceEventId,
    ) -> Result<Vec<ArtifactEnvelope>, ArtifactStoreError>;

    /// 标记某 source_event_id 的所有 Artifact 为已撤回（B3 传播）。
    async fn invalidate_for_recall(
        &self,
        source_event_id: &SourceEventId,
    ) -> Result<u64, ArtifactStoreError>;

    /// 从消息事务创建的持久化任务派生 Artifact。返回完成任务数。
    async fn derive_pending(
        &self,
        default_ttl_secs: u64,
        batch_size: u64,
    ) -> Result<u64, ArtifactStoreError>;

    /// 标记已过期的 Artifact（TTL 到期）。返回受影响行数。
    async fn expire_due(&self, now_unix_secs: i64) -> Result<u64, ArtifactStoreError>;
}

/// Artifact 存储错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum ArtifactStoreError {
    #[error("artifact store invalid data: {0}")]
    InvalidData(String),
    #[error("artifact store unavailable: {0}")]
    Unavailable(String),
    #[error("artifact store database error: {0}")]
    Database(String),
}

impl From<sea_orm::DbErr> for ArtifactStoreError {
    fn from(err: sea_orm::DbErr) -> Self {
        Self::Database(err.to_string())
    }
}

/// Artifact 用例。协议无关。
pub struct ArtifactUseCase {
    store: Arc<dyn ArtifactStoreT>,
}

impl ArtifactUseCase {
    pub fn new(store: Arc<dyn ArtifactStoreT>) -> Self {
        Self { store }
    }

    /// 创建 Artifact 信封。
    ///
    /// `never_long_term` 不生成长期 Artifact；`envelope_only` 只保存最小信封。
    /// 不自动下载；URL 不写日志。
    pub async fn create(&self, envelope: &ArtifactEnvelope) -> Result<(), ArtifactError> {
        self.store
            .create_artifact(envelope)
            .await
            .map_err(|e| ArtifactError::Store(e.to_string()))
    }

    /// 按 source_event_id 按需检索可用 Artifact（严格账号隔离）。
    pub async fn list_for_event(
        &self,
        account: &SourceAccountRef,
        source_event_id: &SourceEventId,
    ) -> Result<Vec<ArtifactEnvelope>, ArtifactError> {
        self.store
            .list_for_event(account, source_event_id)
            .await
            .map_err(|e| ArtifactError::Store(e.to_string()))
    }

    /// 撤回失效传播：标记某 source_event_id 的所有 Artifact 为已撤回。
    pub async fn invalidate_for_recall(
        &self,
        source_event_id: &SourceEventId,
    ) -> Result<u64, ArtifactError> {
        self.store
            .invalidate_for_recall(source_event_id)
            .await
            .map_err(|e| ArtifactError::Store(e.to_string()))
    }

    pub async fn derive_pending(
        &self,
        default_ttl_secs: u64,
        batch_size: u64,
    ) -> Result<u64, ArtifactError> {
        self.store
            .derive_pending(default_ttl_secs, batch_size)
            .await
            .map_err(|e| ArtifactError::Store(e.to_string()))
    }

    /// TTL 到期：把到期 Artifact 标记为 expired。由后台 Worker 周期调用。
    pub async fn expire_due(&self, now_unix_secs: i64) -> Result<u64, ArtifactError> {
        self.store
            .expire_due(now_unix_secs)
            .await
            .map_err(|e| ArtifactError::Store(e.to_string()))
    }
}
