//! Retriever 用例编排。封装 RetrieverStoreT，提供内容策略过滤和指代解析。
//!
//! 不直接调用 LLM；Planner 调用本服务获取已过滤的检索结果。

use std::sync::Arc;

use thiserror::Error;

use crate::planner::AgentEventView;
use crate::{
    AccountScopedParticipantRef, Clock, CommitmentQuery, CommitmentSummary, ContentTrustLevel,
    ConversationRef, EventCausalContextView, EventQuery, EventSearchResult, EventThreadId,
    InboundEventStoreError, ParticipantContextView, PendingOwnerWorkItem, ProjectContextView,
    ProjectMemorySummary, ReferenceContext, ReferenceResolution, RetrieverError, RetrieverStoreT,
    SecretaryStatusView, SourceAccountRef, SourceEventDetail, SourceEventId, SystemClock,
    ThreadContextView, ThreadSearchResult, UpcomingItem, check_causal_role_strictness,
    check_participant_permission_boundary, filter_for_model, resolve_reference_from_candidates,
    validate_causal_context, validate_event_query, validate_participant_context,
};

/// Retriever 内容策略（约束 6/7）。默认 `allow_local_only_to_loopback_llm = false`（保守安全）。
#[derive(Debug, Clone, Default)]
pub struct RetrieverPolicy {
    /// 是否允许 local_only 内容进入本地 loopback LLM。默认 false（安全）。
    pub allow_local_only_to_loopback_llm: bool,
}

