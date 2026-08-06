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
    ProjectMemorySummary, ReferenceContext, ReferenceResolution, RetrievalVisibility,
    RetrieverError, RetrieverStoreT, SecretaryStatusView, SourceAccountRef, SourceEventDetail,
    SourceEventId, SystemClock, ThreadContextView, ThreadSearchResult, UpcomingItem,
    check_causal_role_strictness, check_participant_permission_boundary,
    resolve_reference_from_candidates, validate_causal_context, validate_event_query,
    validate_participant_context,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextReferenceIntent {
    CurrentMessage,
    PreviousMessage,
    ReplyParent,
    ReplyParentActor,
    CurrentThread,
    ThreadStarter,
}

fn classify_context_reference(expression: &str) -> Option<ContextReferenceIntent> {
    use ContextReferenceIntent::*;

    let normalized: String = expression
        .trim()
        .chars()
        .filter(|ch| !ch.is_whitespace() && !matches!(ch, '，' | '。' | '？' | '?' | '！' | '!'))
        .collect();
    match normalized.as_str() {
        "这条消息" | "当前消息" | "本条消息" => Some(CurrentMessage),
        "上一条" | "上一条消息" | "前一条消息" | "刚才那条消息" => {
            Some(PreviousMessage)
        }
        "回复的原消息" | "我回复的原消息" | "被回复的消息" | "回复父消息" => {
            Some(ReplyParent)
        }
        "被回复的人" | "原消息发送者" | "回复对象" => Some(ReplyParentActor),
        "这个线程" | "当前线程" | "这个话题" | "当前话题" => Some(CurrentThread),
        "线程发起人" | "这个线程的发起人" | "话题发起人" | "这个话题的发起人" => {
            Some(ThreadStarter)
        }
        _ => None,
    }
}

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
        let visibility = RetrievalVisibility::for_model(
            is_local_loopback,
            self.policy.allow_local_only_to_loopback_llm,
        );
        Ok(self
            .store
            .list_recent_event_views(account, limit, visibility)
            .await?)
    }

    /// 检索事件并按内容策略过滤。
    /// `is_local_loopback` 由已验证的 LLM 配置生成，不能由调用方传入（约束 6）。
    pub async fn search_events(
        &self,
        query: &EventQuery,
        is_local_loopback: bool,
    ) -> Result<Vec<EventSearchResult>, RetrieverUseCaseError> {
        validate_event_query(query).map_err(RetrieverUseCaseError::Domain)?;
        let visibility = RetrievalVisibility::for_model(
            is_local_loopback,
            self.policy.allow_local_only_to_loopback_llm,
        );
        Ok(self.store.search_events(query, visibility).await?)
    }

    pub async fn read_source_event(
        &self,
        event_id: &crate::SourceEventId,
        account: &SourceAccountRef,
    ) -> Result<Option<SourceEventDetail>, RetrieverUseCaseError> {
        Ok(self
            .store
            .read_source_event(event_id, account, RetrievalVisibility::InternalMetadata)
            .await?)
    }

    /// 读取允许进入当前模型边界的单条事件。`local_only` 例外只对已验证的
    /// 本地 loopback 模型开放，授权在数据库查询正文前完成。
    pub async fn read_source_event_for_model(
        &self,
        event_id: &crate::SourceEventId,
        account: &SourceAccountRef,
        is_local_loopback: bool,
    ) -> Result<Option<SourceEventDetail>, RetrieverUseCaseError> {
        let visibility = RetrievalVisibility::for_model(
            is_local_loopback,
            self.policy.allow_local_only_to_loopback_llm,
        );
        Ok(self
            .store
            .read_source_event(event_id, account, visibility)
            .await?)
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
            .search_threads(account, query_text, limit, RetrievalVisibility::NormalOnly)
            .await?)
    }

    pub async fn search_threads_page(
        &self,
        account: &SourceAccountRef,
        query_text: &str,
        cursor: Option<&crate::ThreadSearchCursor>,
        limit: u16,
    ) -> Result<crate::ThreadSearchPage, RetrieverUseCaseError> {
        self.validate_thread_search_page(query_text, cursor, limit)?;
        Ok(self
            .store
            .search_threads_page(
                account,
                query_text.trim(),
                cursor,
                limit,
                RetrievalVisibility::NormalOnly,
            )
            .await?)
    }

    /// 返回允许进入当前模型边界的线程结果。内容策略直接传入 Store，
    /// 使候选集合、计数和排序都不会先观察再过滤受限内容。
    pub async fn search_threads_for_model(
        &self,
        account: &SourceAccountRef,
        query_text: &str,
        limit: u16,
        is_local_loopback: bool,
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
        let visibility = RetrievalVisibility::for_model(
            is_local_loopback,
            self.policy.allow_local_only_to_loopback_llm,
        );
        let results = self
            .store
            .search_threads(account, query_text, limit, visibility)
            .await?;
        Ok(results)
    }

    pub async fn search_threads_page_for_model(
        &self,
        account: &SourceAccountRef,
        query_text: &str,
        cursor: Option<&crate::ThreadSearchCursor>,
        limit: u16,
        is_local_loopback: bool,
    ) -> Result<crate::ThreadSearchPage, RetrieverUseCaseError> {
        self.validate_thread_search_page(query_text, cursor, limit)?;
        let visibility = RetrievalVisibility::for_model(
            is_local_loopback,
            self.policy.allow_local_only_to_loopback_llm,
        );
        Ok(self
            .store
            .search_threads_page(account, query_text.trim(), cursor, limit, visibility)
            .await?)
    }

    fn validate_thread_search_page(
        &self,
        query_text: &str,
        cursor: Option<&crate::ThreadSearchCursor>,
        limit: u16,
    ) -> Result<(), RetrieverUseCaseError> {
        let normalized_query = query_text.trim();
        if normalized_query.is_empty() {
            return Err(RetrieverUseCaseError::InvalidInput(
                "query_text must not be empty".into(),
            ));
        }
        if !(1..=100).contains(&limit) {
            return Err(RetrieverUseCaseError::InvalidInput(
                "limit must be in 1..=100".into(),
            ));
        }
        if cursor.is_some_and(|cursor| cursor.query_text() != normalized_query) {
            return Err(RetrieverUseCaseError::InvalidInput(
                "thread search cursor does not belong to this query".into(),
            ));
        }
        Ok(())
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

    pub async fn list_pending_owner_work_page(
        &self,
        account: &SourceAccountRef,
        cursor: Option<&crate::PendingOwnerWorkCursor>,
        limit: u16,
    ) -> Result<crate::PendingOwnerWorkPage, RetrieverUseCaseError> {
        if !(1..=20).contains(&limit) {
            return Err(RetrieverUseCaseError::InvalidInput(
                "pending owner work limit must be in 1..=20".into(),
            ));
        }
        Ok(self
            .store
            .list_pending_owner_work_page(account, cursor, limit)
            .await?)
    }

    pub async fn thread_context(
        &self,
        account: &SourceAccountRef,
        thread_id: &EventThreadId,
    ) -> Result<Option<ThreadContextView>, RetrieverUseCaseError> {
        Ok(self.store.thread_context(account, thread_id).await?)
    }

    pub async fn thread_decision_revisions(
        &self,
        account: &SourceAccountRef,
        thread_id: &EventThreadId,
        cursor: Option<&crate::ThreadDecisionRevisionCursor>,
        limit: u16,
    ) -> Result<crate::ThreadDecisionRevisionPage, RetrieverUseCaseError> {
        if !(1..=50).contains(&limit) {
            return Err(RetrieverUseCaseError::InvalidInput(
                "decision revision page limit must be in 1..=50".into(),
            ));
        }
        if let Some(cursor) = cursor
            && cursor.thread_id() != thread_id
        {
            return Err(RetrieverUseCaseError::InvalidInput(
                "decision revision cursor belongs to another thread".into(),
            ));
        }
        Ok(self
            .store
            .thread_decision_revisions(account, thread_id, cursor, limit)
            .await?)
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
        if let Some(resolution) = self
            .resolve_bounded_context_reference(expression, context)
            .await?
        {
            return Ok(resolution);
        }
        let candidates = self
            .store
            .find_reference_candidates(account, expression, context)
            .await?;
        Ok(resolve_reference_from_candidates(candidates, context))
    }

    async fn resolve_bounded_context_reference(
        &self,
        expression: &str,
        context: &ReferenceContext,
    ) -> Result<Option<ReferenceResolution>, RetrieverUseCaseError> {
        use ContextReferenceIntent::*;

        let Some(intent) = classify_context_reference(expression) else {
            return Ok(None);
        };
        let unresolved = |evidence: &str| ReferenceResolution {
            resolved_actor_id: None,
            resolved_thread_id: None,
            resolved_event_ids: Vec::new(),
            ambiguous: true,
            evidence: evidence.into(),
        };

        if intent == PreviousMessage {
            let Some(current_conversation) = context.current_conversation.as_ref() else {
                return Ok(Some(unresolved(
                    "未提供当前会话，不能从账号级最近窗口猜测上一条消息",
                )));
            };
            for event in context.recent_events.iter().rev() {
                if Some(&event.source_event_id) == context.current_event_id.as_ref() {
                    continue;
                }
                let detail = self
                    .store
                    .read_source_event(
                        &event.source_event_id,
                        &context.account,
                        RetrievalVisibility::NormalOnly,
                    )
                    .await?;
                if let Some(detail) = detail
                    && &detail.conversation == current_conversation
                {
                    return Ok(Some(ReferenceResolution {
                        resolved_actor_id: None,
                        resolved_thread_id: detail.thread_id,
                        resolved_event_ids: vec![event.source_event_id.clone()],
                        ambiguous: false,
                        evidence: "由当前运行的有界最近窗口解析上一条消息".into(),
                    }));
                }
            }
            return Ok(Some(unresolved(
                "当前会话内没有可证明的上一条消息，需 Owner 澄清",
            )));
        }

        let Some(current_event_id) = context.current_event_id.as_ref() else {
            return Ok(Some(unresolved(
                "当前运行缺少权威命令事件，不能解析上下文指代",
            )));
        };
        if intent == CurrentMessage {
            return Ok(Some(ReferenceResolution {
                resolved_actor_id: None,
                resolved_thread_id: context.current_thread_id.clone(),
                resolved_event_ids: vec![current_event_id.clone()],
                ambiguous: false,
                evidence: "由当前运行的权威命令事件解析当前消息".into(),
            }));
        }

        let causal = self
            .event_causal_context(&context.account, current_event_id)
            .await?;
        let Some(causal) = causal else {
            return Ok(Some(unresolved(
                "当前事件没有可验证的因果上下文，需 Owner 澄清",
            )));
        };
        let resolution = match intent {
            ReplyParent | ReplyParentActor => match causal.reply_parent {
                Some(parent) => ReferenceResolution {
                    resolved_actor_id: if intent == ReplyParentActor {
                        parent
                            .sender
                            .as_ref()
                            .map(|sender| sender.stable_id.clone())
                    } else {
                        None
                    },
                    resolved_thread_id: causal
                        .thread
                        .as_ref()
                        .map(|thread| thread.thread_id.clone()),
                    resolved_event_ids: vec![parent.source_event_id],
                    ambiguous: intent == ReplyParentActor && parent.sender.is_none(),
                    evidence: if intent == ReplyParentActor {
                        "由已确认 replies_to 关系解析被回复者".into()
                    } else {
                        "由已确认 replies_to 关系解析回复父消息".into()
                    },
                },
                None => unresolved("当前消息没有已确认的回复父消息，需 Owner 澄清"),
            },
            CurrentThread | ThreadStarter => match causal.thread {
                Some(thread) => ReferenceResolution {
                    resolved_actor_id: if intent == ThreadStarter {
                        thread
                            .root_sender
                            .as_ref()
                            .map(|sender| sender.stable_id.clone())
                    } else {
                        None
                    },
                    resolved_thread_id: Some(thread.thread_id),
                    resolved_event_ids: vec![thread.root_event_id],
                    ambiguous: intent == ThreadStarter && thread.root_sender.is_none(),
                    evidence: if intent == ThreadStarter {
                        "由有效线程根事件解析线程发起人".into()
                    } else {
                        "由有效线程投影解析当前线程".into()
                    },
                },
                None => unresolved("当前消息没有有效线程归属，需 Owner 澄清"),
            },
            CurrentMessage | PreviousMessage => unreachable!("handled above"),
        };
        Ok(Some(resolution))
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
        CausalEventRef, CausalThreadRef, ConversationKind, ConversationRef, EventQuery,
        EventSearchResult, IdentityTrust, MessageRole, MessageSource, ParticipantIdentity,
        PlatformIdentityKind, RecentEventRef, SourceAccountRef, SourceEventId, ThreadStatus,
        VerifiedActor, VerifiedActorKind,
    };
    use async_trait::async_trait;
    use std::sync::Mutex;

    fn account() -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, "account-1").unwrap()
    }

    struct FakeStore {
        events: Mutex<Vec<EventSearchResult>>,
        details: Mutex<Vec<SourceEventDetail>>,
        threads: Mutex<Vec<ThreadSearchResult>>,
        causal: Mutex<Option<EventCausalContextView>>,
    }

    fn fake_store(events: Vec<EventSearchResult>) -> Arc<FakeStore> {
        Arc::new(FakeStore {
            events: Mutex::new(events),
            details: Mutex::new(Vec::new()),
            threads: Mutex::new(Vec::new()),
            causal: Mutex::new(None),
        })
    }

    #[async_trait]
    impl RetrieverStoreT for FakeStore {
        async fn search_events(
            &self,
            _query: &EventQuery,
            visibility: RetrievalVisibility,
        ) -> Result<Vec<EventSearchResult>, InboundEventStoreError> {
            Ok(self
                .events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| {
                    matches!(event.content_trust_level, ContentTrustLevel::Normal)
                        || (visibility.includes_local_only()
                            && matches!(event.content_trust_level, ContentTrustLevel::LocalOnly))
                })
                .cloned()
                .collect())
        }
        async fn list_recent_event_views(
            &self,
            _account: &SourceAccountRef,
            _limit: u16,
            _visibility: RetrievalVisibility,
        ) -> Result<Vec<AgentEventView>, InboundEventStoreError> {
            Ok(Vec::new())
        }
        async fn read_source_event(
            &self,
            event_id: &SourceEventId,
            _account: &SourceAccountRef,
            _visibility: RetrievalVisibility,
        ) -> Result<Option<SourceEventDetail>, InboundEventStoreError> {
            Ok(self
                .details
                .lock()
                .unwrap()
                .iter()
                .find(|detail| &detail.source_event_id == event_id)
                .cloned())
        }
        async fn search_threads(
            &self,
            _account: &SourceAccountRef,
            _query_text: &str,
            _limit: u16,
            _visibility: RetrievalVisibility,
        ) -> Result<Vec<ThreadSearchResult>, InboundEventStoreError> {
            Ok(self.threads.lock().unwrap().clone())
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
        async fn thread_decision_revisions(
            &self,
            _account: &SourceAccountRef,
            _thread_id: &EventThreadId,
            _cursor: Option<&crate::ThreadDecisionRevisionCursor>,
            _limit: u16,
        ) -> Result<crate::ThreadDecisionRevisionPage, InboundEventStoreError> {
            Ok(crate::ThreadDecisionRevisionPage {
                decisions: Vec::new(),
                next_cursor: None,
            })
        }
        async fn event_causal_context(
            &self,
            _account: &SourceAccountRef,
            _source_event_id: &SourceEventId,
        ) -> Result<Option<EventCausalContextView>, InboundEventStoreError> {
            Ok(self.causal.lock().unwrap().clone())
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
        let store = fake_store(vec![
            event(ContentTrustLevel::Normal),
            event(ContentTrustLevel::LocalOnly),
        ]);
        let use_case = RetrieverUseCase::new(store, RetrieverPolicy::default());
        let query = EventQuery::for_account(account());
        let results = use_case.search_events(&query, false).await.unwrap();
        // 远程模型排除 local_only
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn local_loopback_includes_local_only_when_allowed() {
        let store = fake_store(vec![
            event(ContentTrustLevel::Normal),
            event(ContentTrustLevel::LocalOnly),
        ]);
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
        let store = fake_store(vec![event(ContentTrustLevel::LocalOnly)]);
        let use_case = RetrieverUseCase::new(store, RetrieverPolicy::default());
        let query = EventQuery::for_account(account());
        let results = use_case.search_events(&query, true).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn never_long_term_always_excluded() {
        let store = fake_store(vec![event(ContentTrustLevel::NeverLongTerm)]);
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
        let store = fake_store(Vec::new());
        let use_case = RetrieverUseCase::new(store, RetrieverPolicy::default());
        let result = use_case.search_threads(&account(), "  ", 10).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn thread_search_page_rejects_cursor_from_another_query() {
        let use_case = RetrieverUseCase::new(fake_store(Vec::new()), RetrieverPolicy::default());
        let cursor = crate::ThreadSearchCursor::new(
            "alpha",
            crate::ThreadSearchMatchRank::Exact,
            100,
            EventThreadId::new("thread-a").unwrap(),
        )
        .unwrap();
        assert!(
            use_case
                .search_threads_page(&account(), "beta", Some(&cursor), 10)
                .await
                .is_err()
        );
    }

    #[test]
    fn paging_cursor_deserialization_revalidates_private_fields() {
        let invalid_thread = serde_json::json!({
            "query_text": " alpha ",
            "match_rank": "exact",
            "latest_event_at_unix_secs": 100,
            "thread_id": "thread-a"
        });
        assert!(serde_json::from_value::<crate::ThreadSearchCursor>(invalid_thread).is_err());

        let invalid_pending = serde_json::json!({
            "due_at_unix_secs": null,
            "source_kind": "agenda",
            "source_id": ""
        });
        assert!(serde_json::from_value::<crate::PendingOwnerWorkCursor>(invalid_pending).is_err());
    }

    #[tokio::test]
    async fn list_upcoming_rejects_zero_horizon() {
        let store = fake_store(Vec::new());
        let use_case = RetrieverUseCase::new(store, RetrieverPolicy::default());
        let result = use_case.list_upcoming(&account(), 0).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn decision_revision_paging_rejects_invalid_limits_and_cross_thread_cursor() {
        let use_case = RetrieverUseCase::new(fake_store(Vec::new()), RetrieverPolicy::default());
        let thread = EventThreadId::new("thread-a").unwrap();
        assert!(
            use_case
                .thread_decision_revisions(&account(), &thread, None, 0)
                .await
                .is_err()
        );
        assert!(
            use_case
                .thread_decision_revisions(&account(), &thread, None, 51)
                .await
                .is_err()
        );

        let cursor = crate::ThreadDecisionRevisionCursor::new(
            EventThreadId::new("thread-b").unwrap(),
            1,
            crate::ThreadDecisionId::new("decision-b").unwrap(),
        )
        .unwrap();
        assert!(
            use_case
                .thread_decision_revisions(&account(), &thread, Some(&cursor), 10)
                .await
                .is_err()
        );
    }

    #[test]
    fn decision_revision_cursor_rejects_negative_timestamp() {
        assert!(
            crate::ThreadDecisionRevisionCursor::new(
                EventThreadId::new("thread-a").unwrap(),
                -1,
                crate::ThreadDecisionId::new("decision-a").unwrap(),
            )
            .is_err()
        );
    }

    #[test]
    fn decision_revision_cursor_deserialization_revalidates_all_fields() {
        let valid = serde_json::json!({
            "thread_id": "thread-a",
            "created_at_unix_micros": 1,
            "decision_id": "decision-a"
        });
        let cursor: crate::ThreadDecisionRevisionCursor =
            serde_json::from_value(valid).expect("valid cursor");
        assert_eq!(cursor.thread_id().as_str(), "thread-a");

        for invalid in [
            serde_json::json!({
                "thread_id": "",
                "created_at_unix_micros": 1,
                "decision_id": "decision-a"
            }),
            serde_json::json!({
                "thread_id": "thread-a",
                "created_at_unix_micros": -1,
                "decision_id": "decision-a"
            }),
            serde_json::json!({
                "thread_id": "thread-a",
                "created_at_unix_micros": 1,
                "decision_id": ""
            }),
        ] {
            assert!(
                serde_json::from_value::<crate::ThreadDecisionRevisionCursor>(invalid).is_err()
            );
        }
    }

    fn reference_context(current: &str, recent: &[&str]) -> ReferenceContext {
        ReferenceContext {
            account: account(),
            current_event_id: Some(SourceEventId::new(current).unwrap()),
            current_conversation: Some(
                ConversationRef::new(ConversationKind::Group, "quality-group").unwrap(),
            ),
            current_thread_id: Some(EventThreadId::new("quality-thread").unwrap()),
            recent_events: recent
                .iter()
                .map(|id| RecentEventRef {
                    source_event_id: SourceEventId::new(*id).unwrap(),
                    summary: String::new(),
                })
                .collect(),
            now_unix_secs: 1_700_000_000,
            timezone: "Asia/Shanghai".into(),
        }
    }

    #[tokio::test]
    async fn realistic_previous_message_reference_uses_bounded_run_window() {
        let store = fake_store(Vec::new());
        store.details.lock().unwrap().push(SourceEventDetail {
            source_event_id: SourceEventId::new("older-event").unwrap(),
            account: account(),
            conversation: ConversationRef::new(ConversationKind::Group, "quality-group").unwrap(),
            actor: VerifiedActor::new(VerifiedActorKind::External, "actor-older").unwrap(),
            participant: None,
            message_role: MessageRole::ExternalObservation,
            occurred_at_unix_secs: 1_699_999_999,
            normalized_text: "部署窗口".into(),
            content_trust_level: ContentTrustLevel::Normal,
            reply_to_event_id: None,
            thread_id: Some(EventThreadId::new("quality-thread").unwrap()),
        });
        store.details.lock().unwrap().push(SourceEventDetail {
            source_event_id: SourceEventId::new("other-group-event").unwrap(),
            account: account(),
            conversation: ConversationRef::new(ConversationKind::Group, "other-group").unwrap(),
            actor: VerifiedActor::new(VerifiedActorKind::External, "actor-other").unwrap(),
            participant: None,
            message_role: MessageRole::ExternalObservation,
            occurred_at_unix_secs: 1_699_999_999,
            normalized_text: "其他群消息".into(),
            content_trust_level: ContentTrustLevel::Normal,
            reply_to_event_id: None,
            thread_id: Some(EventThreadId::new("other-thread").unwrap()),
        });
        let use_case = RetrieverUseCase::new(store, RetrieverPolicy::default());
        let context = reference_context(
            "command-event",
            &["older-event", "other-group-event", "command-event"],
        );

        for sample in ["上一条消息", "刚才那条消息？"] {
            let resolution = use_case
                .resolve_reference(&account(), sample, &context)
                .await
                .unwrap();
            assert!(!resolution.ambiguous, "sample={sample}");
            assert_eq!(resolution.resolved_event_ids[0].as_str(), "older-event");
        }
    }

    #[tokio::test]
    async fn realistic_reply_and_thread_references_use_confirmed_causal_roles() {
        let store = fake_store(Vec::new());
        let parent_sender = ParticipantIdentity::new(
            PlatformIdentityKind::External,
            "parent-actor",
            IdentityTrust::Observed,
        )
        .unwrap();
        let root_sender = ParticipantIdentity::new(
            PlatformIdentityKind::External,
            "root-actor",
            IdentityTrust::Observed,
        )
        .unwrap();
        *store.causal.lock().unwrap() = Some(EventCausalContextView {
            source_event_id: SourceEventId::new("command-event").unwrap(),
            account: account(),
            sender: None,
            reply_parent: Some(CausalEventRef {
                source_event_id: SourceEventId::new("parent-event").unwrap(),
                sender: Some(parent_sender),
            }),
            thread: Some(CausalThreadRef {
                thread_id: EventThreadId::new("quality-thread").unwrap(),
                status: ThreadStatus::Open,
                root_event_id: SourceEventId::new("root-event").unwrap(),
                root_sender: Some(root_sender),
            }),
            mentioned: Vec::new(),
            requesters: Vec::new(),
            assignees: Vec::new(),
            promisors: Vec::new(),
            beneficiaries: Vec::new(),
            participants: Vec::new(),
            relations: Vec::new(),
            ambiguous: false,
            source_refs: Vec::new(),
        });
        let use_case = RetrieverUseCase::new(store, RetrieverPolicy::default());
        let context = reference_context("command-event", &["command-event"]);

        let replied_actor = use_case
            .resolve_reference(&account(), "被回复的人", &context)
            .await
            .unwrap();
        assert!(!replied_actor.ambiguous);
        assert_eq!(
            replied_actor.resolved_actor_id.as_deref(),
            Some("parent-actor")
        );
        assert_eq!(replied_actor.resolved_event_ids[0].as_str(), "parent-event");

        let starter = use_case
            .resolve_reference(&account(), "这个话题的发起人", &context)
            .await
            .unwrap();
        assert!(!starter.ambiguous);
        assert_eq!(starter.resolved_actor_id.as_deref(), Some("root-actor"));
        assert_eq!(starter.resolved_event_ids[0].as_str(), "root-event");
    }

    #[tokio::test]
    async fn contextual_reference_without_authoritative_evidence_stays_ambiguous() {
        let use_case = RetrieverUseCase::new(fake_store(Vec::new()), RetrieverPolicy::default());
        let context = reference_context("command-event", &["command-event"]);

        for sample in ["上一条消息", "回复的原消息", "线程发起人"] {
            let resolution = use_case
                .resolve_reference(&account(), sample, &context)
                .await
                .unwrap();
            assert!(resolution.ambiguous, "sample={sample}");
            assert!(resolution.resolved_event_ids.is_empty(), "sample={sample}");
        }
    }
}
