//! 历史回补应用层：端口与用例编排。
//!
//! 本模块只依赖领域对象（[`crate::backfill`]）和抽象端口，不依赖 NapCat、SeaORM、
//! MySQL 或 `qqbot-server`。外层（`apps/qqbot-server`）实现 [`HistoryBackfillSourceT`]，
//! 基础设施层（`crates/personal-secretary/src/infra`）实现 [`BackfillStateStoreT`]。
//!
//! 用例职责：
//! 1. 原子领取一个 `uncertain` Gap（`uncertain -> backfilling`）；
//! 2. 读取账号下已知会话 Scope 与空窗前稳定游标；
//! 3. 有界分页读取历史，把每条历史消息交给统一幂等入口 [`InboundEventStoreT`]；
//! 4. 持久化每个 Scope 的进度与证据，支持崩溃恢复；
//! 5. 根据证据决定 Gap 保持 `uncertain`、标记 `verified_complete` 或 `unrecoverable`。
//!
//! 关键不变量：历史和实时消息最终调用同一个 `insert_message_if_absent`；重连不等于
//! 已补齐；只有充分证据（含账号会话集合可证完整）才能 `verified_complete`。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    BackfillAnchor, BackfillAnomaly, BackfillBudget, BackfillContinuation, BackfillCursor,
    BackfillError, BackfillEvidence, BackfillLease, BackfillLeaseToken, BackfillOutcome,
    BackfillPage, BackfillReadDirection, BackfillRunId, BackfillScope, BackfillScopeStatus,
    BackfillSourceError, ClaimedGap, HistoryCompleteness, InboundEventStoreError,
    InboundEventStoreT, IngestMessageOutcome, IngestionGapId, IngestionGapStatus, KnownScope,
    ScopeEvidence, ScopeProgress, validate_gap_transition,
};

/// 历史回补来源端口：外层 NapCat 适配器实现，按账号视角分页返回协议无关历史消息。
///
/// 所有 Cursor 和锚点必须绑定 [`SourceAccountRef`]；分页推进只能基于接口实际返回的
/// 真实锚点，禁止用数值加减生成下一锚点。
#[async_trait]
pub trait HistoryBackfillSourceT: Send + Sync {
    /// 按 [`BackfillReadDirection::NewestToOldest`] 读取一页历史消息。
    /// `cursor` 为 `None` 表示从最新位置开始；后续游标必须原样来自前页真实锚点。
    async fn fetch_page(
        &self,
        scope: &BackfillScope,
        cursor: Option<&BackfillCursor>,
        direction: BackfillReadDirection,
        page_size: u32,
    ) -> Result<BackfillPage, BackfillSourceError>;

    /// 该来源是否有权产生 [`BackfillContinuation::ProvenHistoryStart`]。
    /// 只有确定性 Fake 可返回 `true`；真实协议适配器必须返回 `false`。
    fn history_start_evidence_proven(&self) -> bool;

    /// 该来源是否已证明响应页遵循 [`BackfillReadDirection::NewestToOldest`]。
    /// 请求方向本身不是响应顺序证据；真实 NapCat 在外部验证完成前必须返回 `false`。
    fn page_order_evidence_proven(&self) -> bool;

    /// 该来源是否能证明账号级会话集合完整。
    ///
    /// 真实 NapCat 无法枚举账号全部会话，必须返回 `false`；确定性 Fake 来源可返回
    /// `true` 以验证 `verified_complete` 状态转换。
    fn account_conversation_set_proven(&self) -> bool;
}

/// 回补状态仓储端口：基础设施层（MySQL）实现，负责 Gap 原子领取、租约恢复、进度
/// 持久化与终态提交。
#[async_trait]
pub trait BackfillStateStoreT: Send + Sync {
    /// 原子领取一个 `uncertain` Gap（`uncertain -> backfilling`）并创建运行。
    /// 返回 `None` 表示当前没有可领取的 Gap。同一个 Gap 不能被并发领取两次。
    async fn claim_next_gap(
        &self,
        lease: BackfillLease,
    ) -> Result<Option<ClaimedGap>, InboundEventStoreError>;

    /// 恢复因租约过期而滞留的 `backfilling` 运行：延长租约并保留已有进度。
    async fn reclaim_expired(
        &self,
        lease: BackfillLease,
        limit: u32,
    ) -> Result<Vec<ClaimedGap>, InboundEventStoreError>;

    /// 读取该 Gap 已知会话 Scope 及其空窗前稳定游标快照。边界必须是 Gap 创建时冻结的
    /// 快照，而非领取时漂移的实时游标。
    async fn known_scopes_for_gap(
        &self,
        gap_id: &IngestionGapId,
    ) -> Result<Vec<KnownScope>, InboundEventStoreError>;

    /// 持久化（或更新）一个 Scope 的回补进度，并刷新运行租约。
    async fn record_scope_progress(
        &self,
        run_id: &BackfillRunId,
        lease_token: &BackfillLeaseToken,
        progress: &ScopeProgress,
    ) -> Result<(), InboundEventStoreError>;

    /// 读取一个运行的持久化进度（崩溃恢复）。
    async fn load_run_progress(
        &self,
        run_id: &BackfillRunId,
    ) -> Result<Option<Vec<ScopeProgress>>, InboundEventStoreError>;

    /// 根据 [`BackfillOutcome`] 提交运行终态与 Gap 状态转换（原子）。
    async fn finalize_run(
        &self,
        outcome: &BackfillOutcome,
        lease_token: &BackfillLeaseToken,
    ) -> Result<(), InboundEventStoreError>;
}

/// 回补用例需要同时幂等写入历史消息（[`InboundEventStoreT`]）和操作回补状态
/// （[`BackfillStateStoreT`]）。组合端口由基础设施层一次性实现。
pub trait BackfillStateStoreWithIngestionT: BackfillStateStoreT + InboundEventStoreT {}
impl<T> BackfillStateStoreWithIngestionT for T where T: BackfillStateStoreT + InboundEventStoreT {}

/// Gap 历史回补用例。协议无关，由外层 Worker 驱动。
pub struct BackfillGapUseCase {
    store: Arc<dyn BackfillStateStoreWithIngestionT>,
    history_source: Arc<dyn HistoryBackfillSourceT>,
    budget: BackfillBudget,
}

impl BackfillGapUseCase {
    pub fn new(
        store: Arc<dyn BackfillStateStoreWithIngestionT>,
        history_source: Arc<dyn HistoryBackfillSourceT>,
        budget: BackfillBudget,
    ) -> Self {
        Self {
            store,
            history_source,
            budget,
        }
    }

    fn lease(&self) -> BackfillLease {
        BackfillLease::new(self.budget.lease_secs)
    }

    /// 领取并处理一个 `uncertain` Gap。无 Gap 可领取时返回 `None`。
    pub async fn run_one(&self) -> Result<Option<BackfillOutcome>, BackfillError> {
        let Some(claimed) = self.store.claim_next_gap(self.lease()).await? else {
            return Ok(None);
        };
        let outcome = self.process_claimed(&claimed).await?;
        Ok(Some(outcome))
    }

