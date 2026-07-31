//! Owner Retriever 领域类型与端口。
//!
//! 本模块定义协议无关的检索查询、结果类型、指代解析上下文和 Store 端口。
//! 领域层不依赖 SeaORM、NapCat 或 LLM；复杂查询在基础设施仓储中实现。

use std::collections::HashSet;

use async_trait::async_trait;

use crate::{
    ConversationRef, EventThreadId, InboundEventStoreError, MessageRole, RecentEventRef,
    SourceAccountRef, SourceEventId, ThreadStatus, VerifiedActor,
};

// ===== 查询与结果类型 =====

/// 事件检索查询参数。所有字段都是协议无关的。
#[derive(Debug, Clone)]
pub struct EventQuery {
    /// 账号作用域。检索结果严格限定在此账号内，跨账号查询被拒绝。
    pub account: SourceAccountRef,
    /// 会话过滤（群 ID 或私聊对端 ID）。
    pub conversation: Option<ConversationRef>,
    /// 发送者 Actor ID 过滤。
    pub actor_id: Option<String>,
    /// 线程过滤。
    pub thread_id: Option<EventThreadId>,
    /// 起始时间（Unix 秒，含）。
    pub since_unix_secs: Option<i64>,
    /// 截止时间（Unix 秒，含）。
    pub until_unix_secs: Option<i64>,
    /// 关键词过滤（在 normalized_text 上做 LIKE）。
    pub query_text: Option<String>,
    /// 返回上限。1..=100。
    pub limit: u16,
}

impl EventQuery {
    /// 创建一个只限定账号的查询，默认 limit=20。
    pub fn for_account(account: SourceAccountRef) -> Self {
        Self {
            account,
            conversation: None,
            actor_id: None,
            thread_id: None,
            since_unix_secs: None,
            until_unix_secs: None,
            query_text: None,
            limit: 20,
        }
    }
}

/// 内容信任级别，决定正文是否可以进入检索结果或远程 LLM。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentTrustLevel {
    /// 普通内容，可进入长期记忆、检索和模型。
    Normal,
    /// 本地内容，默认不发送远程 LLM；仅在本地 loopback 且配置允许时可进入模型。
    LocalOnly,
    /// 信封模式，只提供元数据（发送者、时间、会话），不提供正文。
    EnvelopeOnly,
    /// 永不进入长期记忆和模型。
    NeverLongTerm,
}

impl ContentTrustLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::LocalOnly => "local_only",
            Self::EnvelopeOnly => "envelope_only",
            Self::NeverLongTerm => "never_long_term",
        }
    }
}

/// 事件检索结果项。正文按内容策略返回有界摘录，envelope_only 时为空。
#[derive(Debug, Clone)]
pub struct EventSearchResult {
    pub source_event_id: SourceEventId,
    pub conversation: ConversationRef,
    pub actor: VerifiedActor,
    /// 参与者身份（比 VerifiedActor 更丰富，包含昵称、别名和可信等级）。
    pub participant: Option<ParticipantIdentity>,
    pub message_role: MessageRole,
    pub occurred_at_unix_secs: i64,
    /// 有界正文摘录。envelope_only 为空字符串。
    pub excerpt: String,
    pub content_trust_level: ContentTrustLevel,
    pub thread_id: Option<EventThreadId>,
}

/// 单条事件详情。按内容策略返回完整或空正文。
#[derive(Debug, Clone)]
pub struct SourceEventDetail {
    pub source_event_id: SourceEventId,
    pub account: SourceAccountRef,
    pub conversation: ConversationRef,
    pub actor: VerifiedActor,
    pub participant: Option<ParticipantIdentity>,
    pub message_role: MessageRole,
    pub occurred_at_unix_secs: i64,
    /// 按 content_mode 策略返回正文。envelope_only 为空字符串。
    pub normalized_text: String,
    pub content_trust_level: ContentTrustLevel,
    pub reply_to_event_id: Option<SourceEventId>,
    pub thread_id: Option<EventThreadId>,
}

/// 线程搜索结果。
#[derive(Debug, Clone)]
pub struct ThreadSearchResult {
    pub thread_id: EventThreadId,
    pub status: ThreadStatus,
    pub event_count: u64,
    pub latest_event_at_unix_secs: i64,
    /// 最新事件的有界摘录。
    pub latest_excerpt: String,
}