/// Retriever 用例错误。
#[derive(Debug, Error)]
pub enum RetrieverUseCaseError {
    #[error(transparent)]
    Store(#[from] InboundEventStoreError),
    #[error(transparent)]
    Domain(#[from] RetrieverError),
    #[error("invalid retriever input: {0}")]
    InvalidInput(String),
}

/// Retriever 用例。封装 Store + 内容策略过滤。
pub struct RetrieverUseCase {
    store: Arc<dyn RetrieverStoreT>,
    policy: RetrieverPolicy,
    clock: Arc<dyn Clock>,
}

impl RetrieverUseCase {
    pub fn new(store: Arc<dyn RetrieverStoreT>, policy: RetrieverPolicy) -> Self {
        Self {
            store,
            policy,
            clock: Arc::new(SystemClock),
        }
    }

    pub fn with_clock(
        store: Arc<dyn RetrieverStoreT>,
        policy: RetrieverPolicy,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            store,
            policy,
            clock,
        }
    }

    /// 列出账号最近的事件证据视图，应用内容策略过滤。
    /// `is_local_loopback` 决定 local_only 事件是否可见；非 loopback 时排除。
    pub async fn list_recent_event_views(
        &self,
        account: &SourceAccountRef,
        limit: u16,
        is_local_loopback: bool,
    ) -> Result<Vec<AgentEventView>, RetrieverUseCaseError> {
        let views = self.store.list_recent_event_views(account, limit).await?;
        Ok(views
            .into_iter()
            .filter(|v| {
                crate::is_allowed_for_model(
                    v.content_trust_level,
                    is_local_loopback,
                    self.policy.allow_local_only_to_loopback_llm,
                )
            })
            .collect())
    }

    /// 检索事件并按内容策略过滤。
    /// `is_local_loopback` 由已验证的 LLM 配置生成，不能由调用方传入（约束 6）。
    pub async fn search_events(
        &self,
        query: &EventQuery,
        is_local_loopback: bool,
    ) -> Result<Vec<EventSearchResult>, RetrieverUseCaseError> {
        validate_event_query(query).map_err(RetrieverUseCaseError::Domain)?;
        let results = self.store.search_events(query).await?;
        Ok(filter_for_model(
            results,
            is_local_loopback,
            self.policy.allow_local_only_to_loopback_llm,
        ))
    }

    pub async fn read_source_event(
        &self,
        event_id: &crate::SourceEventId,
        account: &SourceAccountRef,
    ) -> Result<Option<SourceEventDetail>, RetrieverUseCaseError> {
        Ok(self.store.read_source_event(event_id, account).await?)
    }

    pub async fn search_threads(
        &self,
        account: &SourceAccountRef,
        query_text: &str,
        limit: u16,
    ) -> Result<Vec<ThreadSearchResult>, RetrieverUseCaseError> {
        if query_text.trim().is_empty() {
            return Err(RetrieverUseCaseError::InvalidInput(
                "query_text must not be empty".into(),
            ));
        }
        if !(1..=100).contains(&limit) {
            return Err(RetrieverUseCaseError::InvalidInput(
                "limit must be in 1..=100".into(),
            ));
        }
        Ok(self
            .store
            .search_threads(account, query_text, limit)
            .await?)
    }

    pub async fn list_upcoming(
        &self,
        account: &SourceAccountRef,
        horizon_secs: u64,
    ) -> Result<Vec<UpcomingItem>, RetrieverUseCaseError> {
        if horizon_secs == 0 || horizon_secs > 31_536_000 {
            return Err(RetrieverUseCaseError::InvalidInput(
                "horizon_secs must be in 1..=31536000".into(),
            ));
        }
        Ok(self.store.list_upcoming(account, horizon_secs).await?)
    }

    pub async fn secretary_status(
        &self,
        account: &SourceAccountRef,
    ) -> Result<SecretaryStatusView, RetrieverUseCaseError> {
        Ok(self.store.secretary_status(account).await?)
    }

    pub async fn list_pending_owner_work(
        &self,
        account: &SourceAccountRef,
        limit: u16,
    ) -> Result<Vec<PendingOwnerWorkItem>, RetrieverUseCaseError> {
        if !(1..=20).contains(&limit) {
            return Err(RetrieverUseCaseError::InvalidInput(
                "pending owner work limit must be in 1..=20".into(),
            ));
        }
        Ok(self.store.list_pending_owner_work(account, limit).await?)
    }

    pub async fn thread_context(
        &self,
        account: &SourceAccountRef,
        thread_id: &EventThreadId,
    ) -> Result<Option<ThreadContextView>, RetrieverUseCaseError> {
        Ok(self.store.thread_context(account, thread_id).await?)
    }

    /// 构建单事件的账号作用域因果上下文（THR-011/THR-012）。
    /// 结果必须先通过有界校验与严格角色语义校验；违例 fail-closed，
    /// 绝不让低置信语义伪装成已确认角色进入调用方。
    pub async fn event_causal_context(
        &self,
        account: &SourceAccountRef,
        source_event_id: &SourceEventId,
    ) -> Result<Option<EventCausalContextView>, RetrieverUseCaseError> {
        let view = self
            .store
            .event_causal_context(account, source_event_id)
            .await?;
        if let Some(ref view) = view {
            validate_causal_context(view).map_err(RetrieverUseCaseError::Domain)?;
            let violations = check_causal_role_strictness(view);
            if !violations.is_empty() {
                return Err(RetrieverUseCaseError::Domain(RetrieverError::InvalidData(
                    format!(
                        "causal role strictness violations: {}",
                        violations.join("; ")
                    ),
                )));
            }
        }
        Ok(view)
    }

    /// 构建参与者的账号作用域上下文（ID-004/ID-005/MEM-002）。
    /// 权限边界校验 fail-closed：昵称/群角色/推断身份产生的权限描述会被拒绝。
    pub async fn participant_context(
        &self,
        account: &SourceAccountRef,
        actor_id: &str,
        conversation: Option<&ConversationRef>,
        thread_id: Option<&EventThreadId>,
    ) -> Result<Option<ParticipantContextView>, RetrieverUseCaseError> {
        if actor_id.trim().is_empty() {
            return Err(RetrieverUseCaseError::InvalidInput(
                "actor_id must not be empty".into(),
            ));
        }
        let view = self
            .store
            .participant_context(account, actor_id, conversation, thread_id)
            .await?;
        if let Some(ref view) = view {
            validate_participant_context(view).map_err(RetrieverUseCaseError::Domain)?;
            let violations = check_participant_permission_boundary(view);
            if !violations.is_empty() {
                return Err(RetrieverUseCaseError::Domain(RetrieverError::InvalidData(
                    format!(
                        "participant permission boundary violations: {}",
                        violations.join("; ")
                    ),
                )));
            }
        }
        Ok(view)
    }

    /// 按完整账号作用域参与者引用（账号 + 身份种类 + 稳定 ID）读取上下文。
    /// 调用方已解析出身份种类（如按名查询的唯一候选）时使用本方法，避免
    /// 宽松按 ID 查询在跨命名空间场景下的歧义拒绝。校验与权限边界同
    /// [`Self::participant_context`]。
    pub async fn participant_context_by_ref(
        &self,
        participant: &AccountScopedParticipantRef,
        conversation: Option<&ConversationRef>,
        thread_id: Option<&EventThreadId>,
    ) -> Result<Option<ParticipantContextView>, RetrieverUseCaseError> {
        if participant.stable_id().trim().is_empty() {
            return Err(RetrieverUseCaseError::InvalidInput(
                "participant stable_id must not be empty".into(),
            ));
        }
        let view = self
            .store
            .participant_context_by_ref(participant, conversation, thread_id)
            .await?;
        if let Some(ref view) = view {
            validate_participant_context(view).map_err(RetrieverUseCaseError::Domain)?;
            let violations = check_participant_permission_boundary(view);
            if !violations.is_empty() {
                return Err(RetrieverUseCaseError::Domain(RetrieverError::InvalidData(
                    format!(
                        "participant permission boundary violations: {}",
                        violations.join("; ")
                    ),
                )));
            }
        }
        Ok(view)
    }

    /// 按显示名/别名/群名片有界解析参与者候选（THR-013 复合查询第一阶段）。
    /// 只做指代解析，绝不用于授权；名称必须是有效的非空有界输入。
    pub async fn participants_by_display_name(
        &self,
        account: &SourceAccountRef,
        name: &str,
        conversation: Option<&ConversationRef>,
        thread_id: Option<&EventThreadId>,
        limit: u16,
    ) -> Result<Vec<AccountScopedParticipantRef>, RetrieverUseCaseError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 200 {
            return Err(RetrieverUseCaseError::InvalidInput(
                "name must be non-empty and bounded to 200 chars".into(),
            ));
        }
        let limit = limit.clamp(1, 5);
        Ok(self
            .store
            .participants_by_display_name(account, name, conversation, thread_id, limit)
            .await?)
    }