    /// 仅领取指定数量的过期运行，不在领取调用内执行历史分页。
    ///
    /// Worker 会把返回值送入与新 Gap 相同的有界并发集合，避免一个串行恢复调用长时间
    /// 占用扫描循环，也避免一次加载无界数量的过期运行。
    pub async fn reclaim_expired(&self, limit: u32) -> Result<Vec<ClaimedGap>, BackfillError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        Ok(self.store.reclaim_expired(self.lease(), limit).await?)
    }

    /// 处理一个已领取的过期运行。租约令牌会贯穿进度续租和终态提交。
    pub async fn resume_claimed(
        &self,
        claimed: ClaimedGap,
    ) -> Result<BackfillOutcome, BackfillError> {
        self.process_claimed(&claimed).await
    }

    async fn process_claimed(
        &self,
        claimed: &ClaimedGap,
    ) -> Result<BackfillOutcome, BackfillError> {
        let run_id = claimed.run_id.clone();
        let gap_id = claimed.gap_id.clone();
        let account = claimed.account.clone();

        let known = self.store.known_scopes_for_gap(&gap_id).await?;
        let scopes: Vec<BackfillScope> = known
            .into_iter()
            .map(|known| BackfillScope {
                account: account.clone(),
                conversation: known.conversation,
                boundary_cursor: known.boundary_cursor,
            })
            .collect();

        let persisted = self
            .store
            .load_run_progress(&run_id)
            .await?
            .unwrap_or_default();
        let mut progress_map: HashMap<String, ScopeProgress> = persisted
            .into_iter()
            .map(|progress| (scope_key_of(&progress.conversation), progress))
            .collect();

        let mut evidence = BackfillEvidence {
            account_conversation_set_proven: self.history_source.account_conversation_set_proven(),
            budget_exhausted: false,
            scopes: Vec::with_capacity(scopes.len()),
        };

        // 无已知会话：无法证明账号会话集合，Gap 保持 uncertain。
        if scopes.is_empty() {
            let completeness = HistoryCompleteness::from_evidence(&evidence);
            return self
                .finalize(
                    &run_id,
                    &claimed.lease_token,
                    &gap_id,
                    completeness,
                    evidence,
                )
                .await;
        }

        // 崩溃恢复必须继承本运行已消耗的事件预算，不能从 0 重新开始，否则每次重启都可
        // 额外读取 max_events_per_run 条消息，失去“单次运行有界”的保证。
        let mut total_events = progress_map.values().fold(0u32, |total, progress| {
            total.saturating_add(progress.events_read)
        });
        let mut budget_exhausted = false;

        for scope in &scopes {
            let key = scope.scope_key();
            let mut progress = progress_map
                .remove(&key)
                .unwrap_or_else(|| fresh_progress(scope));

            // 已在之前的运行中验证完整的 Scope 直接采纳，跳过分页。
            if matches!(progress.status, BackfillScopeStatus::VerifiedComplete) {
                evidence.scopes.push(progress_to_evidence(&progress));
                continue;
            }

            let scope_budget_exhausted = match self
                .process_scope(
                    &run_id,
                    &claimed.lease_token,
                    scope,
                    &mut progress,
                    &mut total_events,
                )
                .await
            {
                Ok(exhausted) => exhausted,
                Err(BackfillError::Source(BackfillSourceError::PermissionDenied)) => {
                    progress.anomalies.push(BackfillAnomaly::PermissionDenied);
                    false
                }
                Err(BackfillError::Source(other)) => {
                    // 暂时性来源错误：记录进度后结束该 Scope 为不可证明，但不中止整个运行。
                    progress.anomalies.push(BackfillAnomaly::ProtocolError {
                        detail: other.to_string(),
                    });
                    false
                }
                Err(other) => return Err(other),
            };
            if scope_budget_exhausted {
                budget_exhausted = true;
            }

            progress.status = scope_terminal_status(&progress);
            self.store
                .record_scope_progress(&run_id, &claimed.lease_token, &progress)
                .await?;
            evidence.scopes.push(progress_to_evidence(&progress));
        }

        evidence.budget_exhausted = budget_exhausted;
        let completeness = HistoryCompleteness::from_evidence(&evidence);
        self.finalize(
            &run_id,
            &claimed.lease_token,
            &gap_id,
            completeness,
            evidence,
        )
        .await
    }

    /// 分页处理单个 Scope。更新 `progress` 与 `total_events`，返回该 Scope 是否命中预算上限。
    async fn process_scope(
        &self,
        run_id: &BackfillRunId,
        lease_token: &BackfillLeaseToken,
        scope: &BackfillScope,
        progress: &mut ScopeProgress,
        total_events: &mut u32,
    ) -> Result<bool, BackfillError> {
        progress.status = BackfillScopeStatus::Backfilling;
        let mut cursor = progress.last_cursor.clone();
        let mut session_chain: HashSet<BackfillAnchor> = HashSet::new();
        let mut budget_exhausted = false;

        if let Some(boundary) = &scope.boundary_cursor {
            if boundary.account != scope.account {
                progress
                    .anomalies
                    .push(BackfillAnomaly::CursorAccountMismatch);
                return Ok(false);
            }
            if boundary.anchor.message_id.trim().is_empty() {
                progress.anomalies.push(BackfillAnomaly::EmptyAnchor);
                return Ok(false);
            }
        }

        loop {
            // 页数预算检查。
            if progress.pages_read >= self.budget.max_pages_per_scope {
                progress.anomalies.push(BackfillAnomaly::BudgetExhausted);
                budget_exhausted = true;
                break;
            }
            // 事件数预算检查。
            if *total_events >= self.budget.max_events_per_run {
                progress.anomalies.push(BackfillAnomaly::BudgetExhausted);
                budget_exhausted = true;
                break;
            }

            if let Some(current) = &cursor {
                if current.account != scope.account {
                    progress
                        .anomalies
                        .push(BackfillAnomaly::CursorAccountMismatch);
                    break;
                }
                if !anchor_is_stable(&current.anchor) {
                    progress.anomalies.push(BackfillAnomaly::EmptyAnchor);
                    break;
                }
            }

            // 在发起远程读取前先持久化当前进度并刷新租约。这既避免长请求前租约过期，
            // 也让已被其它 Worker 接管的旧持有者在调用 NapCat 前被 fencing token 拒绝。
            self.store
                .record_scope_progress(run_id, lease_token, progress)
                .await?;

            let page = self
                .history_source
                .fetch_page(
                    scope,
                    cursor.as_ref(),
                    BackfillReadDirection::NewestToOldest,
                    self.budget.page_size,
                )
                .await?;

            progress.pages_read += 1;

            let page_order_proven = self.history_source.page_order_evidence_proven();
            let page_anomalies = validate_page(
                scope,
                cursor.as_ref(),
                &page,
                &session_chain,
                page_order_proven,
            );
            if !page_anomalies.is_empty() {
                progress.anomalies.extend(page_anomalies);
                if matches!(&page.continuation, BackfillContinuation::UnprovenStop) {
                    progress.anomalies.push(BackfillAnomaly::UnprovenStop);
                }
                if matches!(&page.continuation, BackfillContinuation::ProvenHistoryStart)
                    && !self.history_source.history_start_evidence_proven()
                {
                    progress
                        .anomalies
                        .push(BackfillAnomaly::UntrustedHistoryStart);
                }
                if matches!(&page.continuation, BackfillContinuation::ProvenHistoryStart)
                    && scope.boundary_cursor.is_some()
                {
                    progress.anomalies.push(BackfillAnomaly::BoundaryNotFound);
                }
                break;
            }
            if !page_order_proven {
                push_unique(&mut progress.anomalies, BackfillAnomaly::UntrustedPageOrder);
            }

            if matches!(&page.continuation, BackfillContinuation::ProvenHistoryStart)
                && !self.history_source.history_start_evidence_proven()
            {
                progress
                    .anomalies
                    .push(BackfillAnomaly::UntrustedHistoryStart);
                break;
            }

            let mut stop_at_boundary = false;
            for item in &page.items {
                if *total_events >= self.budget.max_events_per_run {
                    progress.anomalies.push(BackfillAnomaly::BudgetExhausted);
                    budget_exhausted = true;
                    break;
                }
                *total_events += 1;
                progress.events_read += 1;

                // 请求方向本身不能证明具体协议实现的响应顺序。顺序未验证时仍可
                // 有界恢复候选事件，但不能用页内位置完成 Scope 或跳过后续事件。
                let is_boundary = page_order_proven
                    && scope.boundary_cursor.as_ref().is_some_and(|boundary| {
                        item.anchor.message_id == boundary.anchor.message_id
                    });

                match self.store.insert_message_if_absent(&item.envelope).await {
                    Ok(IngestMessageOutcome::Accepted { .. }) => {
                        progress.accepted += 1;
                        if is_boundary {
                            progress
                                .anomalies
                                .push(BackfillAnomaly::BoundaryStateMismatch);
                            stop_at_boundary = true;
                        }
                    }
                    Ok(IngestMessageOutcome::Duplicate { .. }) => {
                        progress.duplicates += 1;
                        if is_boundary {
                            progress.reached_boundary = true;
                            stop_at_boundary = true;
                        }
                    }
                    Err(error) => {
                        // 统一幂等入口的暂时性错误向上传播为来源不可用，由调用方记录。
                        return Err(BackfillSourceError::Unavailable(error.to_string()).into());
                    }
                }

                session_chain.insert(item.anchor.clone());
                if stop_at_boundary || budget_exhausted {
                    // 页内顺序为新到旧；边界及预算停止点之后的更旧消息不得写入。
                    break;
                }
            }

            if stop_at_boundary || budget_exhausted {
                break;
            }

            match page.continuation {
                BackfillContinuation::Next(next) => {
                    cursor = Some(next);
                    progress.last_cursor = cursor.clone();
                }
                BackfillContinuation::ProvenHistoryStart => {
                    progress.last_cursor = None;
                    if scope.boundary_cursor.is_none() {
                        if page_order_proven {
                            progress.reached_boundary = true;
                        }
                    } else {
                        progress.anomalies.push(BackfillAnomaly::BoundaryNotFound);
                    }
                    break;
                }
                BackfillContinuation::UnprovenStop => {
                    progress.last_cursor = None;
                    progress.anomalies.push(BackfillAnomaly::UnprovenStop);
                    break;
                }
            }

            // 持久化进度并刷新租约，支持崩溃恢复。
            progress.status = BackfillScopeStatus::Backfilling;
            self.store
                .record_scope_progress(run_id, lease_token, progress)
                .await?;
        }

        Ok(budget_exhausted)
    }

    async fn finalize(
        &self,
        run_id: &BackfillRunId,
        lease_token: &BackfillLeaseToken,
        gap_id: &IngestionGapId,
        completeness: HistoryCompleteness,
        evidence: BackfillEvidence,
    ) -> Result<BackfillOutcome, BackfillError> {
        let gap_target_status = completeness.gap_target_status();
        // claim 已完成 uncertain -> backfilling；这里校验 backfilling -> target 合法。
        validate_gap_transition(IngestionGapStatus::Backfilling, gap_target_status)?;
        let outcome = BackfillOutcome {
            run_id: run_id.clone(),
            gap_id: gap_id.clone(),
            completeness,
            evidence,
            gap_target_status,
            gap_reason: completeness.gap_reason(),
            failure_class: None,
        };
        self.store.finalize_run(&outcome, lease_token).await?;
        Ok(outcome)
    }
}