/// 即将到期事项。
#[derive(Debug, Clone)]
pub struct UpcomingItem {
    pub item_id: String,
    pub kind: String,
    pub due_at_unix_secs: i64,
    pub excerpt: String,
    pub source_event_id: SourceEventId,
}

/// Owner 可见的秘书运行状态。所有计数均限定在单一被管理账号内。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretaryStatusView {
    pub unresolved_gap_count: u64,
    pub open_gap_count: u64,
    pub earliest_gap_started_at_unix_secs: Option<i64>,
    pub open_thread_count: u64,
    pub waiting_thread_count: u64,
    pub active_response_expectation_count: u64,
    pub scheduled_follow_up_count: u64,
    pub pending_evaluation_count: u64,
    pub pending_outbox_count: u64,
    pub failed_outbox_count: u64,
}

/// 一条需要 Owner 关注的有界事项；`summary` 只用于导航，事实仍由来源 ID 回读。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOwnerWorkItem {
    pub source_kind: String,
    pub source_id: String,
    pub due_at_unix_secs: Option<i64>,
    pub status: String,
    pub summary: String,
}

/// 线程参与者的账号作用域统计。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadActorSummary {
    pub actor_kind: String,
    pub actor_id: String,
    pub event_count: u64,
}

/// 谁提出了什么要求或意见，以及对应来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadClaimSummary {
    pub claim_id: String,
    pub claim_kind: String,
    pub claimant_actor_id: String,
    pub status: String,
    pub statement: String,
    pub source_event_ids: Vec<SourceEventId>,
}

/// 已形成或仍在修订的线程结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadDecisionSummary {
    pub decision_id: String,
    pub status: String,
    pub statement: String,
    pub source_event_ids: Vec<SourceEventId>,
}

/// 尚未达成一致或仍待回答的问题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadQuestionSummary {
    pub question_id: String,
    pub raised_by_actor_id: String,
    pub status: String,
    pub question: String,
    pub source_event_ids: Vec<SourceEventId>,
}

/// 单线程的有界因果上下文。正文摘要只导航，来源 ID 提供可审计回读。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadContextView {
    pub thread_id: EventThreadId,
    pub status: ThreadStatus,
    pub event_count: u64,
    pub actors: Vec<ThreadActorSummary>,
    pub claims: Vec<ThreadClaimSummary>,
    pub decisions: Vec<ThreadDecisionSummary>,
    pub open_questions: Vec<ThreadQuestionSummary>,
}

// ===== 参与者身份（EVT-001）=====

/// 账号作用域的参与者身份。比 `VerifiedActor` 更丰富，包含群名片、昵称、别名和可信等级。
/// 普通群成员不一定是已验证 Actor，但仍有可观察的身份信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantIdentity {
    /// 平台身份种类（与 VerifiedActorKind 对应但更丰富）。
    pub platform_kind: PlatformIdentityKind,
    /// 稳定主体 ID（QQ 号、OpenID 等）。
    pub stable_id: String,
    /// 群名片或备注名（可缺失）。
    pub display_name: Option<String>,
    /// 别名集合（可来自 Owner 手动设置或历史观察）。
    pub aliases: Vec<String>,
    /// 身份来源和可信等级。
    pub trust: IdentityTrust,
}

impl ParticipantIdentity {
    pub fn new(
        platform_kind: PlatformIdentityKind,
        stable_id: impl Into<String>,
        trust: IdentityTrust,
    ) -> Result<Self, RetrieverError> {
        let stable_id = stable_id.into();
        if stable_id.trim().is_empty() {
            return Err(RetrieverError::InvalidData(
                "ParticipantIdentity.stable_id must not be empty".into(),
            ));
        }
        Ok(Self {
            platform_kind,
            stable_id,
            display_name: None,
            aliases: Vec::new(),
            trust,
        })
    }

    /// 返回所有可用于指代解析的名称（display_name + aliases）。
    pub fn all_names(&self) -> Vec<&str> {
        let mut names = Vec::new();
        if let Some(display) = &self.display_name {
            names.push(display.as_str());
        }
        for alias in &self.aliases {
            names.push(alias.as_str());
        }
        names
    }
}

