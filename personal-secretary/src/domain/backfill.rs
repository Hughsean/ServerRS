//! 协议无关的历史回补领域模型。
//!
//! 本模块只描述 Gap 回补的 Scope、Cursor、预算、证据、状态机和完整性判定，
//! 不依赖 NapCat、OneBot、QQ、SeaORM、MySQL、Axum、Tokio 或任何 HTTP 客户端。
//! 协议适配和持久化由外层实现应用层定义的端口（见 [`crate::backfill_service`]）。
//!
//! 核心不变量：传输重连成功不等于历史已补齐。只有存在充分证据时，Gap 才能从
//! `backfilling` 转为 `verified_complete`；真实 NapCat 通常只能证明“已知会话 Scope
//! 已回补”，账号级会话集合不可证完整时，Gap 必须保持 `uncertain`。

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ConnectionEpochId, IngestionGapId, IngestionGapReason, IngestionGapStatus, SourceAccountRef,
};
use crate::{ConversationRef, InboundMessageEnvelope};

/// 一次回补运行的唯一标识。证据不足的 Gap 可在退避后创建新的运行，因此一个 Gap
/// 在生命周期内可以对应多次运行。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BackfillRunId(String);

impl BackfillRunId {
    pub fn new(value: impl Into<String>) -> Result<Self, BackfillError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(BackfillError::InvalidIdentity(
                "backfill_run_id must not be empty".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 回补运行的租约所有权令牌。
///
/// 每次首次领取或过期恢复都会生成新令牌。进度续租和终态提交必须携带当前令牌，
/// 防止旧 Worker 在租约过期、已被其它进程接管后恢复执行并覆盖新持有者的状态。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BackfillLeaseToken(String);

impl BackfillLeaseToken {
    pub fn new(value: impl Into<String>) -> Result<Self, BackfillError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(BackfillError::InvalidIdentity(
                "backfill lease token must not be empty".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 回补的会话 Scope：某个账号下某个会话的一段历史需要回补。
///
/// `boundary_cursor` 是空窗前该会话的稳定游标（最后已知消息），用于判定回补是否
/// 已回读到回补前的稳定边界。所有 Cursor 都绑定 [`SourceAccountRef`]，平台消息 ID
/// 不得跨账号解释。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillScope {
    pub account: SourceAccountRef,
    pub conversation: ConversationRef,
    pub boundary_cursor: Option<BackfillCursor>,
}

impl BackfillScope {
    /// 用于持久化和日志的稳定 Scope 键，格式为 `kind:id`，不包含账号主体。
    pub fn scope_key(&self) -> String {
        format!(
            "{}:{}",
            self.conversation.kind.as_str(),
            self.conversation.id
        )
    }
}

/// NapCat 消息 ID 是账号视角局部标识，回补锚点必须同时记录平台消息 ID 和 `message_seq`，
/// 并绑定账号主体。禁止用数值加减生成下一锚点。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BackfillAnchor {
    pub message_id: String,
    pub message_seq: String,
}

impl BackfillAnchor {
    pub fn new(message_id: impl Into<String>, message_seq: impl Into<String>) -> Self {
        Self {
            message_id: message_id.into(),
            message_seq: message_seq.into(),
        }
    }
}

/// 回补分页游标。账号主体参与相等性判定，避免两个账号的锚点被误判为同一位置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackfillCursor {
    pub account: SourceAccountRef,
    pub anchor: BackfillAnchor,
}

impl BackfillCursor {
    pub fn new(account: SourceAccountRef, anchor: BackfillAnchor) -> Self {
        Self { account, anchor }
    }
}

/// 回补有界预算。所有历史读取必须有明确上限，禁止无限循环或一次加载全部历史。
///
/// 业务不变量集中在 [`BackfillBudget::validate`]，配置层（`qqbot-server`）只负责
/// 从 TOML/环境变量填充该结构并调用校验。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackfillBudget {
    pub page_size: u32,
    pub max_pages_per_scope: u32,
    pub max_events_per_run: u32,
    pub max_concurrency: u32,
    /// 允许进入统一入站存储的最早事件时间（Unix 秒，含该秒）。`None` 只允许用于
    /// 禁用 Backfill 的配置或确定性测试来源。
    #[serde(default)]
    pub earliest_occurred_at_unix_secs: Option<i64>,
    pub lease_secs: u64,
    pub retry_initial_ms: u64,
    pub retry_max_ms: u64,
}

/// 历史回补的唯一读取方向：从最新位置向更旧消息翻页。
///
/// 该类型是协议无关的领域契约；OneBot/NapCat 如何表达这个方向由外层私有映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackfillReadDirection {
    NewestToOldest,
}

/// 单页之后的继续证据。三种状态互斥，不再用 `Option<Cursor>` 混淆起点与无证据停止。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillContinuation {
    /// 存在由当前页真实锚点产生的稳定下一页游标。
    Next(BackfillCursor),
    /// 确定性来源明确证明已到历史起点。真实 NapCat 不得产生此证据。
    ProvenHistoryStart,
    /// 来源无法继续翻页，但也不能证明已到历史起点。
    UnprovenStop,
}

