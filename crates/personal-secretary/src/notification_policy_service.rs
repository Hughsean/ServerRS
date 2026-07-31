use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::{
    Clock, EvaluationInput, EvaluationPlan, EvaluationRequestId, MatchField, MessageRole,
    NotificationCandidateRef, NotificationMatchKeyV1, NotificationPolicyKind,
    NotificationPolicyRule, PolicyFamilyId, PolicyRevisionId, SourceAccountRef, SourceEventId,
};

/// 一个账号在同一快照中最多参与的策略 Family 数。上限避免异常配置扩大 worker 内存和提交 fencing。
pub const MAX_EVALUATION_POLICY_FAMILIES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerBindingSnapshot {
    pub managed_account: SourceAccountRef,
    pub command_account: SourceAccountRef,
    pub owner_actor_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRuleSnapshot {
    pub policy_family_id: PolicyFamilyId,
    pub policy_revision_id: PolicyRevisionId,
    pub generation: u64,
    pub policy_kind: NotificationPolicyKind,
    pub rule: NotificationPolicyRule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FamilyGenerationSnapshot {
    pub policy_family_id: PolicyFamilyId,
    pub current_revision_id: PolicyRevisionId,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationSnapshot {
    pub evaluation_request_id: EvaluationRequestId,
    pub candidate: NotificationCandidateRef,
    pub candidate_match_key: NotificationMatchKeyV1,
    /// 快照建立时候选仍可被求值；最终提交会再次验证该状态。
    pub candidate_is_current: bool,
    pub account_policy_epoch: u64,
    /// 所有当前 Head（包含 tombstone）都参与最终 fencing。
    pub family_generations: Vec<FamilyGenerationSnapshot>,
    /// 仅 current Head 为 rule 的已解析策略；其长度受 `MAX_EVALUATION_POLICY_FAMILIES` 限制。
    pub active_rules: Vec<PolicyRuleSnapshot>,
    /// 候选所属 managed account 与实际 Owner command account 必须独立保留。
    pub owner_binding: OwnerBindingSnapshot,
}

impl EvaluationSnapshot {
    /// 仅从已加载的快照建立 evaluator 输入；绝不在事务外回读策略或身份状态。
    pub fn evaluation_input(
        &self,
        now_unix_secs: i64,
    ) -> Result<EvaluationInput, NotificationPolicyUseCaseError> {
        let conversation = self.rule_for(NotificationPolicyKind::Conversation)?;
        Ok(EvaluationInput {
            candidate_is_current: self.candidate_is_current,
            conversation_rule: conversation.as_ref().and_then(|rule| rule.conversation),
            matching_rule: conversation,
            contact_rule: self.rule_for(NotificationPolicyKind::Contact)?,
            category_rule: self.rule_for(NotificationPolicyKind::Category)?,
            account_default_rule: self.rule_for(NotificationPolicyKind::AccountDefault)?,
            quiet_hours: self
                .configured_rule_for(NotificationPolicyKind::QuietHours)?
                .and_then(|rule| rule.quiet_hours),
            now_unix_secs,
        })
    }

    fn configured_rule_for(
        &self,
        kind: NotificationPolicyKind,
    ) -> Result<Option<NotificationPolicyRule>, NotificationPolicyUseCaseError> {
        let mut rules = self
            .active_rules
            .iter()
            .filter(|snapshot| snapshot.policy_kind == kind);
        let first = rules.next();
        if rules.next().is_some() {
            return Err(NotificationPolicyUseCaseError::AmbiguousPolicyRules);
        }
        Ok(first.map(|snapshot| snapshot.rule.clone()))
    }

    fn rule_for(
        &self,
        kind: NotificationPolicyKind,
    ) -> Result<Option<NotificationPolicyRule>, NotificationPolicyUseCaseError> {
        Ok(self
            .matching_rule_snapshot(kind)?
            .map(|snapshot| snapshot.rule.clone()))
    }

    /// 返回指定优先级唯一命中的当前 Head；提交审计借此记录实际生效 revision。
    pub fn matching_rule_snapshot(
        &self,
        kind: NotificationPolicyKind,
    ) -> Result<Option<&PolicyRuleSnapshot>, NotificationPolicyUseCaseError> {
        let mut matched = self.active_rules.iter().filter(|snapshot| {
            snapshot.policy_kind == kind
                && notification_match_key_matches(
                    &snapshot.rule.match_key,
                    &self.candidate_match_key,
                )
        });
        let first = matched.next();
        if matched.next().is_some() {
            return Err(NotificationPolicyUseCaseError::AmbiguousPolicyRules);
        }
        Ok(first)
    }
}

fn notification_match_key_matches(
    expected: &NotificationMatchKeyV1,
    actual: &NotificationMatchKeyV1,
) -> bool {
    expected.account == actual.account
        && expected.conversation.matches(&actual.conversation)
        && expected.actor_id.matches(&actual.actor_id)
        && expected.category.matches(&actual.category)
        && expected.mentioned_owner.matches(&actual.mentioned_owner)
        && expected
            .structured_importance
            .matches(&actual.structured_importance)
        && expected.event_kind.matches(&actual.event_kind)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedEvaluation {
    pub evaluation_request_id: EvaluationRequestId,
    pub lease_token: String,
    pub lease_expires_at_unix_secs: i64,
    pub attempt: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationCommit {
    pub claim: ClaimedEvaluation,
    pub snapshot: EvaluationSnapshot,
    pub plan: EvaluationPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationCommitResult {
    Applied,
    LeaseLost,
    SnapshotStale,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NotificationCandidateProductionReport {
    /// 本次事务新建的、由稳定来源版本唯一约束保护的候选数。
    pub candidates_created: u64,
    /// 本次事务新建的 generation-1 求值请求数。
    pub requests_created: u64,
    /// 已安全终态化的 legacy Outbox 数；仅 reconciliation 使用。
    pub legacy_outbox_suppressed: u64,
    /// 扫描时已不满足来源 freshness 的行数。
    pub sources_skipped_stale: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationFeedbackRequest {
    pub candidate: NotificationCandidateRef,
    pub match_key: NotificationMatchKeyV1,
    pub important: bool,
    pub promote_to_rule: bool,
    pub command_source_event_id: String,
}

/// 创建或替换策略时写入的完整不可变 Revision 内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationPolicyWriteRequest {
    pub account: SourceAccountRef,
    pub policy_family_id: Option<PolicyFamilyId>,
    pub canonical_scope_key: String,
    pub policy_kind: crate::NotificationPolicyKind,
    pub rule: crate::NotificationPolicyRule,
    pub command_source_event_id: Option<String>,
    pub audit_summary: String,
}

/// 停用通过追加 tombstone Revision 完成，永不原地修改历史 Revision。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationPolicyDisableRequest {
    pub account: SourceAccountRef,
    pub policy_family_id: PolicyFamilyId,
    pub expected_generation: u64,
    pub command_source_event_id: Option<String>,
    pub audit_summary: String,
}

/// Owner 策略控制面唯一允许进入响应的有界审计视图。
/// 不包含规则 JSON、命令正文或候选正文，避免把策略执行细节泄漏到回复工件。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NotificationPolicyResponseArtifact {
    pub scope: String,
    pub policy_family_id: Option<PolicyFamilyId>,
    pub policy_revision_id: Option<PolicyRevisionId>,
    pub decision_id: Option<crate::NotificationDecisionId>,
    pub status: String,
    pub priority: String,
    pub typed_reason: String,
    pub audit_reference: String,
}

/// 策略 Action 的事务化 Effect 输入。策略变更与通用 Effect Receipt 必须同一事务提交，
/// 否则进程在两次提交之间崩溃会造成“已写策略但没有成功回执”的不可恢复状态。
#[derive(Debug, Clone)]
pub struct NotificationPolicyEffectRequest {
    pub account: SourceAccountRef,
    pub command_source_event_id: SourceEventId,
    pub run_id: String,
    pub effect_id: String,
    pub proposal_id: String,
    pub proposal_json: String,
    pub lease_token: String,
    pub action: crate::SecretaryAction,
}

#[async_trait]
pub trait NotificationPolicyStoreT: Send + Sync {
    /// 基于持久化 OwnerCommand 与 active Owner binding 构造授权上下文。
    /// 不允许执行器相信模型、审批消息或 ActionRun 中未经验证的身份字段。
    async fn authorization_for_owner_command(
        &self,
        target_account: &SourceAccountRef,
        command_source_event_id: &crate::SourceEventId,
    ) -> Result<NotificationPolicyAuthorizationContext, NotificationPolicyStoreError>;

    async fn create_or_replace(
        &self,
        request: &NotificationPolicyWriteRequest,
    ) -> Result<crate::NotificationPolicyFamily, NotificationPolicyStoreError>;

    async fn disable(
        &self,
        request: &NotificationPolicyDisableRequest,
    ) -> Result<crate::NotificationPolicyFamily, NotificationPolicyStoreError>;

    async fn record_feedback(
        &self,
        request: &NotificationFeedbackRequest,
    ) -> Result<(), NotificationPolicyStoreError>;

    /// 原子提交可变策略 Action 与通用 Effect Receipt。仓储必须在同一个数据库事务中
    /// 校验 Action 租约、写入策略数据和写入 Receipt；不得由调用方拆分这三个步骤。
    async fn apply_effect(
        &self,
        request: &NotificationPolicyEffectRequest,
    ) -> Result<crate::SecretaryActionReceipt, NotificationPolicyStoreError>;

    /// 返回账号内当前策略 Head 的有界列表视图。
    async fn list_policy_artifacts(
        &self,
        account: &SourceAccountRef,
        limit: u16,
    ) -> Result<Vec<NotificationPolicyResponseArtifact>, NotificationPolicyStoreError>;

    /// 返回一个已提交决策的有界解释视图；跨账号或未知 ID 均不得泄漏。
    async fn explain_decision_artifact(
        &self,
        account: &SourceAccountRef,
        decision_id: &crate::NotificationDecisionId,
    ) -> Result<Option<NotificationPolicyResponseArtifact>, NotificationPolicyStoreError>;

    async fn claim_evaluation(
        &self,
        worker_id: &str,
        now_unix_secs: i64,
        lease_secs: u64,
    ) -> Result<Option<ClaimedEvaluation>, NotificationPolicyStoreError>;

    /// 此调用只读取已持久化快照；调用方随后在事务外执行纯领域求值。
    async fn load_evaluation_snapshot(
        &self,
        claim: &ClaimedEvaluation,
    ) -> Result<EvaluationSnapshot, NotificationPolicyStoreError>;

    async fn commit_evaluation(
        &self,
        commit: &EvaluationCommit,
    ) -> Result<EvaluationCommitResult, NotificationPolicyStoreError>;

    async fn recover_expired_evaluations(
        &self,
        now_unix_secs: i64,
        limit: u32,
    ) -> Result<u64, NotificationPolicyStoreError>;

    /// 自动回复拒绝策略仅按账号与可信 actor 精确查询；存储错误由调用方按拒绝处理。
    async fn automatic_reply_is_denied(
        &self,
        account: &SourceAccountRef,
        actor_id: &str,
    ) -> Result<bool, NotificationPolicyStoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationPolicyAuthorizationContext {
    pub owner_binding_account: SourceAccountRef,
    pub owner_actor_id: String,
    pub command_account: SourceAccountRef,
    pub command_actor_id: String,
    pub command_role: MessageRole,
    pub target_account: SourceAccountRef,
}

/// 对 Planner、Action Gate、Resume 与 Effect 共用的最终 Owner 授权约束。
pub fn authorize_notification_policy_action(
    context: &NotificationPolicyAuthorizationContext,
) -> Result<(), NotificationPolicyUseCaseError> {
    if context.command_role != MessageRole::OwnerCommand {
        return Err(NotificationPolicyUseCaseError::OwnerCommandRequired);
    }
    if context.owner_actor_id.trim().is_empty()
        || context.command_actor_id.trim().is_empty()
        || context.owner_actor_id != context.command_actor_id
    {
        return Err(NotificationPolicyUseCaseError::OwnerBindingMismatch);
    }
    // OwnerCommand 可来自 QQ Open Platform，而被管理账号可来自 NapCat；二者由持久化
    // OwnerBinding 关联，不能错误地要求 command account 与 managed account 相同。
    if context.owner_binding_account != context.target_account {
        return Err(NotificationPolicyUseCaseError::CrossAccountAction);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomaticReplyGateDecision {
    Allowed,
    Denied,
}

/// 自动回复拒绝与通知求值隔离；身份或存储状态无法证明安全时一律拒绝。
#[async_trait]
pub trait AutomaticReplyPolicyGate: Send + Sync {
    async fn evaluate(
        &self,
        account: &SourceAccountRef,
        actor_id: MatchField<String>,
    ) -> AutomaticReplyGateDecision;
}

/// 所有通知策略仓储均可直接作为持久化 Gate 注入执行器，避免旁路读取遗漏 fail-closed 约束。
#[async_trait]
impl<T> AutomaticReplyPolicyGate for T
where
    T: NotificationPolicyStoreT + ?Sized,
{
    async fn evaluate(
        &self,
        account: &SourceAccountRef,
        actor_id: MatchField<String>,
    ) -> AutomaticReplyGateDecision {
        let MatchField::Known(actor_id) = actor_id else {
            return AutomaticReplyGateDecision::Denied;
        };
        match self.automatic_reply_is_denied(account, &actor_id).await {
            Ok(false) => AutomaticReplyGateDecision::Allowed,
            Ok(true) | Err(_) => AutomaticReplyGateDecision::Denied,
        }
    }
}

pub struct NotificationPolicyUseCase {
    store: Arc<dyn NotificationPolicyStoreT>,
    clock: Arc<dyn Clock>,
}

impl NotificationPolicyUseCase {
    pub fn new(store: Arc<dyn NotificationPolicyStoreT>, clock: Arc<dyn Clock>) -> Self {
        Self { store, clock }
    }

    /// 从持久化身份记录获取授权上下文，并在用例层再次执行统一约束。
    pub async fn authorization_for_owner_command(
        &self,
        target_account: &SourceAccountRef,
        command_source_event_id: &crate::SourceEventId,
    ) -> Result<NotificationPolicyAuthorizationContext, NotificationPolicyUseCaseError> {
        let context = self
            .store
            .authorization_for_owner_command(target_account, command_source_event_id)
            .await?;
        authorize_notification_policy_action(&context)?;
        Ok(context)
    }

    /// 写入策略前在应用层集中执行 Owner 授权与静默时段 DST 预检，
    /// 仓储只负责不可变 Revision 与 generation CAS。
    pub async fn create_or_replace(
        &self,
        authorization: &NotificationPolicyAuthorizationContext,
        request: &NotificationPolicyWriteRequest,
    ) -> Result<crate::NotificationPolicyFamily, NotificationPolicyUseCaseError> {
        authorize_notification_policy_action(authorization)?;
        if authorization.target_account != request.account {
            return Err(NotificationPolicyUseCaseError::CrossAccountAction);
        }
        if let Some(quiet_hours) = request.rule.quiet_hours.as_ref() {
            crate::validate_quiet_hours(quiet_hours, self.clock.as_ref())?;
        }
        Ok(self.store.create_or_replace(request).await?)
    }

    /// 停用只追加 tombstone Revision；调用方必须携带当前 Family generation。
    pub async fn disable(
        &self,
        authorization: &NotificationPolicyAuthorizationContext,
        request: &NotificationPolicyDisableRequest,
    ) -> Result<crate::NotificationPolicyFamily, NotificationPolicyUseCaseError> {
        authorize_notification_policy_action(authorization)?;
        if authorization.target_account != request.account {
            return Err(NotificationPolicyUseCaseError::CrossAccountAction);
        }
        Ok(self.store.disable(request).await?)
    }

    pub async fn record_feedback(
        &self,
        request: &NotificationFeedbackRequest,
    ) -> Result<(), NotificationPolicyUseCaseError> {
        if request.promote_to_rule {
            request.match_key.eligibility_for_long_term_rule()?;
        }
        self.store.record_feedback(request).await?;
        Ok(())
    }

    /// 反馈是可撤销的 L1 记录，但仍不得绕过 OwnerCommand 的账号授权。
    pub async fn record_authorized_feedback(
        &self,
        authorization: &NotificationPolicyAuthorizationContext,
        request: &NotificationFeedbackRequest,
    ) -> Result<(), NotificationPolicyUseCaseError> {
        authorize_notification_policy_action(authorization)?;
        if authorization.target_account != request.candidate.account
            || request.candidate.account != request.match_key.account
        {
            return Err(NotificationPolicyUseCaseError::CrossAccountAction);
        }
        self.record_feedback(request).await
    }

    /// 原子执行会改变策略状态的 Action。授权检查在应用层完成，仓储随后把策略写入与
    /// Effect Receipt 放入同一事务，成功响应只能建立在这个回执之上。
    pub async fn apply_effect(
        &self,
        authorization: &NotificationPolicyAuthorizationContext,
        request: &NotificationPolicyEffectRequest,
    ) -> Result<crate::SecretaryActionReceipt, NotificationPolicyUseCaseError> {
        authorize_notification_policy_action(authorization)?;
        if authorization.target_account != request.account {
            return Err(NotificationPolicyUseCaseError::CrossAccountAction);
        }
        if let crate::SecretaryAction::SetQuietHours { quiet_hours, .. } = &request.action {
            crate::validate_quiet_hours(quiet_hours, self.clock.as_ref())?;
        }
        self.store.apply_effect(request).await.map_err(Into::into)
    }

    /// 仅供 EffectExecutor 在预检授权已失效时读取精确历史回执或执行仓储最终授权。
    /// 仓储会在同一事务内校验 OwnerBinding 与运行租约，因此此路径不能绕过新写入授权。
    pub async fn apply_effect_with_repository_fencing(
        &self,
        request: &NotificationPolicyEffectRequest,
    ) -> Result<crate::SecretaryActionReceipt, NotificationPolicyUseCaseError> {
        if let crate::SecretaryAction::SetQuietHours { quiet_hours, .. } = &request.action {
            crate::validate_quiet_hours(quiet_hours, self.clock.as_ref())?;
        }
        self.store.apply_effect(request).await.map_err(Into::into)
    }

    /// 只读策略查询也必须绑定已验证 OwnerCommand，避免由普通聊天内容枚举配置。
    pub async fn list_authorized_policy_artifacts(
        &self,
        authorization: &NotificationPolicyAuthorizationContext,
        limit: u16,
    ) -> Result<Vec<NotificationPolicyResponseArtifact>, NotificationPolicyUseCaseError> {
        authorize_notification_policy_action(authorization)?;
        if !(1..=20).contains(&limit) {
            return Err(NotificationPolicyUseCaseError::InvalidListLimit);
        }
        Ok(self
            .store
            .list_policy_artifacts(&authorization.target_account, limit)
            .await?)
    }

    /// 决策说明限定于目标账号；未知决策返回 `None`，不暴露跨账号存在性。
    pub async fn explain_authorized_decision(
        &self,
        authorization: &NotificationPolicyAuthorizationContext,
        decision_id: &crate::NotificationDecisionId,
    ) -> Result<Option<NotificationPolicyResponseArtifact>, NotificationPolicyUseCaseError> {
        authorize_notification_policy_action(authorization)?;
        Ok(self
            .store
            .explain_decision_artifact(&authorization.target_account, decision_id)
            .await?)
    }

    pub async fn recover_expired_evaluations(
        &self,
        limit: u32,
    ) -> Result<u64, NotificationPolicyUseCaseError> {
        if !(1..=1000).contains(&limit) {
            return Err(NotificationPolicyUseCaseError::InvalidRecoveryLimit);
        }
        Ok(self
            .store
            .recover_expired_evaluations(self.clock.now_unix_secs(), limit)
            .await?)
    }

    /// 领取和提交均由仓储短事务完成；本方法刻意不持有事务跨越纯求值。
    pub async fn evaluate_next(
        &self,
        worker_id: &str,
        lease_secs: u64,
        evaluator: impl FnOnce(&EvaluationSnapshot) -> EvaluationPlan,
    ) -> Result<Option<EvaluationCommitResult>, NotificationPolicyUseCaseError> {
        let now = self.clock.now_unix_secs();
        let Some(claim) = self
            .store
            .claim_evaluation(worker_id, now, lease_secs)
            .await?
        else {
            return Ok(None);
        };
        let snapshot = self.store.load_evaluation_snapshot(&claim).await?;
        let plan = evaluator(&snapshot);
        let commit = EvaluationCommit {
            claim,
            snapshot,
            plan,
        };
        Ok(Some(self.store.commit_evaluation(&commit).await?))
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum NotificationPolicyStoreError {
    #[error("notification policy authorization is no longer valid")]
    Unauthorized,
    #[error("notification policy storage is unavailable")]
    Unavailable,
    #[error("notification policy storage rejected the request")]
    Conflict,
    #[error("notification policy storage failed")]
    Database,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum NotificationPolicyUseCaseError {
    #[error(transparent)]
    Policy(#[from] crate::NotificationPolicyError),
    #[error(transparent)]
    Store(#[from] NotificationPolicyStoreError),
    #[error("notification policy action requires a verified OwnerCommand")]
    OwnerCommandRequired,
    #[error("notification policy action owner binding does not match the command")]
    OwnerBindingMismatch,
    #[error("notification policy action cannot target another account")]
    CrossAccountAction,
    #[error("notification policy recovery limit must be between 1 and 1000")]
    InvalidRecoveryLimit,
    #[error("notification policy list limit must be between 1 and 20")]
    InvalidListLimit,
    #[error("multiple current notification rules match the same evaluation priority")]
    AmbiguousPolicyRules,
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::{EventKind, MessageSource, NotificationCategory, StructuredImportance};

    struct FixedClock(i64);

    impl Clock for FixedClock {
        fn now_unix_secs(&self) -> i64 {
            self.0
        }
    }

    struct FakeStore {
        feedback_calls: Mutex<u32>,
        automatic_reply_result: Mutex<Result<bool, NotificationPolicyStoreError>>,
    }

    impl Default for FakeStore {
        fn default() -> Self {
            Self {
                feedback_calls: Mutex::new(0),
                automatic_reply_result: Mutex::new(Ok(true)),
            }
        }
    }

    #[async_trait]
    impl NotificationPolicyStoreT for FakeStore {
        async fn authorization_for_owner_command(
            &self,
            _target_account: &SourceAccountRef,
            _command_source_event_id: &crate::SourceEventId,
        ) -> Result<NotificationPolicyAuthorizationContext, NotificationPolicyStoreError> {
            Err(NotificationPolicyStoreError::Conflict)
        }

        async fn create_or_replace(
            &self,
            _request: &NotificationPolicyWriteRequest,
        ) -> Result<crate::NotificationPolicyFamily, NotificationPolicyStoreError> {
            unreachable!("feedback test store never writes policies")
        }

        async fn disable(
            &self,
            _request: &NotificationPolicyDisableRequest,
        ) -> Result<crate::NotificationPolicyFamily, NotificationPolicyStoreError> {
            unreachable!("feedback test store never writes policies")
        }

        async fn record_feedback(
            &self,
            _request: &NotificationFeedbackRequest,
        ) -> Result<(), NotificationPolicyStoreError> {
            *self.feedback_calls.lock().unwrap() += 1;
            Ok(())
        }

        async fn apply_effect(
            &self,
            _request: &NotificationPolicyEffectRequest,
        ) -> Result<crate::SecretaryActionReceipt, NotificationPolicyStoreError> {
            Err(NotificationPolicyStoreError::Conflict)
        }

        async fn list_policy_artifacts(
            &self,
            _account: &SourceAccountRef,
            _limit: u16,
        ) -> Result<Vec<NotificationPolicyResponseArtifact>, NotificationPolicyStoreError> {
            Ok(Vec::new())
        }

        async fn explain_decision_artifact(
            &self,
            _account: &SourceAccountRef,
            _decision_id: &crate::NotificationDecisionId,
        ) -> Result<Option<NotificationPolicyResponseArtifact>, NotificationPolicyStoreError>
        {
            Ok(None)
        }

        async fn claim_evaluation(
            &self,
            _worker_id: &str,
            _now_unix_secs: i64,
            _lease_secs: u64,
        ) -> Result<Option<ClaimedEvaluation>, NotificationPolicyStoreError> {
            Ok(None)
        }

        async fn load_evaluation_snapshot(
            &self,
            _claim: &ClaimedEvaluation,
        ) -> Result<EvaluationSnapshot, NotificationPolicyStoreError> {
            unreachable!("test store never claims an evaluation")
        }

        async fn commit_evaluation(
            &self,
            _commit: &EvaluationCommit,
        ) -> Result<EvaluationCommitResult, NotificationPolicyStoreError> {
            unreachable!("test store never claims an evaluation")
        }

        async fn recover_expired_evaluations(
            &self,
            _now_unix_secs: i64,
            _limit: u32,
        ) -> Result<u64, NotificationPolicyStoreError> {
            Ok(0)
        }

        async fn automatic_reply_is_denied(
            &self,
            _account: &SourceAccountRef,
            _actor_id: &str,
        ) -> Result<bool, NotificationPolicyStoreError> {
            self.automatic_reply_result.lock().unwrap().clone()
        }
    }

    fn account() -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, "account-1").unwrap()
    }

    fn feedback_with_unknown_metadata() -> NotificationFeedbackRequest {
        NotificationFeedbackRequest {
            candidate: NotificationCandidateRef::new("agenda", "item-1", 1, account()).unwrap(),
            match_key: NotificationMatchKeyV1::new(
                account(),
                MatchField::Unknown,
                MatchField::Known("actor-1".into()),
                MatchField::Known(NotificationCategory::Agenda),
                MatchField::Known(false),
                MatchField::Known(StructuredImportance::Normal),
                MatchField::Known(EventKind::AgendaDue),
            )
            .unwrap(),
            important: false,
            promote_to_rule: true,
            command_source_event_id: "event-1".into(),
        }
    }

    #[tokio::test]
    async fn use_case_refuses_feedback_promotion_when_match_metadata_is_unknown() {
        let store = Arc::new(FakeStore::default());
        let use_case = NotificationPolicyUseCase::new(store.clone(), Arc::new(FixedClock(0)));
        let result = use_case
            .record_feedback(&feedback_with_unknown_metadata())
            .await;
        assert_eq!(
            result,
            Err(NotificationPolicyUseCaseError::Policy(
                crate::NotificationPolicyError::UnknownMatchMetadata
            ))
        );
        assert_eq!(*store.feedback_calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn feedback_with_unknown_metadata_is_recorded_without_rule_promotion() {
        let store = Arc::new(FakeStore::default());
        let use_case = NotificationPolicyUseCase::new(store.clone(), Arc::new(FixedClock(0)));
        let mut request = feedback_with_unknown_metadata();
        request.promote_to_rule = false;

        assert_eq!(use_case.record_feedback(&request).await, Ok(()));
        assert_eq!(*store.feedback_calls.lock().unwrap(), 1);
    }

    #[test]
    fn authorization_requires_owner_command_and_matching_account() {
        let context = NotificationPolicyAuthorizationContext {
            owner_binding_account: account(),
            owner_actor_id: "owner-1".into(),
            command_account: account(),
            command_actor_id: "owner-1".into(),
            command_role: MessageRole::OwnerCommand,
            target_account: account(),
        };
        assert_eq!(authorize_notification_policy_action(&context), Ok(()));

        let mut cross_account = context;
        cross_account.target_account =
            SourceAccountRef::new(MessageSource::NapCat, "account-2").unwrap();
        assert_eq!(
            authorize_notification_policy_action(&cross_account),
            Err(NotificationPolicyUseCaseError::CrossAccountAction)
        );
    }

    #[tokio::test]
    async fn persisted_automatic_reply_gate_fails_closed_for_unknown_or_storage_failure() {
        let store = FakeStore::default();
        assert_eq!(
            AutomaticReplyPolicyGate::evaluate(&store, &account(), MatchField::Unknown).await,
            AutomaticReplyGateDecision::Denied
        );
        assert_eq!(
            AutomaticReplyPolicyGate::evaluate(
                &store,
                &account(),
                MatchField::Known("actor-1".into())
            )
            .await,
            AutomaticReplyGateDecision::Denied
        );
        *store.automatic_reply_result.lock().unwrap() = Err(NotificationPolicyStoreError::Database);
        assert_eq!(
            AutomaticReplyPolicyGate::evaluate(
                &store,
                &account(),
                MatchField::Known("actor-1".into())
            )
            .await,
            AutomaticReplyGateDecision::Denied
        );
        *store.automatic_reply_result.lock().unwrap() = Ok(false);
        assert_eq!(
            AutomaticReplyPolicyGate::evaluate(
                &store,
                &account(),
                MatchField::Known("actor-1".into())
            )
            .await,
            AutomaticReplyGateDecision::Allowed
        );
    }

    #[tokio::test]
    async fn automatic_reply_gate_contract_requires_fail_closed_implementation() {
        struct DenyGate;
        #[async_trait]
        impl AutomaticReplyPolicyGate for DenyGate {
            async fn evaluate(
                &self,
                _account: &SourceAccountRef,
                _actor_id: MatchField<String>,
            ) -> AutomaticReplyGateDecision {
                AutomaticReplyGateDecision::Denied
            }
        }
        assert_eq!(
            DenyGate.evaluate(&account(), MatchField::Unknown).await,
            AutomaticReplyGateDecision::Denied
        );
    }
}