    /// 解析指代。Store 返回候选，用例层判定唯一/歧义/无结果。
    pub async fn resolve_reference(
        &self,
        account: &SourceAccountRef,
        expression: &str,
        context: &ReferenceContext,
    ) -> Result<ReferenceResolution, RetrieverUseCaseError> {
        if &context.account != account {
            return Err(RetrieverUseCaseError::InvalidInput(
                "reference context account must match the requested account".into(),
            ));
        }
        if expression.trim().is_empty() {
            return Err(RetrieverUseCaseError::InvalidInput(
                "expression must not be empty".into(),
            ));
        }
        let candidates = self
            .store
            .find_reference_candidates(account, expression, context)
            .await?;
        Ok(resolve_reference_from_candidates(candidates, context))
    }

    /// 判定内容信任级别是否允许进入模型（约束 6/7）。
    pub fn is_allowed_for_model(&self, trust: ContentTrustLevel, is_local_loopback: bool) -> bool {
        crate::is_allowed_for_model(
            trust,
            is_local_loopback,
            self.policy.allow_local_only_to_loopback_llm,
        )
    }

    /// 列出当前账号的所有活跃项目记忆（MEM-003 A2）。
    pub async fn list_projects(
        &self,
        account: &SourceAccountRef,
        limit: u16,
    ) -> Result<Vec<ProjectMemorySummary>, RetrieverUseCaseError> {
        if !(1..=20).contains(&limit) {
            return Err(RetrieverUseCaseError::InvalidInput(
                "project list limit must be in 1..=20".into(),
            ));
        }
        Ok(self.store.list_projects(account, limit).await?)
    }

    /// 查询单个项目的完整上下文（MEM-003 A2）。
    pub async fn query_project(
        &self,
        account: &SourceAccountRef,
        project_key: &str,
    ) -> Result<Option<ProjectContextView>, RetrieverUseCaseError> {
        if project_key.trim().is_empty() || project_key.chars().count() > 191 {
            return Err(RetrieverUseCaseError::InvalidInput(
                "project_key must be non-empty and bounded".into(),
            ));
        }
        Ok(self.store.query_project(account, project_key).await?)
    }