fn anchor_is_stable(anchor: &BackfillAnchor) -> bool {
    !anchor.message_id.trim().is_empty() && !anchor.message_seq.trim().is_empty()
}

/// 在任何 ingestion 前校验整页，避免部分写入后才发现身份或 continuation 非法。
fn validate_page(
    scope: &BackfillScope,
    current: Option<&BackfillCursor>,
    page: &BackfillPage,
    session_chain: &HashSet<BackfillAnchor>,
    page_order_proven: bool,
) -> Vec<BackfillAnomaly> {
    let mut anomalies = Vec::new();
    if page.items.is_empty() {
        anomalies.push(BackfillAnomaly::EmptyPage);
    }

    let mut page_anchors = HashSet::new();
    for item in &page.items {
        if !anchor_is_stable(&item.anchor)
            || item.envelope.source.message_id.trim().is_empty()
            || item.envelope.source.message_id != item.anchor.message_id
        {
            push_unique(&mut anomalies, BackfillAnomaly::EmptyAnchor);
        }
        if item.envelope.source.account_ref() != scope.account
            || item.envelope.conversation != scope.conversation
        {
            push_unique(&mut anomalies, BackfillAnomaly::MessageScopeMismatch);
        }
        if !page_anchors.insert(item.anchor.clone()) {
            push_unique(&mut anomalies, BackfillAnomaly::DuplicateAnchor);
        }
    }

    if !page.items.is_empty()
        && page
            .items
            .iter()
            .all(|item| session_chain.contains(&item.anchor))
    {
        push_unique(&mut anomalies, BackfillAnomaly::DuplicatePage);
    }

    if let Some(current) = current {
        let current_position = page
            .items
            .iter()
            .position(|item| item.anchor == current.anchor);
        match current_position {
            None => push_unique(&mut anomalies, BackfillAnomaly::AnchorDisappeared),
            Some(0) => {}
            Some(_) if page_order_proven => {
                push_unique(&mut anomalies, BackfillAnomaly::SortConflict)
            }
            Some(_) => {}
        }
        if page_order_proven
            && page
                .items
                .iter()
                .any(|item| session_chain.contains(&item.anchor) && item.anchor != current.anchor)
        {
            push_unique(&mut anomalies, BackfillAnomaly::SortConflict);
        }
    }

    if let BackfillContinuation::Next(next) = &page.continuation {
        if next.account != scope.account {
            push_unique(&mut anomalies, BackfillAnomaly::CursorAccountMismatch);
        }
        if !anchor_is_stable(&next.anchor) {
            push_unique(&mut anomalies, BackfillAnomaly::EmptyAnchor);
        }
        if !page_anchors.contains(&next.anchor)
            || (page_order_proven
                && page.items.last().map(|item| &item.anchor) != Some(&next.anchor))
        {
            push_unique(&mut anomalies, BackfillAnomaly::InvalidContinuation);
        }
        if current == Some(next) {
            push_unique(&mut anomalies, BackfillAnomaly::NoCursorAdvance);
        }
    }

    anomalies
}

fn push_unique(anomalies: &mut Vec<BackfillAnomaly>, anomaly: BackfillAnomaly) {
    if !anomalies.contains(&anomaly) {
        anomalies.push(anomaly);
    }
}

fn scope_key_of(conversation: &crate::ConversationRef) -> String {
    format!("{}:{}", conversation.kind.as_str(), conversation.id)
}

fn fresh_progress(scope: &BackfillScope) -> ScopeProgress {
    ScopeProgress {
        conversation: scope.conversation.clone(),
        status: BackfillScopeStatus::Pending,
        last_cursor: None,
        pages_read: 0,
        events_read: 0,
        accepted: 0,
        duplicates: 0,
        reached_boundary: false,
        anomalies: Vec::new(),
    }
}

