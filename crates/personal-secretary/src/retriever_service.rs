//! Retriever 用例编排。封装 RetrieverStoreT，提供内容策略过滤和指代解析。
//!
//! 不直接调用 LLM；Planner 调用本服务获取已过滤的检索结果。

use std::sync::Arc;

use thiserror::Error;

use crate::{
    Clock, ContentTrustLevel, EventQuery, EventSearchResult, InboundEventStoreError,
    ReferenceContext, ReferenceResolution, RetrieverError, RetrieverStoreT, SourceAccountRef,
    SourceEventDetail, SystemClock, ThreadSearchResult, UpcomingItem, filter_for_model,
    resolve_reference_from_candidates, validate_event_query,
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

    /// 解析指代。Store 返回候选，用例层判定唯一/歧义/无结果。
    pub async fn resolve_reference(
        &self,
        account: &SourceAccountRef,
        expression: &str,
        context: &ReferenceContext,
    ) -> Result<ReferenceResolution, RetrieverUseCaseError> {
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