/// 平台身份种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformIdentityKind {
    /// Owner 本人（通过配置绑定或平台签名验证）。
    Owner,
    /// 官方 Bot。
    OfficialBot,
    /// 外部普通用户（群成员或私聊对端）。
    External,
}

impl PlatformIdentityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::OfficialBot => "official_bot",
            Self::External => "external",
        }
    }

    /// 从 VerifiedActorKind 转换。
    pub fn from_verified_actor_kind(kind: crate::VerifiedActorKind) -> Self {
        match kind {
            crate::VerifiedActorKind::Owner => Self::Owner,
            crate::VerifiedActorKind::OfficialBot => Self::OfficialBot,
            crate::VerifiedActorKind::External => Self::External,
        }
    }
}

/// 身份来源和可信等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityTrust {
    /// 通过配置绑定或平台签名验证，最高可信。
    Verified,
    /// 通过协议字段观察（如 NapCat sender 字段），中等可信。
    Observed,
    /// 由 Owner 手动设置或历史推断，低可信。
    Inferred,
}

impl IdentityTrust {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Observed => "observed",
            Self::Inferred => "inferred",
        }
    }
}

/// 参与者引用（轻量，用于结果中引用而不内联完整身份）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantRef {
    pub platform_kind: PlatformIdentityKind,
    pub stable_id: String,
}

impl ParticipantRef {
    pub fn new(
        platform_kind: PlatformIdentityKind,
        stable_id: impl Into<String>,
    ) -> Result<Self, RetrieverError> {
        let stable_id = stable_id.into();
        if stable_id.trim().is_empty() {
            return Err(RetrieverError::InvalidData(
                "ParticipantRef.stable_id must not be empty".into(),
            ));
        }
        Ok(Self {
            platform_kind,
            stable_id,
        })
    }
}

// ===== 指代解析 =====

/// Store 返回的原始候选（不做判定，只返回数据）。
#[derive(Debug, Clone)]
pub struct ReferenceCandidate {
    pub actor_id: Option<String>,
    pub participant: Option<ParticipantIdentity>,
    pub thread_id: Option<EventThreadId>,
    pub source_event_ids: Vec<SourceEventId>,
    /// 为什么是这个候选（依据说明）。
    pub evidence: String,
}

/// 用例层的指代解析上下文。
#[derive(Debug, Clone)]
pub struct ReferenceContext {
    pub account: SourceAccountRef,
    pub current_conversation: Option<ConversationRef>,
    pub current_thread_id: Option<EventThreadId>,
    pub recent_events: Vec<RecentEventRef>,
    pub now_unix_secs: i64,
    pub timezone: String,
}

/// 用例层判定后的指代解析结果。
#[derive(Debug, Clone)]
pub struct ReferenceResolution {
    pub resolved_actor_id: Option<String>,
    pub resolved_thread_id: Option<EventThreadId>,
    pub resolved_event_ids: Vec<SourceEventId>,
    pub ambiguous: bool,
    /// 判定依据。
    pub evidence: String,
}

// ===== Store 端口 =====

/// Retriever 存储端口。基础设施层实现，领域层定义。
#[async_trait]
pub trait RetrieverStoreT: Send + Sync {
    /// 按多条件检索事件。SQL 中强制 account_id 过滤，跨账号查询被拒绝。
    /// 正文按 content_mode/memory_mode 策略返回。
    async fn search_events(
        &self,
        query: &EventQuery,
    ) -> Result<Vec<EventSearchResult>, InboundEventStoreError>;

    /// 读取单条事件详情。account 限定，envelope_only 返回空正文。
    async fn read_source_event(
        &self,
        event_id: &SourceEventId,
        account: &SourceAccountRef,
    ) -> Result<Option<SourceEventDetail>, InboundEventStoreError>;

    /// 按关键词搜索线程。
    async fn search_threads(
        &self,
        account: &SourceAccountRef,
        query_text: &str,
        limit: u16,
    ) -> Result<Vec<ThreadSearchResult>, InboundEventStoreError>;

