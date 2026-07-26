//! ActionStore 端口、运行标识、租约与错误分类（约束 3/8）。
//!
//! 基础设施层实现 `ActionStoreT`；领域层定义边界。CAS + lease fencing、
//! `take_checkpoint` 单次消费与 `UnknownCommit` 精确分类均在此约束。

use async_trait::async_trait;
use thiserror::Error;

use crate::{
    OwnerResponseDraft, RecentEventRef, SecretaryActionEffect, SecretaryActionReceipt,
    SourceAccountRef, SourceEventId,
};

/// 创建 action_run 所需的全部种子数据，封装为结构体避免参数过多。
#[derive(Debug, Clone)]
pub struct ActionRunSeed {
    pub account: SourceAccountRef,
    pub command_source_event_id: SourceEventId,
    pub command_text: String,
    pub conversation_id: String,
    pub occurred_at_unix_secs: i64,
    pub timezone_offset_secs: i64,
    pub recent_events: Vec<RecentEventRef>,
}

/// 领取 suspended run 的完整 CAS 条件。恢复边界必须同时绑定运行、挂起点、
/// Proposal、原 OwnerCommand 与新租约参数，防止错配或旧审批重放。
#[derive(Debug, Clone)]
pub struct SuspendedRunClaim {
    pub run_id: ActionRunId,
    pub checkpoint_id: String,
    pub proposal_id: String,
    pub command_source_event_id: SourceEventId,
    pub worker_id: String,
    pub lease_secs: u64,
    pub now_unix_secs: i64,
}

/// Action 运行存储端口。基础设施层实现，领域层定义。
#[async_trait]
pub trait ActionStoreT: Send + Sync {
    /// 幂等创建 action_run（INSERT IGNORE）。同一 OwnerCommand 重复扫描只运行一次。
    /// 返回是否新建（true=新建，false=已存在）。
    async fn ensure_action_run(
        &self,
        run_id: &ActionRunId,
        seed: &ActionRunSeed,
    ) -> Result<bool, ActionStoreError>;

    /// 领取一个 pending 的 action_run（CAS）。返回领取的运行上下文或 None（无待处理）。
    async fn claim_pending_run(
        &self,
        worker_id: &str,
        lease_secs: u64,
        now_unix_secs: i64,
    ) -> Result<Option<ClaimedActionRun>, ActionStoreError>;

    /// CAS 领取一个等待 Owner 输入的 suspended run，并签发新的恢复租约。
    /// checkpoint_id 必须与 run 当前挂起点一致，防止旧审批恢复新状态。
    async fn claim_suspended_run(
        &self,
        claim: &SuspendedRunClaim,
    ) -> Result<Option<ClaimedActionRun>, ActionStoreError>;

    /// 将持有租约的 running run 标记为 suspended，并释放 Worker 租约。
    /// 完整 Graph Checkpoint 由绑定 run_id 的 CheckpointStore 持久化；这里仅保存索引摘要。
    async fn mark_suspended(
        &self,
        run_id: &ActionRunId,
        lease_token: &ActionLeaseToken,
        checkpoint_json: &str,
    ) -> Result<(), ActionStoreError>;

    /// 加载 Checkpoint。
    async fn load_checkpoint(
        &self,
        run_id: &ActionRunId,
    ) -> Result<Option<String>, ActionStoreError>;

    /// 单次消费 Checkpoint（resume 时调用，CAS 防并发双击）。
    async fn take_checkpoint(
        &self,
        run_id: &ActionRunId,
        lease_token: &ActionLeaseToken,
    ) -> Result<Option<String>, ActionStoreError>;

    /// 查询已提交的 Effect Receipt。Graph 重放时必须先查，避免重复执行真实动作。
    async fn load_effect_receipt(
        &self,
        run_id: &ActionRunId,
        effect_id: &str,
    ) -> Result<Option<SecretaryActionReceipt>, ActionStoreError>;

    /// 应用 Effect（幂等，用 effect_id 去重）。
    /// P0-3 修复：显式传入 run_id，避免误用 proposal_id 作为 run_id。
    async fn apply_effect(
        &self,
        run_id: &ActionRunId,
        effect: &SecretaryActionEffect,
        effect_id: &str,
        result_ref: &str,
        lease_token: &ActionLeaseToken,
    ) -> Result<SecretaryActionReceipt, ActionStoreError>;

    /// 标记运行完成。
    async fn mark_completed(
        &self,
        run_id: &ActionRunId,
        lease_token: &ActionLeaseToken,
        response_draft: Option<&OwnerResponseDraft>,
    ) -> Result<(), ActionStoreError>;