/// 单页历史读取结果。页内顺序固定为“新到旧”。
#[derive(Debug, Clone)]
pub struct BackfillPage {
    pub items: Vec<BackfillHistoryItem>,
    pub continuation: BackfillContinuation,
}

/// 协议无关的历史消息 DTO：携带统一信封和该消息在当前账号视角下的真实锚点。
#[derive(Debug, Clone)]
pub struct BackfillHistoryItem {
    pub envelope: InboundMessageEnvelope,
    pub anchor: BackfillAnchor,
}

/// 回补租约。崩溃后超时或失去租约的 `backfilling` 任务必须可以恢复。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackfillLease {
    pub lease_secs: u64,
}

impl BackfillLease {
    pub fn new(lease_secs: u64) -> Self {
        Self { lease_secs }
    }
}

/// 一次回补运行的持久化状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillRunStatus {
    Pending,
    Backfilling,
    VerifiedComplete,
    Unprovable,
    Unrecoverable,
}

impl BackfillRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Backfilling => "backfilling",
            Self::VerifiedComplete => "verified_complete",
            Self::Unprovable => "unprovable",
            Self::Unrecoverable => "unrecoverable",
        }
    }

    pub fn parse_from_str(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "backfilling" => Some(Self::Backfilling),
            "verified_complete" => Some(Self::VerifiedComplete),
            "unprovable" => Some(Self::Unprovable),
            "unrecoverable" => Some(Self::Unrecoverable),
            _ => None,
        }
    }
}

/// 单个 Scope 的回补状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillScopeStatus {
    Pending,
    Backfilling,
    VerifiedComplete,
    Unprovable,
    Unrecoverable,
}

impl BackfillScopeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Backfilling => "backfilling",
            Self::VerifiedComplete => "verified_complete",
            Self::Unprovable => "unprovable",
            Self::Unrecoverable => "unrecoverable",
        }
    }

    pub fn parse_from_str(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "backfilling" => Some(Self::Backfilling),
            "verified_complete" => Some(Self::VerifiedComplete),
            "unprovable" => Some(Self::Unprovable),
            "unrecoverable" => Some(Self::Unrecoverable),
            _ => None,
        }
    }
}

