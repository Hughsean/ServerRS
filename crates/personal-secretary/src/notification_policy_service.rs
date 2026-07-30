use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::{
    Clock, EvaluationInput, EvaluationPlan, EvaluationRequestId, MatchField, MessageRole,
    NotificationCandidateRef, NotificationMatchKeyV1, NotificationPolicyKind,
    NotificationPolicyRule, PolicyFamilyId, PolicyRevisionId, SourceAccountRef,
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
        let matching = |kind| self.rule_for(kind);
        Ok(EvaluationInput {
            candidate_is_current: self.candidate_is_current,
            matching_rule: matching(NotificationPolicyKind::Conversation)?,
            conversation_rule: None,
            contact_rule: matching(NotificationPolicyKind::Contact)?,
            category_rule: matching(NotificationPolicyKind::Category)?,
            account_default_rule: matching(NotificationPolicyKind::AccountDefault)?,
            // v1 的持久化 rule JSON 是 NotificationPolicyRule；QuietHoursRule 由后续专用
            // schema 接入前不从未验证 JSON 猜测，保持无静默时间规则。
            quiet_hours: None,
            now_unix_secs,
        })
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

#[async_trait]
pub trait NotificationPolicyStoreT: Send + Sync {
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
    if context.owner_binding_account != context.command_account
        || context.command_account != context.target_account
    {
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