    /// 查找指代解析候选。Store 只返回候选集合，不判定唯一/歧义。
    async fn find_reference_candidates(
        &self,
        account: &SourceAccountRef,
        expression: &str,
        context: &ReferenceContext,
    ) -> Result<Vec<ReferenceCandidate>, InboundEventStoreError>;

    /// 查询即将到期的承诺和提醒。
    async fn list_upcoming(
        &self,
        account: &SourceAccountRef,
        horizon_secs: u64,
    ) -> Result<Vec<UpcomingItem>, InboundEventStoreError>;

    /// 查询账号级连续性、线程、跟进、求值和 Outbox 状态。
    async fn secretary_status(
        &self,
        account: &SourceAccountRef,
    ) -> Result<SecretaryStatusView, InboundEventStoreError>;

    /// 查询需要 Owner 处理的事项；结果必须有界且按到期时间排序。
    async fn list_pending_owner_work(
        &self,
        account: &SourceAccountRef,
        limit: u16,
    ) -> Result<Vec<PendingOwnerWorkItem>, InboundEventStoreError>;

    /// 查询单线程的参与者、要求、结论和未决问题；严格限定账号。
    async fn thread_context(
        &self,
        account: &SourceAccountRef,
        thread_id: &EventThreadId,
    ) -> Result<Option<ThreadContextView>, InboundEventStoreError>;
}

// ===== 错误类型 =====

#[derive(Debug, thiserror::Error)]
pub enum RetrieverError {
    #[error("invalid retriever data: {0}")]
    InvalidData(String),
}

// ===== 校验与过滤纯函数 =====

/// 校验 EventQuery 参数。
pub fn validate_event_query(query: &EventQuery) -> Result<(), RetrieverError> {
    if query.limit == 0 || query.limit > 100 {
        return Err(RetrieverError::InvalidData(
            "EventQuery.limit must be in 1..=100".into(),
        ));
    }
    if let Some(text) = &query.query_text
        && text.chars().count() > 1000
    {
        return Err(RetrieverError::InvalidData(
            "EventQuery.query_text must not exceed 1000 chars".into(),
        ));
    }
    if let (Some(since), Some(until)) = (query.since_unix_secs, query.until_unix_secs)
        && since > until
    {
        return Err(RetrieverError::InvalidData(
            "EventQuery.since_unix_secs must not be after until_unix_secs".into(),
        ));
    }
    Ok(())
}

/// 判定哪些信任级别的内容可以进入模型输入。
/// `is_local_loopback` 由已验证的 LLM 配置生成，不能由调用方随意传入。
/// 远程模型（`is_local_loopback=false`）无条件排除 `local_only` 和 `never_long_term`。
/// 本地模型（`is_local_loopback=true`）且 `allow_local_only` 时仍排除 `never_long_term`。
pub fn is_allowed_for_model(
    trust: ContentTrustLevel,
    is_local_loopback: bool,
    allow_local_only: bool,
) -> bool {
    match trust {
        ContentTrustLevel::Normal => true,
        ContentTrustLevel::LocalOnly => is_local_loopback && allow_local_only,
        ContentTrustLevel::EnvelopeOnly => false,
        ContentTrustLevel::NeverLongTerm => false,
    }
}

/// 过滤检索结果，只保留允许进入模型输入的条目。
pub fn filter_for_model(
    results: Vec<EventSearchResult>,
    is_local_loopback: bool,
    allow_local_only: bool,
) -> Vec<EventSearchResult> {
    results
        .into_iter()
        .filter(|r| {
            is_allowed_for_model(r.content_trust_level, is_local_loopback, allow_local_only)
        })
        .collect()
}

