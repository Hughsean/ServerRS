//! 延迟 Reply 后台修复用例（Codex 复核 P1-1）。
//!
//! 主实时路径（父事件重放/回补）解析 pending Reply 是**最好情况**：若父事件永不
//! 重放，unresolved 子事件会永久留在待解析状态。本用例提供必然重试：以有界批次
//! 领取 unresolved 子事件（租约 + SKIP LOCKED + 指数退避，跨重启安全），逐个在
//! 事务内重试解析；命中父事件时走与主路径相同的回填与线程投影失效逻辑。
//!
//! 协议无关：Worker 只调用 [`ReconcilePendingRepliesUseCase::run_one`]，批次大小、
//! 租约秒数与退避参数由外层配置注入。

use std::sync::Arc;

use async_trait::async_trait;

use crate::{ConversationRef, InboundEventStoreError, SourceAccountRef, SourceEventId};

/// 一个已领取的 unresolved 子事件候选。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedPendingReply {
    pub source_event_id: SourceEventId,
    /// 子事件的账号作用域（父事件必须同账号、同通道、同会话）。
    pub account: SourceAccountRef,
    pub conversation: ConversationRef,
    pub reply_to_platform_message_id: String,
    /// 租约令牌（fencing）：处理时的完成/退避写入必须以本令牌 + 未过期作为
    /// 条件并检查影响行数，防止旧 Worker 覆盖新 Worker 的租约或写入。
    pub lease_token: String,
    /// 已尝试次数（来自退避簿），用于指数退避。
    pub attempts: u32,
}

/// 一次修复运行的预算。`retry_*` 同时约束退避下限与上限，防止热循环。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileBudget {
    pub batch_size: u32,
    pub lease_secs: u64,
    pub retry_initial_ms: u64,
    pub retry_max_ms: u64,
}

impl ReconcileBudget {
    pub fn new(batch_size: u32, lease_secs: u64, retry_initial_ms: u64, retry_max_ms: u64) -> Self {
        Self {
            batch_size,
            lease_secs,
            retry_initial_ms,
            retry_max_ms,
        }
    }
}

/// 一次修复运行的结果计数（仅类型化数量，不包含任何平台标识）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileRunOutcome {
    pub claimed: u32,
    pub resolved: u32,
    pub still_pending: u32,
}

/// 延迟 Reply 修复仓储端口：领取候选与单候选解析。
#[async_trait]
pub trait ReplyReconcileStoreT: Send + Sync {
    /// 原子领取至多 `limit` 个 unresolved 子事件：写入租约（fencing 令牌），
    /// 带 SKIP LOCKED 防并发领取。无候选时返回空 Vec。
    async fn claim_reconcile_batch(
        &self,
        lease_secs: u64,
        limit: u32,
    ) -> Result<Vec<ClaimedPendingReply>, InboundEventStoreError>;

    /// 处理一个已领取候选（内部短事务）：
    /// - 命中父事件：解析 pending 关系并失效旧线程投影（与主路径同一条事务逻辑），
    ///   清理已解析候选的租约行，返回 `Ok(true)`；
    /// - 未命中：写入指数退避（`retry_initial_ms` 起倍增至 `retry_max_ms`）并释放
    ///   租约，返回 `Ok(false)`；
    /// - 候选已被其它路径解析：清理租约行，返回 `Ok(false)`。
    async fn resolve_claimed_pending_reply(
        &self,
        claimed: &ClaimedPendingReply,
        retry_initial_ms: u64,
        retry_max_ms: u64,
    ) -> Result<bool, InboundEventStoreError>;
}

/// 延迟 Reply 修复用例：领取一批、逐个解析、返回有界结果。
pub struct ReconcilePendingRepliesUseCase {
    store: Arc<dyn ReplyReconcileStoreT>,
    budget: ReconcileBudget,
}

impl ReconcilePendingRepliesUseCase {
    pub fn new(store: Arc<dyn ReplyReconcileStoreT>, budget: ReconcileBudget) -> Self {
        Self { store, budget }
    }

    /// 执行一轮修复：有界领取 + 逐个解析。任何单个候选失败都会中断本轮并返回
    /// 错误（错误记录在仓储的 last_error 与退避状态，下一轮按退避继续）。
    pub async fn run_one(&self) -> Result<ReconcileRunOutcome, InboundEventStoreError> {
        let claimed = self
            .store
            .claim_reconcile_batch(self.budget.lease_secs, self.budget.batch_size)
            .await?;
        let mut outcome = ReconcileRunOutcome {
            claimed: claimed.len() as u32,
            ..ReconcileRunOutcome::default()
        };
        for item in &claimed {
            if self
                .store
                .resolve_claimed_pending_reply(
                    item,
                    self.budget.retry_initial_ms,
                    self.budget.retry_max_ms,
                )
                .await?
            {
                outcome.resolved += 1;
            } else {
                outcome.still_pending += 1;
            }
        }
        Ok(outcome)
    }
}