/// 回补过程中检测到的异常。任何异常都会阻止该 Scope 被标记为完整。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackfillAnomaly {
    /// 成功返回但无消息，无法区分无历史、缓存未加载、权限限制或后端暂时不可用。
    EmptyPage,
    /// 来源停止翻页，但未提供到达历史起点的证据。
    UnprovenStop,
    /// 本页所有锚点均已在上一页出现，分页未推进。
    DuplicatePage,
    /// 下一游标与当前游标相同，分页未推进。
    NoCursorAdvance,
    /// 当前或下一游标与 Scope 的账号主体不同。
    CursorAccountMismatch,
    /// 页内消息信封与 Scope 的账号或会话不同。
    MessageScopeMismatch,
    /// 消息或分页游标缺少稳定 `message_id`/`message_seq` 锚点。
    EmptyAnchor,
    /// 同一页内出现重复锚点。
    DuplicateAnchor,
    /// `Next` 游标不是当前“新到旧”页的最后一个真实锚点。
    InvalidContinuation,
    /// 上一页返回的锚点在本页消失，锚点链断裂。
    AnchorDisappeared,
    /// 排序方向或顺序与之前页面冲突，无法形成连续链。
    SortConflict,
    /// 冻结边界消息经幂等入口返回 `Accepted`，表明边界快照与存储状态不一致。
    BoundaryStateMismatch,
    /// 来源证明到达历史起点，但仍未命中冻结边界。
    BoundaryNotFound,
    /// 非确定性来源返回了“已到历史起点”证据。
    UntrustedHistoryStart,
    /// 来源只能表达请求方向，尚不能证明响应页确实按“新到旧”排序。
    UntrustedPageOrder,
    /// 协议错误（HTTP/OneBot retcode 非 0、解析失败等），按可恢复处理。
    ProtocolError { detail: String },
    /// 权限不足，无法读取该会话历史。
    PermissionDenied,
    /// 达到页数或事件数预算上限，无法继续证明连续性。
    BudgetExhausted,
    /// 已读到运维配置的历史时间下界；更旧消息不入库，且该 Gap 停止自动重扫。
    ConfiguredCutoffReached,
}

impl BackfillAnomaly {
    /// 权限错误视为该 Scope 永久不可恢复；其余异常视为暂时不可证明。
    pub fn is_unrecoverable(&self) -> bool {
        matches!(self, Self::PermissionDenied)
    }
}

/// 单个 Scope 的回补证据。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeEvidence {
    pub scope_key: String,
    pub pages_read: u32,
    pub events_read: u32,
    pub accepted: u32,
    pub duplicates: u32,
    pub anchor_chain: Vec<BackfillAnchor>,
    pub reached_boundary: bool,
    pub anomalies: Vec<BackfillAnomaly>,
}

impl ScopeEvidence {
    /// Scope 完整的判定：读取过至少一页、回读到稳定边界、且无任何异常。
    /// `anchor_chain` 不参与判定，使崩溃恢复后无需重建整条锚点链即可采纳已完成 Scope。
    pub fn is_complete(&self) -> bool {
        self.pages_read > 0 && self.reached_boundary && self.anomalies.is_empty()
    }

    /// 该 Scope 是否命中永久不可恢复异常。
    pub fn is_unrecoverable(&self) -> bool {
        self.anomalies.iter().any(BackfillAnomaly::is_unrecoverable)
    }
}

/// 一次回补运行的完整证据。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackfillEvidence {
    pub scopes: Vec<ScopeEvidence>,
    /// 历史来源是否能证明账号级会话集合完整。真实 NapCat 返回 false。
    pub account_conversation_set_proven: bool,
    /// 是否命中页数/事件数预算上限。
    pub budget_exhausted: bool,
}

impl BackfillEvidence {
    /// 所有已知 Scope 均完整（且至少存在一个 Scope）。
    pub fn all_scopes_complete(&self) -> bool {
        !self.scopes.is_empty() && self.scopes.iter().all(ScopeEvidence::is_complete)
    }

    /// 任意 Scope 命中永久不可恢复异常。
    pub fn any_unrecoverable(&self) -> bool {
        self.scopes.iter().any(ScopeEvidence::is_unrecoverable)
    }

    /// 任一 Scope 已命中配置的硬时间下界。继续自动重跑不会产生新证据，只会从最新页
    /// 重复读取，因此基础设施层必须挂起该 Gap。
    pub fn configured_cutoff_reached(&self) -> bool {
        self.scopes.iter().any(|scope| {
            scope
                .anomalies
                .contains(&BackfillAnomaly::ConfiguredCutoffReached)
        })
    }
}

/// Gap 回到 `uncertain` 后的再次领取策略。由 [`HistoryCompleteness::reclaim_policy`] 返回，
/// 基础设施层据此操作 `secretary_gap_reclaim_schedule` 表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReclaimPolicy {
    /// Gap 已达终态（`verified_complete`/`unrecoverable`），不再处于 `uncertain`。
    /// 删除 reclaim_schedule 行。
    Terminal,
    /// Gap 保持 `uncertain`，但 `secs` 秒内不可领取（设置 `next_eligible_at = now + secs`）。
    /// 用于暂时性证据不足，尽快重试。
    Backoff(u64),
    /// Gap 保持 `uncertain`，但设置极远未来的 `next_eligible_at`，停止自动重试。
    /// 用于 `KnownScopesComplete`：重跑无新证据，仅人工重验或能力升级后才重新排队。
    Suspended,
}