    /// 标记运行失败并设置退避。
    async fn mark_failed(
        &self,
        run_id: &ActionRunId,
        lease_token: &ActionLeaseToken,
        error: &str,
        next_eligible_at_unix_secs: i64,
    ) -> Result<(), ActionStoreError>;

    /// 释放租约（不标记完成/失败，让其他 Worker 可重新领取）。
    async fn release_lease(
        &self,
        run_id: &ActionRunId,
        lease_token: &ActionLeaseToken,
    ) -> Result<(), ActionStoreError>;

    /// 记录审计事件。
    async fn append_audit(
        &self,
        run_id: &ActionRunId,
        event_kind: &str,
        detail_json: &str,
    ) -> Result<(), ActionStoreError>;
}

// ===== 运行标识与租约 =====

macro_rules! action_id {
    ($name:ident, $field:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ActionStoreError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(ActionStoreError::InvalidData(format!(
                        "{} must not be empty",
                        $field
                    )));
                }
                if value.len() > 36 {
                    return Err(ActionStoreError::InvalidData(format!(
                        "{} must not exceed 36 bytes",
                        $field
                    )));
                }
                Ok(Self(value))
            }

            pub fn generate() -> Self {
                Self(uuid::Uuid::new_v4().to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

action_id!(ActionRunId, "action_run_id");
action_id!(ActionLeaseToken, "action_lease_token");

impl ActionRunId {
    /// 从 OwnerCommand 的不可变事件 ID 与 Planner 版本生成稳定的 36 字符 UUID。
    /// 既避免重扫时重复创建，也不会超过数据库 CHAR(36) 边界。
    pub fn for_owner_command(source_event_id: &SourceEventId, planner_version: &str) -> Self {
        let name = format!("{}:{planner_version}", source_event_id.as_str());
        Self(uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, name.as_bytes()).to_string())
    }
}

/// 领取到的 action_run 运行上下文。
#[derive(Debug, Clone)]
pub struct ClaimedActionRun {
    pub run_id: ActionRunId,
    pub lease_token: ActionLeaseToken,
    pub account: SourceAccountRef,
    pub command_source_event_id: SourceEventId,
    pub command_text: String,
    pub conversation_id: String,
    pub occurred_at_unix_secs: i64,
    pub timezone_offset_secs: i64,
    pub recent_events: Vec<RecentEventRef>,
}

// ===== 错误类型（约束 8：错误分类不能全映射为 UnknownCommit）=====

#[derive(Debug, Error)]
pub enum ActionStoreError {
    #[error("invalid action data: {0}")]
    InvalidData(String),
    #[error("action store is unavailable")]
    Unavailable,
    #[error("action database operation failed: {0}")]
    Database(String),
    #[error("action lease ownership was lost")]
    LeaseLost,
    /// 约束 8：只有"可能已提交但没拿到结果"才是 UnknownCommit。
    #[error("action effect may have been committed but result is unknown: {0}")]
    UnknownCommit(String),
}

impl From<crate::InboundEventStoreError> for ActionStoreError {
    fn from(error: crate::InboundEventStoreError) -> Self {
        match error {
            crate::InboundEventStoreError::InvalidData(msg) => Self::InvalidData(msg),
            crate::InboundEventStoreError::Unavailable => Self::Unavailable,
            crate::InboundEventStoreError::Database(msg) => Self::Database(msg),
            crate::InboundEventStoreError::LeaseLost => Self::LeaseLost,
        }
    }
}

impl ActionStoreError {
    /// 把存储错误映射为 Effect 错误分类（约束 8）。
    #[allow(dead_code)]
    pub fn to_effect_error(self) -> agent_core::graph::EffectError {
        use agent_core::graph::{EffectError, EffectErrorKind};
        match self {
            Self::InvalidData(_) | Self::LeaseLost => {
                EffectError::with_source(EffectErrorKind::Permanent, self)
            }
            Self::Unavailable | Self::Database(_) => {
                EffectError::with_source(EffectErrorKind::UnknownCommit, self)
            }
            Self::UnknownCommit(_) => {
                EffectError::with_source(EffectErrorKind::UnknownCommit, self)
            }
        }
    }
}

// ===== 运行时上下文（注入节点，不污染 AgentState）=====

/// 一次 Action 运行的上下文，由 Worker 领取后注入 PlanNode。
/// 不存入 AgentState，避免状态机膨胀（约束 2）。
#[derive(Debug, Clone)]
pub struct ActionRunContext {
    pub account: SourceAccountRef,
    pub command_source_event_id: SourceEventId,
    pub command_text: String,
    pub conversation_id: String,
    pub occurred_at_unix_secs: i64,
    pub timezone_offset_secs: i64,
    pub now_unix_secs: i64,
    pub lease_token: ActionLeaseToken,
}