fn scope_terminal_status(progress: &ScopeProgress) -> BackfillScopeStatus {
    let evidence = progress_to_evidence(progress);
    if evidence.is_complete() {
        BackfillScopeStatus::VerifiedComplete
    } else if evidence.is_unrecoverable() {
        BackfillScopeStatus::Unrecoverable
    } else {
        BackfillScopeStatus::Unprovable
    }
}

fn progress_to_evidence(progress: &ScopeProgress) -> ScopeEvidence {
    ScopeEvidence {
        scope_key: scope_key_of(&progress.conversation),
        pages_read: progress.pages_read,
        events_read: progress.events_read,
        accepted: progress.accepted,
        duplicates: progress.duplicates,
        anchor_chain: Vec::new(),
        reached_boundary: progress.reached_boundary,
        anomalies: progress.anomalies.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::*;
    use crate::{
        BackfillHistoryItem, ConnectionEpochId, ConversationKind, ConversationRef,
        InboundMessageEnvelope, IngestionGapId, MessageSource, SourceAccountRef, SourceMessageRef,
        VerifiedActor, VerifiedActorKind,
    };

    fn budget() -> BackfillBudget {
        BackfillBudget {
            page_size: 10,
            max_pages_per_scope: 20,
            max_events_per_run: 2000,
            max_concurrency: 2,
            lease_secs: 60,
            retry_initial_ms: 1,
            retry_max_ms: 2,
        }
    }

    fn account_ref(account_id: &str) -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, account_id).unwrap()
    }

    fn cursor(account_id: &str, msg_id: &str, seq: &str) -> BackfillCursor {
        BackfillCursor::new(account_ref(account_id), BackfillAnchor::new(msg_id, seq))
    }

    fn hist_item(account_id: &str, msg_id: &str, seq: &str, conv_id: &str) -> BackfillHistoryItem {
        let envelope = InboundMessageEnvelope::new(
            SourceMessageRef::new(MessageSource::NapCat, account_id, msg_id).unwrap(),
            ConversationRef::new(ConversationKind::Group, conv_id).unwrap(),
            VerifiedActor::new(VerifiedActorKind::External, "sender-1").unwrap(),
            1_800_000_000,
            "",
            Vec::new(),
        )
        .unwrap();
        BackfillHistoryItem {
            envelope,
            anchor: BackfillAnchor::new(msg_id, seq),
        }
    }

    fn claimed_gap(run: &str, gap: &str, account_id: &str) -> ClaimedGap {
        ClaimedGap {
            run_id: BackfillRunId::new(run).unwrap(),
            lease_token: BackfillLeaseToken::new(format!("lease-{run}")).unwrap(),
            gap_id: IngestionGapId::new(gap).unwrap(),
            account: account_ref(account_id),
            connection_epoch_id: ConnectionEpochId::new("epoch-1").unwrap(),
            is_resume: false,
        }
    }

    fn claimed_gap_resume(run: &str, gap: &str, account_id: &str) -> ClaimedGap {
        ClaimedGap {
            is_resume: true,
            ..claimed_gap(run, gap, account_id)
        }
    }

    fn known_scope(conv_id: &str, boundary: Option<BackfillCursor>) -> KnownScope {
        KnownScope {
            conversation: ConversationRef::new(ConversationKind::Group, conv_id).unwrap(),
            boundary_cursor: boundary,
        }
    }

    fn page(items: Vec<BackfillHistoryItem>, continuation: BackfillContinuation) -> BackfillPage {
        BackfillPage {
            items,
            continuation,
        }
    }

    /// 幂等写入 + 回补状态的内存实现，用于验证用例编排。
    #[derive(Default)]
    struct FakeStore {
        state: Mutex<FakeState>,
    }

    #[derive(Default)]
    struct FakeState {
        inserted: HashMap<String, usize>, // idempotency_key -> 落库次数（仅 Accepted 递增）
        unique_events: usize,
        claimable: VecDeque<ClaimedGap>,
        reclaimed: Vec<ClaimedGap>,
        known_scopes: Vec<KnownScope>,
        progress: HashMap<String, ScopeProgress>, // run_id|scope_key -> progress
        finalizations: Vec<BackfillOutcome>,
    }

    impl FakeStore {
        async fn seed_message(&self, account_id: &str, msg_id: &str, conv_id: &str) {
            let envelope = hist_item(account_id, msg_id, "seed", conv_id).envelope;
            let _ = <Self as InboundEventStoreT>::insert_message_if_absent(self, &envelope).await;
        }

        fn set_progress(&self, run_id: &str, progress: ScopeProgress) {
            let key = format!(
                "{}|{}:{}",
                run_id,
                progress.conversation.kind.as_str(),
                progress.conversation.id
            );
            self.state.lock().unwrap().progress.insert(key, progress);
        }

        fn finalizations(&self) -> Vec<BackfillOutcome> {
            self.state.lock().unwrap().finalizations.clone()
        }

        fn unique_events(&self) -> usize {
            self.state.lock().unwrap().unique_events
        }

        fn has_message(&self, account_id: &str, msg_id: &str, conv_id: &str) -> bool {
            let key = hist_item(account_id, msg_id, "probe", conv_id)
                .envelope
                .idempotency_key()
                .as_str()
                .to_owned();
            self.state.lock().unwrap().inserted.contains_key(&key)
        }
    }

    #[async_trait]
    impl InboundEventStoreT for FakeStore {
        async fn insert_message_if_absent(
            &self,
            message: &InboundMessageEnvelope,
        ) -> Result<IngestMessageOutcome, InboundEventStoreError> {
            let key = message.idempotency_key().as_str().to_owned();
            let mut state = self.state.lock().unwrap();
            let source_event_id =
                crate::SourceEventId::new(format!("evt-{}", key.replace(':', "_"))).unwrap();
            if state.inserted.contains_key(&key) {
                return Ok(IngestMessageOutcome::Duplicate { source_event_id });
            }
            state.inserted.insert(key, 1);
            state.unique_events += 1;
            Ok(IngestMessageOutcome::Accepted {
                source_event_id,
                reply_to_event_id: None,
            })
        }
    }

    #[async_trait]
    impl BackfillStateStoreT for FakeStore {
        async fn claim_next_gap(
            &self,
            _lease: BackfillLease,
        ) -> Result<Option<ClaimedGap>, InboundEventStoreError> {
            Ok(self.state.lock().unwrap().claimable.pop_front())
        }

        async fn reclaim_expired(
            &self,
            _lease: BackfillLease,
            limit: u32,
        ) -> Result<Vec<ClaimedGap>, InboundEventStoreError> {
            let mut state = self.state.lock().unwrap();
            let take = usize::try_from(limit)
                .unwrap_or(usize::MAX)
                .min(state.reclaimed.len());
            Ok(state.reclaimed.drain(..take).collect())
        }

        async fn known_scopes_for_gap(
            &self,
            _gap_id: &IngestionGapId,
        ) -> Result<Vec<KnownScope>, InboundEventStoreError> {
            Ok(self.state.lock().unwrap().known_scopes.clone())
        }

        async fn record_scope_progress(
            &self,
            run_id: &BackfillRunId,
            _lease_token: &BackfillLeaseToken,
            progress: &ScopeProgress,
        ) -> Result<(), InboundEventStoreError> {
            let key = format!(
                "{}|{}:{}",
                run_id.as_str(),
                progress.conversation.kind.as_str(),
                progress.conversation.id
            );
            self.state
                .lock()
                .unwrap()
                .progress
                .insert(key, progress.clone());
            Ok(())
        }

        async fn load_run_progress(
            &self,
            run_id: &BackfillRunId,
        ) -> Result<Option<Vec<ScopeProgress>>, InboundEventStoreError> {
            let prefix = format!("{}|", run_id.as_str());
            let scopes: Vec<ScopeProgress> = self
                .state
                .lock()
                .unwrap()
                .progress
                .iter()
                .filter(|(key, _)| key.starts_with(&prefix))
                .map(|(_, value)| value.clone())
                .collect();
            Ok((!scopes.is_empty()).then_some(scopes))
        }

        async fn finalize_run(
            &self,
            outcome: &BackfillOutcome,
            _lease_token: &BackfillLeaseToken,
        ) -> Result<(), InboundEventStoreError> {
            self.state
                .lock()
                .unwrap()
                .finalizations
                .push(outcome.clone());
            Ok(())
        }
    }

    /// 可脚本化的历史来源：按调用顺序弹出预置页，并记录每次 fetch 的游标。
    struct FakeSource {
        pages: Mutex<VecDeque<BackfillPage>>,
        proven: bool,
        page_order_proven: bool,
        fetch_log: Mutex<Vec<Option<BackfillCursor>>>,
    }

    impl FakeSource {
        fn new(proven: bool) -> Self {
            Self {
                pages: Mutex::new(VecDeque::new()),
                proven,
                page_order_proven: true,
                fetch_log: Mutex::new(Vec::new()),
            }
        }

        fn with_unproven_page_order(proven: bool) -> Self {
            Self {
                pages: Mutex::new(VecDeque::new()),
                proven,
                page_order_proven: false,
                fetch_log: Mutex::new(Vec::new()),
            }
        }

        fn queue(&self, page: BackfillPage) {
            self.pages.lock().unwrap().push_back(page);
        }

        fn fetch_log(&self) -> Vec<Option<BackfillCursor>> {
            self.fetch_log.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl HistoryBackfillSourceT for FakeSource {
        async fn fetch_page(
            &self,
            _scope: &BackfillScope,
            cursor: Option<&BackfillCursor>,
            direction: BackfillReadDirection,
            _page_size: u32,
        ) -> Result<BackfillPage, BackfillSourceError> {
            assert_eq!(direction, BackfillReadDirection::NewestToOldest);
            self.fetch_log.lock().unwrap().push(cursor.cloned());
            self.pages
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| BackfillSourceError::Unavailable("no scripted page".into()))
        }

        fn history_start_evidence_proven(&self) -> bool {
            true
        }

        fn page_order_evidence_proven(&self) -> bool {
            self.page_order_proven
        }

        fn account_conversation_set_proven(&self) -> bool {
            self.proven
        }
    }

    fn build(
        store: Arc<FakeStore>,
        source: Arc<FakeSource>,
        budget: BackfillBudget,
    ) -> BackfillGapUseCase {
        let store_port: Arc<dyn BackfillStateStoreWithIngestionT> = store;
        let source_port: Arc<dyn HistoryBackfillSourceT> = source;
        BackfillGapUseCase::new(store_port, source_port, budget)
    }

    #[tokio::test]
    async fn fake_source_with_sufficient_evidence_marks_gap_verified_complete() {
        let store = Arc::new(FakeStore::default());
        let source = Arc::new(FakeSource::new(true));
        // 空窗前稳定边界消息已实时落库；历史回补回读到它即为连续性证据。
        store.seed_message("acc-1", "old", "g1").await;
        store.state.lock().unwrap().known_scopes =
            vec![known_scope("g1", Some(cursor("acc-1", "old", "seed")))];
        store
            .state
            .lock()
            .unwrap()
            .claimable
            .push_back(claimed_gap("run-1", "gap-1", "acc-1"));
        source.queue(page(
            vec![
                hist_item("acc-1", "new", "s-new", "g1"),
                hist_item("acc-1", "mid", "s-mid", "g1"),
                hist_item("acc-1", "old", "seed", "g1"),
            ],
            BackfillContinuation::ProvenHistoryStart,
        ));

        let outcome = build(store.clone(), source, budget())
            .run_one()
            .await
            .unwrap()
            .expect("a claimable gap must be processed");

        assert_eq!(outcome.completeness, HistoryCompleteness::ProvenComplete);
        assert_eq!(
            outcome.gap_target_status,
            IngestionGapStatus::VerifiedComplete
        );
        let scope = &outcome.evidence.scopes[0];
        assert_eq!(scope.accepted, 2);
        assert_eq!(scope.duplicates, 1);
        assert!(scope.reached_boundary);
        assert_eq!(
            store.finalizations()[0].gap_target_status,
            IngestionGapStatus::VerifiedComplete
        );
    }

    #[tokio::test]
    async fn reconnect_with_unproven_account_set_keeps_gap_uncertain() {
        let store = Arc::new(FakeStore::default());
        let source = Arc::new(FakeSource::new(false)); // 真实 NapCat 无法证明账号会话集合完整
        store.seed_message("acc-1", "old", "g1").await;
        store.state.lock().unwrap().known_scopes =
            vec![known_scope("g1", Some(cursor("acc-1", "old", "seed")))];
        store
            .state
            .lock()
            .unwrap()
            .claimable
            .push_back(claimed_gap("run-1", "gap-1", "acc-1"));
        source.queue(page(
            vec![
                hist_item("acc-1", "new", "s-new", "g1"),
                hist_item("acc-1", "old", "seed", "g1"),
            ],
            BackfillContinuation::ProvenHistoryStart,
        ));

        let outcome = build(store, source, budget())
            .run_one()
            .await
            .unwrap()
            .unwrap();

        // 重连只结束空窗时间，不等于已补齐：所有已知 Scope 完整但账号集合不可证 => uncertain。
        assert_eq!(
            outcome.completeness,
            HistoryCompleteness::KnownScopesComplete
        );
        assert_eq!(outcome.gap_target_status, IngestionGapStatus::Uncertain);
        assert_eq!(
            outcome.gap_reason,
            Some(crate::IngestionGapReason::HistoryUnprovable)
        );
    }

    #[tokio::test]
    async fn budget_exhaustion_keeps_gap_uncertain() {
        let store = Arc::new(FakeStore::default());
        let source = Arc::new(FakeSource::new(true));
        store.seed_message("acc-1", "m3", "g1").await; // 边界在第 2 页，预算只允许 1 页
        store.state.lock().unwrap().known_scopes =
            vec![known_scope("g1", Some(cursor("acc-1", "m3", "seed")))];
        store
            .state
            .lock()
            .unwrap()
            .claimable
            .push_back(claimed_gap("run-1", "gap-1", "acc-1"));
        source.queue(page(
            vec![
                hist_item("acc-1", "m1", "s1", "g1"),
                hist_item("acc-1", "m2", "s2", "g1"),
            ],
            BackfillContinuation::Next(cursor("acc-1", "m2", "s2")),
        ));
        source.queue(page(
            vec![hist_item("acc-1", "m3", "seed", "g1")],
            BackfillContinuation::ProvenHistoryStart,
        ));

        let mut bounded = budget();
        bounded.max_pages_per_scope = 1;
        let outcome = build(store, source, bounded)
            .run_one()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(outcome.completeness, HistoryCompleteness::Unprovable);
        assert!(outcome.evidence.budget_exhausted);
        assert_eq!(outcome.gap_target_status, IngestionGapStatus::Uncertain);
    }

    #[tokio::test]
    async fn no_cursor_advance_stops_and_keeps_uncertain() {
        let store = Arc::new(FakeStore::default());
        let source = Arc::new(FakeSource::new(true));
        store.state.lock().unwrap().known_scopes = vec![known_scope("g1", None)];
        store
            .state
            .lock()
            .unwrap()
            .claimable
            .push_back(claimed_gap("run-1", "gap-1", "acc-1"));
        let stall = cursor("acc-1", "m1", "s1");
        source.queue(page(
            vec![hist_item("acc-1", "m1", "s1", "g1")],
            BackfillContinuation::Next(stall.clone()),
        ));
        // 第二页返回相同游标，分页未推进。
        source.queue(page(
            vec![hist_item("acc-1", "m2", "s2", "g1")],
            BackfillContinuation::Next(stall),
        ));

        let outcome = build(store, source, budget())
            .run_one()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(outcome.completeness, HistoryCompleteness::Unprovable);
        assert!(
            outcome.evidence.scopes[0]
                .anomalies
                .iter()
                .any(|a| matches!(a, BackfillAnomaly::NoCursorAdvance))
        );
    }

    #[tokio::test]
    async fn worker_restart_resumes_from_persisted_progress() {
        let store = Arc::new(FakeStore::default());
        let source = Arc::new(FakeSource::new(true));
        // 两个已知 Scope：A 已在崩溃前验证完整，B 停在游标 X。
        store.seed_message("acc-1", "ba", "ga").await;
        store.seed_message("acc-1", "bb", "gb").await;
        store.state.lock().unwrap().known_scopes = vec![
            known_scope("ga", Some(cursor("acc-1", "ba", "seed"))),
            known_scope("gb", Some(cursor("acc-1", "bb", "seed"))),
        ];
        store.set_progress(
            "run-1",
            ScopeProgress {
                conversation: ConversationRef::new(ConversationKind::Group, "ga").unwrap(),
                status: BackfillScopeStatus::VerifiedComplete,
                pages_read: 1,
                reached_boundary: true,
                ..fresh()
            },
        );
        let resume_cursor = cursor("acc-1", "mid-b", "smid");
        store.set_progress(
            "run-1",
            ScopeProgress {
                conversation: ConversationRef::new(ConversationKind::Group, "gb").unwrap(),
                status: BackfillScopeStatus::Backfilling,
                last_cursor: Some(resume_cursor.clone()),
                pages_read: 1,
                ..fresh()
            },
        );
        store
            .state
            .lock()
            .unwrap()
            .reclaimed
            .push(claimed_gap_resume("run-1", "gap-1", "acc-1"));
        // 仅 Scope B 需要继续读取，且从持久化的 X 开始。
        source.queue(page(
            vec![
                hist_item("acc-1", "mid-b", "smid", "gb"),
                hist_item("acc-1", "bb", "seed", "gb"),
            ],
            BackfillContinuation::ProvenHistoryStart,
        ));

        let use_case = build(store, source.clone(), budget());
        let claimed = use_case.reclaim_expired(1).await.unwrap();
        let outcome = use_case
            .resume_claimed(claimed.into_iter().next().unwrap())
            .await
            .unwrap();

        // Scope A 被跳过（未 fetch），Scope B 从持久化游标 X 恢复。
        assert_eq!(source.fetch_log(), vec![Some(resume_cursor)]);
        assert_eq!(outcome.completeness, HistoryCompleteness::ProvenComplete);
        assert_eq!(
            outcome.gap_target_status,
            IngestionGapStatus::VerifiedComplete
        );
    }

    #[tokio::test]
    async fn worker_restart_keeps_the_persisted_total_event_budget() {
        let store = Arc::new(FakeStore::default());
        let source = Arc::new(FakeSource::new(true));
        store.state.lock().unwrap().known_scopes = vec![known_scope("g1", None)];
        store.set_progress(
            "run-1",
            ScopeProgress {
                conversation: ConversationRef::new(ConversationKind::Group, "g1").unwrap(),
                status: BackfillScopeStatus::Backfilling,
                events_read: 100,
                ..fresh()
            },
        );
        store
            .state
            .lock()
            .unwrap()
            .reclaimed
            .push(claimed_gap_resume("run-1", "gap-1", "acc-1"));

        let mut bounded = budget();
        bounded.max_events_per_run = 100;
        let use_case = build(store, source.clone(), bounded);
        let claimed = use_case.reclaim_expired(1).await.unwrap();
        let outcome = use_case
            .resume_claimed(claimed.into_iter().next().unwrap())
            .await
            .unwrap();

        assert!(source.fetch_log().is_empty());
        assert!(outcome.evidence.budget_exhausted);
        assert_eq!(outcome.completeness, HistoryCompleteness::Unprovable);
    }

    #[tokio::test]
    async fn same_gap_cannot_be_claimed_twice() {
        let store = Arc::new(FakeStore::default());
        store
            .state
            .lock()
            .unwrap()
            .claimable
            .push_back(claimed_gap("run-1", "gap-1", "acc-1"));

        let first = store.claim_next_gap(BackfillLease::new(60)).await.unwrap();
        let second = store.claim_next_gap(BackfillLease::new(60)).await.unwrap();
        assert!(first.is_some(), "first claim must succeed");
        assert!(
            second.is_none(),
            "second concurrent claim must not get the same gap"
        );
    }

    #[tokio::test]
    async fn realtime_and_history_share_one_idempotent_entry() {
        let store = Arc::new(FakeStore::default());
        // 实时先到：M 已落库。
        store.seed_message("acc-1", "M", "g1").await;
        let before = store.unique_events();
        assert_eq!(before, 1);

        store.state.lock().unwrap().known_scopes =
            vec![known_scope("g1", Some(cursor("acc-1", "M", "seed")))];
        store
            .state
            .lock()
            .unwrap()
            .claimable
            .push_back(claimed_gap("run-1", "gap-1", "acc-1"));
        let source = Arc::new(FakeSource::new(true));
        // 历史后到：同一条 M 经统一幂等入口返回 Duplicate，不产生新 SourceEvent。
        source.queue(page(
            vec![hist_item("acc-1", "M", "seed", "g1")],
            BackfillContinuation::ProvenHistoryStart,
        ));

        let outcome = build(store.clone(), source, budget())
            .run_one()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            store.unique_events(),
            1,
            "history must not duplicate the realtime event"
        );
        assert_eq!(outcome.evidence.scopes[0].duplicates, 1);
    }

    #[tokio::test]
    async fn empty_scope_set_is_unprovable_and_keeps_uncertain() {
        let store = Arc::new(FakeStore::default());
        store
            .state
            .lock()
            .unwrap()
            .claimable
            .push_back(claimed_gap("run-1", "gap-1", "acc-1"));
        // 无已知会话：无法证明账号会话集合完整。
        store.state.lock().unwrap().known_scopes = vec![];
        let source = Arc::new(FakeSource::new(true));

        let outcome = build(store, source, budget())
            .run_one()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(outcome.completeness, HistoryCompleteness::Unprovable);
        assert_eq!(outcome.gap_target_status, IngestionGapStatus::Uncertain);
    }

    #[tokio::test]
    async fn multipage_overlap_hits_frozen_boundary_and_skips_older_items() {
        let store = Arc::new(FakeStore::default());
        let source = Arc::new(FakeSource::new(true));
        store.seed_message("acc-1", "boundary", "g1").await;
        store.state.lock().unwrap().known_scopes = vec![known_scope(
            "g1",
            Some(cursor("acc-1", "boundary", "boundary-seq")),
        )];
        store.state.lock().unwrap().claimable.push_back(claimed_gap(
            "run-overlap",
            "gap-overlap",
            "acc-1",
        ));

        let overlap = cursor("acc-1", "overlap", "opaque-overlap");
        source.queue(page(
            vec![
                hist_item("acc-1", "new-1", "opaque-new-1", "g1"),
                hist_item("acc-1", "new-2", "opaque-new-2", "g1"),
                hist_item("acc-1", "overlap", "opaque-overlap", "g1"),
            ],
            BackfillContinuation::Next(overlap.clone()),
        ));
        source.queue(page(
            vec![
                hist_item("acc-1", "overlap", "opaque-overlap", "g1"),
                hist_item("acc-1", "new-3", "opaque-new-3", "g1"),
                hist_item("acc-1", "boundary", "boundary-seq", "g1"),
                hist_item("acc-1", "must-not-write", "opaque-older", "g1"),
            ],
            BackfillContinuation::Next(cursor("acc-1", "must-not-write", "opaque-older")),
        ));

        let outcome = build(store.clone(), source.clone(), budget())
            .run_one()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(outcome.completeness, HistoryCompleteness::ProvenComplete);
        assert!(outcome.evidence.scopes[0].reached_boundary);
        assert_eq!(outcome.evidence.scopes[0].accepted, 4);
        assert_eq!(outcome.evidence.scopes[0].duplicates, 2);
        assert!(!store.has_message("acc-1", "must-not-write", "g1"));
        assert_eq!(source.fetch_log(), vec![None, Some(overlap)]);
    }

    #[tokio::test]
    async fn accepted_frozen_boundary_is_inconsistent_and_skips_older_items() {
        let store = Arc::new(FakeStore::default());
        let source = Arc::new(FakeSource::new(true));
        store.state.lock().unwrap().known_scopes = vec![known_scope(
            "g1",
            Some(cursor("acc-1", "boundary", "boundary-seq")),
        )];
        store.state.lock().unwrap().claimable.push_back(claimed_gap(
            "run-accepted",
            "gap-accepted",
            "acc-1",
        ));
        source.queue(page(
            vec![
                hist_item("acc-1", "new", "opaque-new", "g1"),
                hist_item("acc-1", "boundary", "boundary-seq", "g1"),
                hist_item("acc-1", "must-not-write", "opaque-older", "g1"),
            ],
            BackfillContinuation::ProvenHistoryStart,
        ));

        let outcome = build(store.clone(), source, budget())
            .run_one()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(outcome.completeness, HistoryCompleteness::Unprovable);
        assert!(!outcome.evidence.scopes[0].reached_boundary);
        assert!(
            outcome.evidence.scopes[0]
                .anomalies
                .contains(&BackfillAnomaly::BoundaryStateMismatch)
        );
        assert!(!store.has_message("acc-1", "must-not-write", "g1"));
    }

    #[tokio::test]
    async fn proven_history_start_before_frozen_boundary_is_unprovable() {
        let store = Arc::new(FakeStore::default());
        let source = Arc::new(FakeSource::new(true));
        store.seed_message("acc-1", "missing-boundary", "g1").await;
        store.state.lock().unwrap().known_scopes = vec![known_scope(
            "g1",
            Some(cursor("acc-1", "missing-boundary", "boundary-seq")),
        )];
        store.state.lock().unwrap().claimable.push_back(claimed_gap(
            "run-missing",
            "gap-missing",
            "acc-1",
        ));
        source.queue(page(
            vec![hist_item("acc-1", "only-history", "opaque", "g1")],
            BackfillContinuation::ProvenHistoryStart,
        ));

        let outcome = build(store, source, budget())
            .run_one()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.completeness, HistoryCompleteness::Unprovable);
        assert!(
            outcome.evidence.scopes[0]
                .anomalies
                .contains(&BackfillAnomaly::BoundaryNotFound)
        );
    }

    async fn run_without_boundary(account_set_proven: bool) -> BackfillOutcome {
        let store = Arc::new(FakeStore::default());
        let source = Arc::new(FakeSource::new(account_set_proven));
        store.state.lock().unwrap().known_scopes = vec![known_scope("g1", None)];
        store.state.lock().unwrap().claimable.push_back(claimed_gap(
            "run-start",
            "gap-start",
            "acc-1",
        ));
        source.queue(page(
            vec![hist_item("acc-1", "oldest", "opaque-oldest", "g1")],
            BackfillContinuation::ProvenHistoryStart,
        ));
        build(store, source, budget())
            .run_one()
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn no_boundary_and_proven_start_requires_proven_account_set_for_complete_gap() {
        let proven = run_without_boundary(true).await;
        assert_eq!(proven.completeness, HistoryCompleteness::ProvenComplete);

        let account_set_unproven = run_without_boundary(false).await;
        assert_eq!(
            account_set_unproven.completeness,
            HistoryCompleteness::KnownScopesComplete
        );
        assert_eq!(
            account_set_unproven.gap_target_status,
            IngestionGapStatus::Uncertain
        );
    }

    #[tokio::test]
    async fn unproven_page_order_recovers_candidates_without_completing_scope() {
        let store = Arc::new(FakeStore::default());
        let source = Arc::new(FakeSource::with_unproven_page_order(true));
        store.seed_message("acc-1", "boundary", "g1").await;
        store.state.lock().unwrap().known_scopes = vec![known_scope(
            "g1",
            Some(cursor("acc-1", "boundary", "boundary-seq")),
        )];
        store.state.lock().unwrap().claimable.push_back(claimed_gap(
            "run-untrusted-order",
            "gap-untrusted-order",
            "acc-1",
        ));
        source.queue(page(
            vec![
                hist_item("acc-1", "boundary", "boundary-seq", "g1"),
                hist_item("acc-1", "gap-message", "opaque-gap", "g1"),
            ],
            BackfillContinuation::ProvenHistoryStart,
        ));

        let outcome = build(store.clone(), source, budget())
            .run_one()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(outcome.completeness, HistoryCompleteness::Unprovable);
        assert!(!outcome.evidence.scopes[0].reached_boundary);
        assert!(
            outcome.evidence.scopes[0]
                .anomalies
                .contains(&BackfillAnomaly::UntrustedPageOrder)
        );
        assert!(store.has_message("acc-1", "gap-message", "g1"));
    }

    #[tokio::test]
    async fn unproven_page_order_without_boundary_reports_only_the_order_gap() {
        let store = Arc::new(FakeStore::default());
        let source = Arc::new(FakeSource::with_unproven_page_order(true));
        store.state.lock().unwrap().known_scopes = vec![known_scope("g1", None)];
        store.state.lock().unwrap().claimable.push_back(claimed_gap(
            "run-untrusted-start",
            "gap-untrusted-start",
            "acc-1",
        ));
        source.queue(page(
            vec![hist_item("acc-1", "oldest", "opaque-oldest", "g1")],
            BackfillContinuation::ProvenHistoryStart,
        ));

        let outcome = build(store, source, budget())
            .run_one()
            .await
            .unwrap()
            .unwrap();
        let anomalies = &outcome.evidence.scopes[0].anomalies;
        assert!(anomalies.contains(&BackfillAnomaly::UntrustedPageOrder));
        assert!(!anomalies.contains(&BackfillAnomaly::BoundaryNotFound));
    }

    #[tokio::test]
    async fn empty_page_and_unproven_stop_are_both_anomalies() {
        let store = Arc::new(FakeStore::default());
        let source = Arc::new(FakeSource::new(true));
        store.state.lock().unwrap().known_scopes = vec![known_scope("g1", None)];
        store.state.lock().unwrap().claimable.push_back(claimed_gap(
            "run-empty",
            "gap-empty",
            "acc-1",
        ));
        source.queue(page(Vec::new(), BackfillContinuation::UnprovenStop));

        let outcome = build(store, source, budget())
            .run_one()
            .await
            .unwrap()
            .unwrap();
        let anomalies = &outcome.evidence.scopes[0].anomalies;
        assert!(anomalies.contains(&BackfillAnomaly::EmptyPage));
        assert!(anomalies.contains(&BackfillAnomaly::UnprovenStop));
        assert_eq!(outcome.completeness, HistoryCompleteness::Unprovable);
    }

    #[tokio::test]
    async fn nonempty_unproven_stop_never_proves_scope_complete() {
        let store = Arc::new(FakeStore::default());
        let source = Arc::new(FakeSource::new(true));
        store.state.lock().unwrap().known_scopes = vec![known_scope("g1", None)];
        store
            .state
            .lock()
            .unwrap()
            .claimable
            .push_back(claimed_gap("run-stop", "gap-stop", "acc-1"));
        source.queue(page(
            vec![hist_item("acc-1", "one", "opaque-one", "g1")],
            BackfillContinuation::UnprovenStop,
        ));

        let outcome = build(store, source, budget())
            .run_one()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.completeness, HistoryCompleteness::Unprovable);
        assert!(
            outcome.evidence.scopes[0]
                .anomalies
                .contains(&BackfillAnomaly::UnprovenStop)
        );
    }

    #[tokio::test]
    async fn short_page_with_next_continues_until_explicit_start_evidence() {
        let store = Arc::new(FakeStore::default());
        let source = Arc::new(FakeSource::new(true));
        store.state.lock().unwrap().known_scopes = vec![known_scope("g1", None)];
        store.state.lock().unwrap().claimable.push_back(claimed_gap(
            "run-short",
            "gap-short",
            "acc-1",
        ));
        let opaque = cursor("acc-1", "short-anchor", "not-a-number");
        source.queue(page(
            vec![hist_item("acc-1", "short-anchor", "not-a-number", "g1")],
            BackfillContinuation::Next(opaque.clone()),
        ));
        source.queue(page(
            vec![
                hist_item("acc-1", "short-anchor", "not-a-number", "g1"),
                hist_item("acc-1", "oldest", "opaque-oldest", "g1"),
            ],
            BackfillContinuation::ProvenHistoryStart,
        ));

        let outcome = build(store, source.clone(), budget())
            .run_one()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(source.fetch_log(), vec![None, Some(opaque)]);
        assert_eq!(outcome.completeness, HistoryCompleteness::ProvenComplete);
    }

    #[tokio::test]
    async fn duplicate_page_and_same_cursor_produce_typed_anomalies() {
        let store = Arc::new(FakeStore::default());
        let source = Arc::new(FakeSource::new(true));
        store.state.lock().unwrap().known_scopes = vec![known_scope("g1", None)];
        store.state.lock().unwrap().claimable.push_back(claimed_gap(
            "run-repeat",
            "gap-repeat",
            "acc-1",
        ));
        let same = cursor("acc-1", "b", "opaque-b");
        let repeated = vec![
            hist_item("acc-1", "a", "opaque-a", "g1"),
            hist_item("acc-1", "b", "opaque-b", "g1"),
        ];
        source.queue(page(
            repeated.clone(),
            BackfillContinuation::Next(same.clone()),
        ));
        source.queue(page(repeated, BackfillContinuation::Next(same)));

        let outcome = build(store, source, budget())
            .run_one()
            .await
            .unwrap()
            .unwrap();
        let anomalies = &outcome.evidence.scopes[0].anomalies;
        assert!(anomalies.contains(&BackfillAnomaly::DuplicatePage));
        assert!(anomalies.contains(&BackfillAnomaly::NoCursorAdvance));
        assert!(anomalies.contains(&BackfillAnomaly::SortConflict));
    }

    #[tokio::test]
    async fn illegal_next_cursor_cross_account_cursor_and_empty_anchor_are_rejected() {
        async fn run_case(
            run_id: &str,
            page_value: BackfillPage,
            persisted_cursor: Option<BackfillCursor>,
        ) -> BackfillOutcome {
            let store = Arc::new(FakeStore::default());
            let source = Arc::new(FakeSource::new(true));
            store.state.lock().unwrap().known_scopes = vec![known_scope("g1", None)];
            store
                .state
                .lock()
                .unwrap()
                .reclaimed
                .push(claimed_gap_resume(run_id, "gap-invalid", "acc-1"));
            if let Some(last_cursor) = persisted_cursor {
                store.set_progress(
                    run_id,
                    ScopeProgress {
                        conversation: ConversationRef::new(ConversationKind::Group, "g1").unwrap(),
                        status: BackfillScopeStatus::Backfilling,
                        last_cursor: Some(last_cursor),
                        ..fresh()
                    },
                );
            }
            source.queue(page_value);
            let use_case = build(store, source, budget());
            let claimed = use_case.reclaim_expired(1).await.unwrap().remove(0);
            use_case.resume_claimed(claimed).await.unwrap()
        }

        let invalid_next = run_case(
            "run-invalid-next",
            page(
                vec![hist_item("acc-1", "actual", "opaque-actual", "g1")],
                BackfillContinuation::Next(cursor("acc-1", "fabricated", "opaque-fake")),
            ),
            None,
        )
        .await;
        assert!(
            invalid_next.evidence.scopes[0]
                .anomalies
                .contains(&BackfillAnomaly::InvalidContinuation)
        );

        let cross_account_next = run_case(
            "run-cross-account-next",
            page(
                vec![hist_item("acc-1", "actual", "opaque-actual", "g1")],
                BackfillContinuation::Next(cursor("other-account", "actual", "opaque-actual")),
            ),
            None,
        )
        .await;
        assert!(
            cross_account_next.evidence.scopes[0]
                .anomalies
                .contains(&BackfillAnomaly::CursorAccountMismatch)
        );

        let cross_account = run_case(
            "run-cross-account",
            page(Vec::new(), BackfillContinuation::UnprovenStop),
            Some(cursor("other-account", "cursor", "opaque")),
        )
        .await;
        assert!(
            cross_account.evidence.scopes[0]
                .anomalies
                .contains(&BackfillAnomaly::CursorAccountMismatch)
        );

        let empty_anchor = run_case(
            "run-empty-anchor",
            page(
                vec![hist_item("acc-1", "message", "", "g1")],
                BackfillContinuation::UnprovenStop,
            ),
            None,
        )
        .await;
        assert!(
            empty_anchor.evidence.scopes[0]
                .anomalies
                .contains(&BackfillAnomaly::EmptyAnchor)
        );

        let wrong_message_scope = run_case(
            "run-wrong-message-scope",
            page(
                vec![hist_item(
                    "other-account",
                    "message",
                    "opaque-message",
                    "g1",
                )],
                BackfillContinuation::ProvenHistoryStart,
            ),
            None,
        )
        .await;
        assert!(
            wrong_message_scope.evidence.scopes[0]
                .anomalies
                .contains(&BackfillAnomaly::MessageScopeMismatch)
        );
    }

    fn fresh() -> ScopeProgress {
        ScopeProgress {
            conversation: ConversationRef::new(ConversationKind::Group, "g1").unwrap(),
            status: BackfillScopeStatus::Pending,
            last_cursor: None,
            pages_read: 0,
            events_read: 0,
            accepted: 0,
            duplicates: 0,
            reached_boundary: false,
            anomalies: Vec::new(),
        }
    }
}