/// 历史完整性判定结果。直接决定 Gap 的目标状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryCompleteness {
    /// 所有已知 Scope 完整、无预算耗尽、且账号会话集合可证完整。
    /// 仅确定性 Fake 来源可达；真实 NapCat 无法达到。
    ProvenComplete,
    /// 所有已知 Scope 完整，但账号会话集合不可证完整（真实 NapCat 常态）。
    /// Gap 必须保持 `uncertain`。
    KnownScopesComplete,
    /// 证据不足：预算耗尽、锚点冲突、协议错误或空页歧义。Gap 保持 `uncertain`。
    Unprovable,
    /// 永久不可恢复（权限错误等）。Gap 转为 `unrecoverable`。
    Unrecoverable,
}

impl HistoryCompleteness {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProvenComplete => "proven_complete",
            Self::KnownScopesComplete => "known_scopes_complete",
            Self::Unprovable => "unprovable",
            Self::Unrecoverable => "unrecoverable",
        }
    }

    pub fn parse_from_str(value: &str) -> Option<Self> {
        match value {
            "proven_complete" => Some(Self::ProvenComplete),
            "known_scopes_complete" => Some(Self::KnownScopesComplete),
            "unprovable" => Some(Self::Unprovable),
            "unrecoverable" => Some(Self::Unrecoverable),
            _ => None,
        }
    }

    /// 根据证据推导完整性判定。
    pub fn from_evidence(evidence: &BackfillEvidence) -> Self {
        if evidence.any_unrecoverable() {
            return Self::Unrecoverable;
        }
        if evidence.budget_exhausted || !evidence.all_scopes_complete() {
            return Self::Unprovable;
        }
        if evidence.account_conversation_set_proven {
            Self::ProvenComplete
        } else {
            Self::KnownScopesComplete
        }
    }

    /// 该判定对应的 Gap 目标状态。
    pub fn gap_target_status(self) -> IngestionGapStatus {
        match self {
            Self::ProvenComplete => IngestionGapStatus::VerifiedComplete,
            Self::KnownScopesComplete | Self::Unprovable => IngestionGapStatus::Uncertain,
            Self::Unrecoverable => IngestionGapStatus::Unrecoverable,
        }
    }

    /// 该判定对应的 Gap reason（用于审计和 Owner 提示）。
    pub fn gap_reason(self) -> Option<IngestionGapReason> {
        match self {
            Self::KnownScopesComplete | Self::Unprovable => {
                Some(IngestionGapReason::HistoryUnprovable)
            }
            Self::ProvenComplete | Self::Unrecoverable => None,
        }
    }

    /// Gap 回到 `uncertain` 后的再次领取策略。
    ///
    /// 三种策略：
    /// - `Terminal`：Gap 已达终态（`verified_complete`/`unrecoverable`），不再处于
    ///   `uncertain`，删除 reclaim_schedule 行即可。
    /// - `Backoff(secs)`：Gap 保持 `uncertain`，但暂时不可领取，`secs` 秒后退避到期。
    ///   用于暂时性证据不足（`Unprovable`），尽快重试。
    /// - `Suspended`：Gap 保持 `uncertain`，但设置极远未来的 `next_eligible_at`，停止
    ///   自动重试。用于 `KnownScopesComplete`：所有已知 Scope 已回补，但账号会话集合
    ///   不可证；由于 Gap 边界在创建时已冻结，重跑不会获得新证据，只会重复读取相同
    ///   历史并产生新运行记录。仅人工重验或能力升级后才应重新排队。
    pub fn reclaim_policy(self) -> ReclaimPolicy {
        match self {
            Self::KnownScopesComplete => ReclaimPolicy::Suspended,
            Self::Unprovable => ReclaimPolicy::Backoff(30),
            Self::ProvenComplete | Self::Unrecoverable => ReclaimPolicy::Terminal,
        }
    }
}