    /// 查询承诺记忆（MEM-004 B2）。
    pub async fn list_commitments(
        &self,
        query: &CommitmentQuery,
    ) -> Result<Vec<CommitmentSummary>, RetrieverUseCaseError> {
        if !(1..=100).contains(&query.limit) {
            return Err(RetrieverUseCaseError::InvalidInput(
                "commitment query limit must be in 1..=100".into(),
            ));
        }
        Ok(self.store.list_commitments(query).await?)
    }

    pub fn now_unix_secs(&self) -> i64 {
        self.clock.now_unix_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ConversationKind, ConversationRef, EventQuery, EventSearchResult, MessageRole,
        MessageSource, SourceAccountRef, SourceEventId, VerifiedActor, VerifiedActorKind,
    };
    use async_trait::async_trait;
    use std::sync::Mutex;

    fn account() -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, "account-1").unwrap()
    }

    struct FakeStore {
        events: Mutex<Vec<EventSearchResult>>,
    }

    #[async_trait]
    impl RetrieverStoreT for FakeStore {
        async fn search_events(
            &self,
            _query: &EventQuery,
        ) -> Result<Vec<EventSearchResult>, InboundEventStoreError> {
            Ok(self.events.lock().unwrap().clone())
        }
        async fn list_recent_event_views(
            &self,
            _account: &SourceAccountRef,
            _limit: u16,
        ) -> Result<Vec<AgentEventView>, InboundEventStoreError> {
            Ok(Vec::new())
        }
        async fn read_source_event(
            &self,
            _event_id: &SourceEventId,
            _account: &SourceAccountRef,
        ) -> Result<Option<SourceEventDetail>, InboundEventStoreError> {
            Ok(None)
        }
        async fn search_threads(
            &self,
            _account: &SourceAccountRef,
            _query_text: &str,
            _limit: u16,
        ) -> Result<Vec<ThreadSearchResult>, InboundEventStoreError> {
            Ok(Vec::new())
        }
        async fn find_reference_candidates(
            &self,
            _account: &SourceAccountRef,
            _expression: &str,
            _context: &ReferenceContext,
        ) -> Result<Vec<crate::ReferenceCandidate>, InboundEventStoreError> {
            Ok(Vec::new())
        }
        async fn list_upcoming(
            &self,
            _account: &SourceAccountRef,
            _horizon_secs: u64,
        ) -> Result<Vec<UpcomingItem>, InboundEventStoreError> {
            Ok(Vec::new())
        }
        async fn secretary_status(
            &self,
            _account: &SourceAccountRef,
        ) -> Result<SecretaryStatusView, InboundEventStoreError> {
            Ok(SecretaryStatusView {
                unresolved_gap_count: 0,
                open_gap_count: 0,
                earliest_gap_started_at_unix_secs: None,
                open_thread_count: 0,
                waiting_thread_count: 0,
                active_response_expectation_count: 0,
                scheduled_follow_up_count: 0,
                pending_evaluation_count: 0,
                pending_outbox_count: 0,
                failed_outbox_count: 0,
            })
        }
        async fn list_pending_owner_work(
            &self,
            _account: &SourceAccountRef,
            _limit: u16,
        ) -> Result<Vec<PendingOwnerWorkItem>, InboundEventStoreError> {
            Ok(Vec::new())
        }
        async fn thread_context(
            &self,
            _account: &SourceAccountRef,
            _thread_id: &EventThreadId,
        ) -> Result<Option<ThreadContextView>, InboundEventStoreError> {
            Ok(None)
        }
        async fn event_causal_context(
            &self,
            _account: &SourceAccountRef,
            _source_event_id: &SourceEventId,
        ) -> Result<Option<EventCausalContextView>, InboundEventStoreError> {
            Ok(None)
        }
        async fn participant_context(
            &self,
            _account: &SourceAccountRef,
            _actor_id: &str,
            _conversation: Option<&ConversationRef>,
            _thread_id: Option<&EventThreadId>,
        ) -> Result<Option<ParticipantContextView>, InboundEventStoreError> {
            Ok(None)
        }
        async fn participant_context_by_ref(
            &self,
            _participant: &AccountScopedParticipantRef,
            _conversation: Option<&ConversationRef>,
            _thread_id: Option<&EventThreadId>,
        ) -> Result<Option<ParticipantContextView>, InboundEventStoreError> {
            Ok(None)
        }
        async fn participants_by_display_name(
            &self,
            _account: &SourceAccountRef,
            _name: &str,
            _conversation: Option<&ConversationRef>,
            _thread_id: Option<&EventThreadId>,
            _limit: u16,
        ) -> Result<Vec<AccountScopedParticipantRef>, InboundEventStoreError> {
            Ok(Vec::new())
        }
        async fn list_projects(
            &self,
            _account: &SourceAccountRef,
            _limit: u16,
        ) -> Result<Vec<ProjectMemorySummary>, InboundEventStoreError> {
            Ok(Vec::new())
        }
        async fn query_project(
            &self,
            _account: &SourceAccountRef,
            _project_key: &str,
        ) -> Result<Option<ProjectContextView>, InboundEventStoreError> {
            Ok(None)
        }
        async fn list_commitments(
            &self,
            _query: &CommitmentQuery,
        ) -> Result<Vec<CommitmentSummary>, InboundEventStoreError> {
            Ok(Vec::new())
        }
    }

    fn event(trust: ContentTrustLevel) -> EventSearchResult {
        EventSearchResult {
            source_event_id: SourceEventId::new("e1").unwrap(),
            conversation: ConversationRef::new(ConversationKind::Group, "g1").unwrap(),
            actor: VerifiedActor::new(VerifiedActorKind::External, "a1").unwrap(),
            participant: None,
            message_role: MessageRole::ExternalObservation,
            occurred_at_unix_secs: 100,
            excerpt: "text".into(),
            content_trust_level: trust,
            thread_id: None,
        }
    }

    #[tokio::test]
    async fn remote_loopback_excludes_local_only_by_default() {
        let store = Arc::new(FakeStore {
            events: Mutex::new(vec![
                event(ContentTrustLevel::Normal),
                event(ContentTrustLevel::LocalOnly),
            ]),
        });
        let use_case = RetrieverUseCase::new(store, RetrieverPolicy::default());
        let query = EventQuery::for_account(account());
        let results = use_case.search_events(&query, false).await.unwrap();
        // 远程模型排除 local_only
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn local_loopback_includes_local_only_when_allowed() {
        let store = Arc::new(FakeStore {
            events: Mutex::new(vec![
                event(ContentTrustLevel::Normal),
                event(ContentTrustLevel::LocalOnly),
            ]),
        });
        let policy = RetrieverPolicy {
            allow_local_only_to_loopback_llm: true,
        };
        let use_case = RetrieverUseCase::new(store, policy);
        let query = EventQuery::for_account(account());
        let results = use_case.search_events(&query, true).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn local_loopback_excludes_local_only_when_not_allowed() {
        let store = Arc::new(FakeStore {
            events: Mutex::new(vec![event(ContentTrustLevel::LocalOnly)]),
        });
        let use_case = RetrieverUseCase::new(store, RetrieverPolicy::default());
        let query = EventQuery::for_account(account());
        let results = use_case.search_events(&query, true).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn never_long_term_always_excluded() {
        let store = Arc::new(FakeStore {
            events: Mutex::new(vec![event(ContentTrustLevel::NeverLongTerm)]),
        });
        let policy = RetrieverPolicy {
            allow_local_only_to_loopback_llm: true,
        };
        let use_case = RetrieverUseCase::new(store, policy);
        let query = EventQuery::for_account(account());
        let results = use_case.search_events(&query, true).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn search_threads_rejects_empty_query() {
        let store = Arc::new(FakeStore {
            events: Mutex::new(Vec::new()),
        });
        let use_case = RetrieverUseCase::new(store, RetrieverPolicy::default());
        let result = use_case.search_threads(&account(), "  ", 10).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_upcoming_rejects_zero_horizon() {
        let store = Arc::new(FakeStore {
            events: Mutex::new(Vec::new()),
        });
        let use_case = RetrieverUseCase::new(store, RetrieverPolicy::default());
        let result = use_case.list_upcoming(&account(), 0).await;
        assert!(result.is_err());
    }
}