/// 判定指代解析结果：从候选集合中判定唯一、歧义或无结果。
pub fn resolve_reference_from_candidates(
    candidates: Vec<ReferenceCandidate>,
    _context: &ReferenceContext,
) -> ReferenceResolution {
    if candidates.is_empty() {
        return ReferenceResolution {
            resolved_actor_id: None,
            resolved_thread_id: None,
            resolved_event_ids: Vec::new(),
            ambiguous: false,
            evidence: "无匹配候选".into(),
        };
    }

    // 收集所有不重复的 actor_id 和 thread_id
    let mut actor_ids: Vec<String> = Vec::new();
    let mut thread_ids: Vec<EventThreadId> = Vec::new();
    let mut all_event_ids: HashSet<String> = HashSet::new();

    for candidate in &candidates {
        if let Some(actor_id) = &candidate.actor_id
            && !actor_ids.contains(actor_id)
        {
            actor_ids.push(actor_id.clone());
        }
        if let Some(thread_id) = &candidate.thread_id
            && !thread_ids.contains(thread_id)
        {
            thread_ids.push(thread_id.clone());
        }
        for event_id in &candidate.source_event_ids {
            all_event_ids.insert(event_id.as_str().to_owned());
        }
    }

    let resolved_event_ids: Vec<SourceEventId> = all_event_ids
        .into_iter()
        .filter_map(|id| SourceEventId::new(id).ok())
        .collect();

    // 唯一 actor 且唯一 thread -> 非歧义
    if actor_ids.len() == 1 && thread_ids.len() <= 1 {
        return ReferenceResolution {
            resolved_actor_id: actor_ids.first().cloned(),
            resolved_thread_id: thread_ids.first().cloned(),
            resolved_event_ids,
            ambiguous: false,
            evidence: format!("匹配到唯一候选（{}个证据）", candidates.len()),
        };
    }

    // 多个候选 -> 歧义
    ReferenceResolution {
        resolved_actor_id: actor_ids.first().cloned(),
        resolved_thread_id: thread_ids.first().cloned(),
        resolved_event_ids,
        ambiguous: true,
        evidence: format!(
            "匹配到 {} 个候选（{} 个 actor，{} 个线程），需 Owner 澄清",
            candidates.len(),
            actor_ids.len(),
            thread_ids.len()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MessageSource, SourceAccountRef};

    fn account() -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, "account-1").unwrap()
    }

    #[test]
    fn validate_rejects_zero_limit() {
        let query = EventQuery {
            account: account(),
            conversation: None,
            actor_id: None,
            thread_id: None,
            since_unix_secs: None,
            until_unix_secs: None,
            query_text: None,
            limit: 0,
        };
        assert!(validate_event_query(&query).is_err());
    }

    #[test]
    fn validate_rejects_limit_over_100() {
        let query = EventQuery {
            account: account(),
            conversation: None,
            actor_id: None,
            thread_id: None,
            since_unix_secs: None,
            until_unix_secs: None,
            query_text: None,
            limit: 101,
        };
        assert!(validate_event_query(&query).is_err());
    }

    #[test]
    fn validate_rejects_since_after_until() {
        let query = EventQuery {
            account: account(),
            conversation: None,
            actor_id: None,
            thread_id: None,
            since_unix_secs: Some(200),
            until_unix_secs: Some(100),
            query_text: None,
            limit: 10,
        };
        assert!(validate_event_query(&query).is_err());
    }

    #[test]
    fn local_only_excluded_from_remote_model() {
        assert!(!is_allowed_for_model(
            ContentTrustLevel::LocalOnly,
            false,
            true
        ));
    }

    #[test]
    fn local_only_allowed_for_loopback_when_configured() {
        assert!(is_allowed_for_model(
            ContentTrustLevel::LocalOnly,
            true,
            true
        ));
    }

    #[test]
    fn local_only_blocked_for_loopback_when_not_configured() {
        assert!(!is_allowed_for_model(
            ContentTrustLevel::LocalOnly,
            true,
            false
        ));
    }

    #[test]
    fn never_long_term_always_excluded() {
        assert!(!is_allowed_for_model(
            ContentTrustLevel::NeverLongTerm,
            true,
            true
        ));
    }

    #[test]
    fn envelope_only_always_excluded_from_model() {
        assert!(!is_allowed_for_model(
            ContentTrustLevel::EnvelopeOnly,
            true,
            true
        ));
    }

    #[test]
    fn normal_always_allowed() {
        assert!(is_allowed_for_model(
            ContentTrustLevel::Normal,
            false,
            false
        ));
    }

    #[test]
    fn participant_identity_all_names_includes_display_and_aliases() {
        let mut p = ParticipantIdentity::new(
            PlatformIdentityKind::External,
            "12345",
            IdentityTrust::Observed,
        )
        .unwrap();
        p.display_name = Some("张三".into());
        p.aliases = vec!["老张".into(), "Zhang".into()];
        assert_eq!(p.all_names(), vec!["张三", "老张", "Zhang"]);
    }

    #[test]
    fn participant_identity_rejects_empty_stable_id() {
        let result =
            ParticipantIdentity::new(PlatformIdentityKind::External, "", IdentityTrust::Observed);
        assert!(result.is_err());
    }

    #[test]
    fn platform_kind_from_verified_actor_kind() {
        assert_eq!(
            PlatformIdentityKind::from_verified_actor_kind(crate::VerifiedActorKind::Owner),
            PlatformIdentityKind::Owner
        );
        assert_eq!(
            PlatformIdentityKind::from_verified_actor_kind(crate::VerifiedActorKind::External),
            PlatformIdentityKind::External
        );
    }

    #[test]
    fn resolve_empty_candidates_returns_no_result() {
        let context = ReferenceContext {
            account: account(),
            current_conversation: None,
            current_thread_id: None,
            recent_events: Vec::new(),
            now_unix_secs: 1000,
            timezone: "Asia/Shanghai".into(),
        };
        let result = resolve_reference_from_candidates(Vec::new(), &context);
        assert!(!result.ambiguous);
        assert!(result.resolved_actor_id.is_none());
    }

    #[test]
    fn resolve_single_candidate_not_ambiguous() {
        let context = ReferenceContext {
            account: account(),
            current_conversation: None,
            current_thread_id: None,
            recent_events: Vec::new(),
            now_unix_secs: 1000,
            timezone: "Asia/Shanghai".into(),
        };
        let candidate = ReferenceCandidate {
            actor_id: Some("12345".into()),
            participant: None,
            thread_id: None,
            source_event_ids: vec![SourceEventId::new("event-1").unwrap()],
            evidence: "匹配".into(),
        };
        let result = resolve_reference_from_candidates(vec![candidate], &context);
        assert!(!result.ambiguous);
        assert_eq!(result.resolved_actor_id.as_deref(), Some("12345"));
    }

    #[test]
    fn resolve_multiple_actors_ambiguous() {
        let context = ReferenceContext {
            account: account(),
            current_conversation: None,
            current_thread_id: None,
            recent_events: Vec::new(),
            now_unix_secs: 1000,
            timezone: "Asia/Shanghai".into(),
        };
        let candidates = vec![
            ReferenceCandidate {
                actor_id: Some("111".into()),
                participant: None,
                thread_id: None,
                source_event_ids: vec![SourceEventId::new("event-1").unwrap()],
                evidence: "候选1".into(),
            },
            ReferenceCandidate {
                actor_id: Some("222".into()),
                participant: None,
                thread_id: None,
                source_event_ids: vec![SourceEventId::new("event-2").unwrap()],
                evidence: "候选2".into(),
            },
        ];
        let result = resolve_reference_from_candidates(candidates, &context);
        assert!(result.ambiguous);
    }

    #[test]
    fn filter_for_model_removes_local_only_from_remote() {
        let results = vec![
            EventSearchResult {
                source_event_id: SourceEventId::new("e1").unwrap(),
                conversation: crate::ConversationRef::new(crate::ConversationKind::Group, "g1")
                    .unwrap(),
                actor: crate::VerifiedActor::new(crate::VerifiedActorKind::External, "a1").unwrap(),
                participant: None,
                message_role: crate::MessageRole::ExternalObservation,
                occurred_at_unix_secs: 100,
                excerpt: "normal text".into(),
                content_trust_level: ContentTrustLevel::Normal,
                thread_id: None,
            },
            EventSearchResult {
                source_event_id: SourceEventId::new("e2").unwrap(),
                conversation: crate::ConversationRef::new(crate::ConversationKind::Group, "g1")
                    .unwrap(),
                actor: crate::VerifiedActor::new(crate::VerifiedActorKind::External, "a2").unwrap(),
                participant: None,
                message_role: crate::MessageRole::ExternalObservation,
                occurred_at_unix_secs: 200,
                excerpt: "local only text".into(),
                content_trust_level: ContentTrustLevel::LocalOnly,
                thread_id: None,
            },
        ];
        let filtered = filter_for_model(results, false, true);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].source_event_id.as_str(), "e1");
    }
}