/// 领域层非法状态转换错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GapTransitionError {
    #[error("cannot transition gap from {from} to {to}")]
    Illegal {
        from: IngestionGapStatus,
        to: IngestionGapStatus,
    },
    #[error("cannot complete a gap that was never claimed for backfill")]
    UnclaimedCannotComplete,
}

/// 校验 Gap 状态转换是否合法。领取必须 `uncertain -> backfilling`；只有充分证据才能
/// `backfilling -> verified_complete`；证据不足时 `backfilling -> uncertain`；永久失败
/// `backfilling -> unrecoverable`。未领取（`uncertain`）的任务不得直接完成或标记不可恢复。
pub fn validate_gap_transition(
    from: IngestionGapStatus,
    to: IngestionGapStatus,
) -> Result<(), GapTransitionError> {
    use IngestionGapStatus::*;
    let legal = matches!(
        (from, to),
        (Uncertain, Backfilling)
            | (Backfilling, VerifiedComplete)
            | (Backfilling, Uncertain)
            | (Backfilling, Unrecoverable)
            | (VerifiedComplete, VerifiedComplete)
            | (Unrecoverable, Unrecoverable)
    );
    if legal {
        Ok(())
    } else if from == Uncertain && matches!(to, VerifiedComplete | Unrecoverable) {
        Err(GapTransitionError::UnclaimedCannotComplete)
    } else {
        Err(GapTransitionError::Illegal { from, to })
    }
}

/// 领算校验错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BackfillConfigError {
    #[error("backfill.page_size must be between 1 and 100")]
    PageSizeOutOfRange,
    #[error("backfill.{0} must be positive")]
    NotPositive(&'static str),
    #[error("backfill.{0} exceeds the allowed upper bound")]
    TooLarge(&'static str),
    #[error("backfill.retry_max_ms must be >= retry_initial_ms")]
    RetryMaxBelowInitial,
    #[error("backfill.earliest_occurred_at_unix_secs must not be negative")]
    EarliestOccurredAtBeforeUnixEpoch,
}

impl BackfillBudget {
    /// 校验预算满足业务不变量。配置层构造完成后必须调用。
    pub fn validate(&self) -> Result<(), BackfillConfigError> {
        if self.page_size == 0 || self.page_size > 100 {
            return Err(BackfillConfigError::PageSizeOutOfRange);
        }
        if self
            .earliest_occurred_at_unix_secs
            .is_some_and(|value| value < 0)
        {
            return Err(BackfillConfigError::EarliestOccurredAtBeforeUnixEpoch);
        }
        check_positive(self.max_pages_per_scope, "max_pages_per_scope")?;
        check_positive(self.max_events_per_run, "max_events_per_run")?;
        check_positive(self.max_concurrency, "max_concurrency")?;
        check_positive(self.lease_secs, "lease_secs")?;
        check_positive(self.retry_initial_ms, "retry_initial_ms")?;
        check_positive(self.retry_max_ms, "retry_max_ms")?;
        if self.max_pages_per_scope > 10_000 {
            return Err(BackfillConfigError::TooLarge("max_pages_per_scope"));
        }
        if self.max_events_per_run > 1_000_000 {
            return Err(BackfillConfigError::TooLarge("max_events_per_run"));
        }
        if self.max_concurrency > 64 {
            return Err(BackfillConfigError::TooLarge("max_concurrency"));
        }
        if self.lease_secs > 3600 {
            return Err(BackfillConfigError::TooLarge("lease_secs"));
        }
        if self.retry_max_ms > 300_000 {
            return Err(BackfillConfigError::TooLarge("retry_max_ms"));
        }
        if self.retry_max_ms < self.retry_initial_ms {
            return Err(BackfillConfigError::RetryMaxBelowInitial);
        }
        Ok(())
    }
}

fn check_positive<T>(value: T, field: &'static str) -> Result<(), BackfillConfigError>
where
    T: PartialOrd + Default,
{
    if value <= T::default() {
        Err(BackfillConfigError::NotPositive(field))
    } else {
        Ok(())
    }
}

/// 已领取的 Gap，由状态仓储原子返回。
#[derive(Debug, Clone)]
pub struct ClaimedGap {
    pub run_id: BackfillRunId,
    /// 当前运行的租约所有权令牌。每次过期恢复都会轮换。
    pub lease_token: BackfillLeaseToken,
    pub gap_id: IngestionGapId,
    pub account: SourceAccountRef,
    pub connection_epoch_id: ConnectionEpochId,
    /// 是否为恢复因租约过期而滞留的运行（携带已有进度）。
    pub is_resume: bool,
}

/// 状态仓储返回的已知会话 Scope 及其空窗前稳定游标。
#[derive(Debug, Clone)]
pub struct KnownScope {
    pub conversation: ConversationRef,
    pub boundary_cursor: Option<BackfillCursor>,
}

/// 单个 Scope 的持久化进度，用于崩溃恢复。
#[derive(Debug, Clone)]
pub struct ScopeProgress {
    pub conversation: ConversationRef,
    pub status: BackfillScopeStatus,
    pub last_cursor: Option<BackfillCursor>,
    pub pages_read: u32,
    pub events_read: u32,
    pub accepted: u32,
    pub duplicates: u32,
    pub reached_boundary: bool,
    pub anomalies: Vec<BackfillAnomaly>,
}

/// 一次回补运行的持久化进度。
#[derive(Debug, Clone)]
pub struct BackfillRunProgress {
    pub run_id: BackfillRunId,
    pub gap_id: IngestionGapId,
    pub scopes: Vec<ScopeProgress>,
}

/// 一次回补运行的最终结果。
#[derive(Debug, Clone)]
pub struct BackfillOutcome {
    pub run_id: BackfillRunId,
    pub gap_id: IngestionGapId,
    pub completeness: HistoryCompleteness,
    pub evidence: BackfillEvidence,
    pub gap_target_status: IngestionGapStatus,
    pub gap_reason: Option<IngestionGapReason>,
    pub failure_class: Option<String>,
}

impl BackfillOutcome {
    /// 配置下界是确定性停止条件，命中后不允许按普通 `Unprovable` 的短退避重新扫描。
    pub fn reclaim_policy(&self) -> ReclaimPolicy {
        if self.evidence.configured_cutoff_reached() {
            ReclaimPolicy::Suspended
        } else {
            self.completeness.reclaim_policy()
        }
    }
}

/// 历史来源端口错误。外层 NapCat 适配器把协议/传输错误映射到本类型。
#[derive(Debug, Error)]
pub enum BackfillSourceError {
    #[error("history source is unavailable: {0}")]
    Unavailable(String),
    #[error("history source protocol error: {0}")]
    Protocol(String),
    #[error("history source permission denied for scope")]
    PermissionDenied,
}

/// 领域/应用层回补错误。
#[derive(Debug, Error)]
pub enum BackfillError {
    #[error("invalid backfill identity: {0}")]
    InvalidIdentity(String),
    #[error("backfill state store error: {0}")]
    State(#[from] crate::InboundEventStoreError),
    #[error("backfill history source error: {0}")]
    Source(#[from] BackfillSourceError),
    #[error("illegal gap transition: {0}")]
    IllegalTransition(#[from] GapTransitionError),
    #[error("backfill budget exhausted before reaching boundary")]
    BudgetExhausted,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IngestionGapStatus::*;

    fn budget() -> BackfillBudget {
        BackfillBudget {
            page_size: 50,
            max_pages_per_scope: 20,
            max_events_per_run: 2000,
            max_concurrency: 2,
            earliest_occurred_at_unix_secs: None,
            lease_secs: 60,
            retry_initial_ms: 500,
            retry_max_ms: 10_000,
        }
    }

    #[test]
    fn uncertain_to_backfilling_is_the_only_legal_claim() {
        assert!(validate_gap_transition(Uncertain, Backfilling).is_ok());
        assert!(validate_gap_transition(Backfilling, Uncertain).is_ok());
        assert!(validate_gap_transition(Backfilling, VerifiedComplete).is_ok());
        assert!(validate_gap_transition(Backfilling, Unrecoverable).is_ok());
    }

    #[test]
    fn unclaimed_gap_cannot_be_completed_directly() {
        let error = validate_gap_transition(Uncertain, VerifiedComplete).unwrap_err();
        assert_eq!(error, GapTransitionError::UnclaimedCannotComplete);
        let error = validate_gap_transition(Uncertain, Unrecoverable).unwrap_err();
        assert_eq!(error, GapTransitionError::UnclaimedCannotComplete);
    }

    #[test]
    fn terminal_to_terminal_is_idempotent_but_other_jumps_are_illegal() {
        assert!(validate_gap_transition(VerifiedComplete, VerifiedComplete).is_ok());
        assert!(validate_gap_transition(Unrecoverable, Unrecoverable).is_ok());
        assert!(validate_gap_transition(VerifiedComplete, Uncertain).is_err());
        assert!(validate_gap_transition(Unrecoverable, Backfilling).is_err());
        assert!(validate_gap_transition(VerifiedComplete, Backfilling).is_err());
    }

    #[test]
    fn reconnect_cannot_produce_verified_complete_without_proven_account_set() {
        // 真实 NapCat：所有已知 Scope 完整，但账号会话集合不可证完整。
        let mut evidence = BackfillEvidence {
            account_conversation_set_proven: false,
            budget_exhausted: false,
            scopes: vec![ScopeEvidence {
                scope_key: "group:g1".into(),
                pages_read: 3,
                reached_boundary: true,
                anchor_chain: vec![BackfillAnchor::new("m1", "1")],
                ..Default::default()
            }],
        };
        let completeness = HistoryCompleteness::from_evidence(&evidence);
        assert_eq!(completeness, HistoryCompleteness::KnownScopesComplete);
        // 重连只结束空窗时间，不等于已补齐：Gap 必须保持 uncertain。
        assert_eq!(
            completeness.gap_target_status(),
            IngestionGapStatus::Uncertain
        );
        assert_eq!(
            completeness.gap_reason(),
            Some(IngestionGapReason::HistoryUnprovable)
        );

        // 确定性 Fake：账号会话集合可证完整，才能 verified_complete。
        evidence.account_conversation_set_proven = true;
        assert_eq!(
            HistoryCompleteness::from_evidence(&evidence),
            HistoryCompleteness::ProvenComplete
        );
        assert_eq!(
            HistoryCompleteness::from_evidence(&evidence).gap_target_status(),
            IngestionGapStatus::VerifiedComplete
        );
    }

    #[test]
    fn budget_exhaustion_blocks_complete_even_if_scopes_reached_boundary() {
        let evidence = BackfillEvidence {
            account_conversation_set_proven: true,
            budget_exhausted: true,
            scopes: vec![ScopeEvidence {
                scope_key: "group:g1".into(),
                pages_read: 20,
                reached_boundary: true,
                anchor_chain: vec![BackfillAnchor::new("m1", "1")],
                ..Default::default()
            }],
        };
        // 达到预算上限时不能标记完整。
        assert_eq!(
            HistoryCompleteness::from_evidence(&evidence),
            HistoryCompleteness::Unprovable
        );
    }

    #[test]
    fn anomalies_make_scope_unprovable_or_unrecoverable() {
        let mut evidence = BackfillEvidence {
            account_conversation_set_proven: true,
            budget_exhausted: false,
            scopes: vec![ScopeEvidence {
                scope_key: "group:g1".into(),
                pages_read: 2,
                reached_boundary: true,
                anchor_chain: vec![BackfillAnchor::new("m1", "1")],
                anomalies: vec![BackfillAnomaly::DuplicatePage],
                ..Default::default()
            }],
        };
        assert_eq!(
            HistoryCompleteness::from_evidence(&evidence),
            HistoryCompleteness::Unprovable
        );

        evidence.scopes[0].anomalies = vec![BackfillAnomaly::PermissionDenied];
        assert_eq!(
            HistoryCompleteness::from_evidence(&evidence),
            HistoryCompleteness::Unrecoverable
        );
        assert_eq!(
            HistoryCompleteness::from_evidence(&evidence).gap_target_status(),
            IngestionGapStatus::Unrecoverable
        );
    }

    #[test]
    fn empty_scope_set_is_unprovable() {
        let evidence = BackfillEvidence {
            account_conversation_set_proven: true,
            budget_exhausted: false,
            scopes: vec![],
        };
        assert_eq!(
            HistoryCompleteness::from_evidence(&evidence),
            HistoryCompleteness::Unprovable
        );
    }

    #[test]
    fn scope_requires_pages_boundary_and_clean_chain_to_be_complete() {
        let mut scope = ScopeEvidence {
            scope_key: "group:g1".into(),
            pages_read: 1,
            reached_boundary: true,
            anchor_chain: vec![BackfillAnchor::new("m1", "1")],
            ..Default::default()
        };
        assert!(scope.is_complete());

        scope.reached_boundary = false;
        assert!(!scope.is_complete());
        scope.reached_boundary = true;

        scope.pages_read = 0;
        assert!(!scope.is_complete());
        scope.pages_read = 1;

        scope.anomalies.push(BackfillAnomaly::EmptyPage);
        assert!(!scope.is_complete());
    }

    #[test]
    fn budget_rejects_zero_over_limit_and_bad_retry_order() {
        let mut b = budget();
        b.page_size = 0;
        assert_eq!(b.validate(), Err(BackfillConfigError::PageSizeOutOfRange));
        b.page_size = 101;
        assert_eq!(b.validate(), Err(BackfillConfigError::PageSizeOutOfRange));
        b.page_size = 100;

        b.max_pages_per_scope = 0;
        assert_eq!(
            b.validate(),
            Err(BackfillConfigError::NotPositive("max_pages_per_scope"))
        );
        b.max_pages_per_scope = 10_001;
        assert_eq!(
            b.validate(),
            Err(BackfillConfigError::TooLarge("max_pages_per_scope"))
        );
        b.max_pages_per_scope = 20;

        b.max_concurrency = 65;
        assert_eq!(
            b.validate(),
            Err(BackfillConfigError::TooLarge("max_concurrency"))
        );
        b.max_concurrency = 2;

        b.retry_max_ms = b.retry_initial_ms - 1;
        assert_eq!(b.validate(), Err(BackfillConfigError::RetryMaxBelowInitial));

        b.retry_max_ms = 300_001;
        assert_eq!(
            b.validate(),
            Err(BackfillConfigError::TooLarge("retry_max_ms"))
        );
    }

    #[test]
    fn default_budget_validates() {
        assert!(budget().validate().is_ok());
    }

    #[test]
    fn backfill_run_and_lease_ids_reject_empty() {
        assert!(BackfillRunId::new("").is_err());
        assert!(BackfillRunId::new("  ").is_err());
        assert!(BackfillRunId::new("run-1").is_ok());
        assert!(BackfillLeaseToken::new("").is_err());
        assert!(BackfillLeaseToken::new("  ").is_err());
        assert!(BackfillLeaseToken::new("lease-1").is_ok());
    }

    #[test]
    fn reclaim_policy_separates_terminal_backoff_and_suspended() {
        // 终态：Gap 不再 uncertain，删除 reclaim_schedule 行。
        assert_eq!(
            HistoryCompleteness::ProvenComplete.reclaim_policy(),
            ReclaimPolicy::Terminal
        );
        assert_eq!(
            HistoryCompleteness::Unrecoverable.reclaim_policy(),
            ReclaimPolicy::Terminal
        );
        // 暂时性证据不足：短退避，尽快重试。
        assert_eq!(
            HistoryCompleteness::Unprovable.reclaim_policy(),
            ReclaimPolicy::Backoff(30)
        );
        // 已知 Scope 完成但账号集合不可证：Gap 边界已冻结，重跑无新证据，
        // 挂起自动重试（极远未来 next_eligible_at），仅人工重验或能力升级后重新排队。
        assert_eq!(
            HistoryCompleteness::KnownScopesComplete.reclaim_policy(),
            ReclaimPolicy::Suspended
        );
    }

    #[test]
    fn scope_key_is_kind_and_id_without_account() {
        use crate::{ConversationKind, ConversationRef};
        let scope = BackfillScope {
            account: SourceAccountRef::new(crate::MessageSource::NapCat, "acc-1").unwrap(),
            conversation: ConversationRef::new(ConversationKind::Group, "group-9").unwrap(),
            boundary_cursor: None,
        };
        assert_eq!(scope.scope_key(), "group:group-9");
    }
}
