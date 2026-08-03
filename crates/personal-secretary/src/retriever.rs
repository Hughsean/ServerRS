//! Owner Retriever 领域类型与端口。
//!
//! 本模块定义协议无关的检索查询、结果类型、指代解析上下文和 Store 端口。
//! 领域层不依赖 SeaORM、NapCat 或 LLM；复杂查询在基础设施仓储中实现。

use std::collections::HashSet;

use async_trait::async_trait;

use crate::planner::AgentEventView;
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
    /// 来源行版本，用于后续忽略/推迟等写操作的并发 fencing。
    /// 无版本来源（如 outbox）为 None；缺失时不得用 0 表示。
    pub source_version: Option<u64>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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

    /// JSON 字段值（与 serde 默认序列化一致，PascalCase）。
    pub fn serialized_name(self) -> &'static str {
        match self {
            Self::Owner => "Owner",
            Self::OfficialBot => "OfficialBot",
            Self::External => "External",
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

// ===== 账号作用域参与者（ID-004 / ID-005 / MEM-002）=====
//
// 身份 = SourceAccountRef + PlatformIdentityKind + 平台稳定主体 ID。
// 同一平台 ID 在不同被管理账号下是不同参与者；昵称、群名片、备注和别名
// 只用于显示与指代候选解析，绝不构成授权。群角色只描述群内权限，
// 不提升为系统 Owner。

/// 参与者上下文中的硬上限（约束 7：有界数量、有界字符、有界来源）。
pub const MAX_PARTICIPANT_ALIASES: usize = 10;
pub const MAX_PARTICIPANT_ATTRIBUTES: usize = 10;
pub const MAX_PARTICIPANT_SOURCE_REFS: usize = 10;
pub const MAX_RELATED_EVENT_REFS: usize = 10;
pub const MAX_ATTRIBUTE_VALUE_CHARS: usize = 200;
pub const MAX_CAUSAL_PARTICIPANTS: usize = 20;
/// 结构关系（1 发送 + ≤20 提及 + 1 回复 + 1 线程成员 + 1 线程根）+
/// 语义角色（≤5 要求者 + ≤4 承诺对 × 2 = 8 承诺/受益）的上限。
pub const MAX_CAUSAL_RELATIONS: usize = 40;
pub const MAX_CAUSAL_MENTIONED: usize = 20;
/// 事件 + 回复父 + 线程根 + ≤5 要求来源 + ≤8 承诺来源。
pub const MAX_CAUSAL_SOURCE_REFS: usize = 20;
pub const MAX_RELATION_SOURCES: usize = 3;

/// 账号作用域参与者引用。复用现有 `ParticipantIdentity`（平台种类 + 稳定主体 ID），
/// 不建立第二套平行身份体系；显式携带账号，使跨账号的相同平台 ID 成为不同参与者。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountScopedParticipantRef {
    pub account: SourceAccountRef,
    pub identity: ParticipantIdentity,
}

impl AccountScopedParticipantRef {
    pub fn new(
        account: SourceAccountRef,
        platform_kind: PlatformIdentityKind,
        stable_id: impl Into<String>,
        trust: IdentityTrust,
    ) -> Result<Self, RetrieverError> {
        Ok(Self {
            account,
            identity: ParticipantIdentity::new(platform_kind, stable_id, trust)?,
        })
    }

    pub fn stable_id(&self) -> &str {
        &self.identity.stable_id
    }
}

/// 群角色。只描述参与者在该群内的协议角色，绝不用于判定系统 Owner 或任何授权。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupRole {
    /// 群主（协议字段，仅群内角色）。
    Owner,
    /// 群管理员（协议字段，仅群内角色）。
    Admin,
    /// 普通群成员。
    Member,
    /// 无法确认（私聊、字段缺失或未知值）。
    Unknown,
}

impl GroupRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
            Self::Unknown => "unknown",
        }
    }

    /// 解析协议字段（如 OneBot `role`）。缺失或未知值一律为 `Unknown`，绝不猜测。
    pub fn parse_protocol(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("owner") => Self::Owner,
            Some("admin") => Self::Admin,
            Some("member") => Self::Member,
            _ => Self::Unknown,
        }
    }
}

/// 参与者属性种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantAttributeKind {
    /// 当前显示名（观察或目录）。
    DisplayName,
    /// 群名片。
    GroupCard,
    /// 备注名。
    Remark,
    /// 历史别名（显示名变化前的旧值，有界）。
    HistoricalAlias,
    /// 与 Owner 的关系（已确认人物记忆）。
    Relationship,
    /// 职责（已确认人物记忆）。
    Responsibility,
    /// 权限描述（仅描述；不得覆盖 OwnerBinding / 系统超管 / Action Gate）。
    Permission,
    /// 沟通偏好（已确认人物记忆）。
    CommunicationPreference,
}

impl ParticipantAttributeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DisplayName => "display_name",
            Self::GroupCard => "group_card",
            Self::Remark => "remark",
            Self::HistoricalAlias => "historical_alias",
            Self::Relationship => "relationship",
            Self::Responsibility => "responsibility",
            Self::Permission => "permission",
            Self::CommunicationPreference => "communication_preference",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "display_name" => Some(Self::DisplayName),
            "group_card" => Some(Self::GroupCard),
            "remark" => Some(Self::Remark),
            "historical_alias" => Some(Self::HistoricalAlias),
            "relationship" => Some(Self::Relationship),
            "responsibility" => Some(Self::Responsibility),
            "permission" => Some(Self::Permission),
            "communication_preference" => Some(Self::CommunicationPreference),
            _ => None,
        }
    }
}

/// 单条参与者属性。每条属性独立携带可信等级、来源事件或目录快照引用和失效状态；
/// 召回/删除/失效的来源不得支撑新的人物事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantAttribute {
    pub kind: ParticipantAttributeKind,
    /// 属性值（有界，`MAX_ATTRIBUTE_VALUE_CHARS`）。
    pub value: String,
    pub trust: IdentityTrust,
    /// 已确认标记。只有 Owner 确认、目录快照或已确认人物记忆可置 true；
    /// 低置信语义绝不伪装成已确认。
    pub confirmed: bool,
    /// 支撑来源事件（有界，`MAX_RELATION_SOURCES`）。
    pub source_event_ids: Vec<SourceEventId>,
    /// 目录快照引用（若有）。
    pub directory_snapshot_id: Option<String>,
    pub invalidated: bool,
    pub invalidation_reason: Option<String>,
}

/// 参与者的账号作用域上下文（THR-013 / MEM-002）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantContextView {
    pub participant: AccountScopedParticipantRef,
    /// 当前显示名（账号级昵称，有界）。
    pub display_name: Option<String>,
    /// 群名片或备注（会话作用域观察，有界）。只有按 `conversation`/`thread_id`
    /// 约束查询时才有值；未提供会话时返回 None，绝不跨会话猜测。
    pub group_card: Option<String>,
    /// 历史别名（有界数量，`MAX_PARTICIPANT_ALIASES`）。
    pub aliases: Vec<String>,
    /// 群角色（会话作用域观察）。未提供会话时一律 Unknown。
    pub group_role: GroupRole,
    /// 关系/职责/权限描述/沟通偏好等类型化属性（有界数量）。
    pub attributes: Vec<ParticipantAttribute>,
    pub conversation: Option<ConversationRef>,
    pub thread_id: Option<EventThreadId>,
    /// 最近相关事件（有界，`MAX_RELATED_EVENT_REFS`），只导航，事实由来源回读。
    pub related_event_ids: Vec<SourceEventId>,
    /// 同名候选无法唯一解析时为 true，要求 Owner 澄清。
    pub unresolved_ambiguity: bool,
    /// 全部支撑来源已失效/过期/被删除时为 true；此时不得把旧事实当有效返回。
    pub expired_or_invalidated: bool,
}

// ===== 事件因果关系（THR-011 / THR-012）=====

/// 事件因果关系的类型化种类。严格语义：
/// - 发送者 ≠ 要求者；回复根发送者 ≠ 当前发送者；
/// - 线程根发送者 = 线程发起人（不是 Owner 判定）；
/// - @ 到的人不自动成为负责人；
/// - 负责人/承诺人/受益方只能来自已确认 Thread Claim、承诺记忆或 Owner 确认。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventRelationKind {
    /// 事件由该参与者发送（结构事实）。
    SentBy,
    /// 事件 @ 到该参与者（结构事实，协议只携带 actor_id）。
    Mentions,
    /// 事件回复该参与者发送的父事件（结构事实）。
    RepliesTo,
    /// 事件属于某有效线程（结构事实）。
    MemberOfThread,
    /// 事件是线程根事件，该参与者是线程发起人（结构事实）。
    ThreadRootBy,
    /// 已确认要求由该参与者提出（已确认 Request 声明）。
    RequestedBy,
    /// 已确认负责人（已确认来源/承诺记忆/Owner 确认；"我来处理"必须有来源并语义确认）。
    AssignedTo,
    /// 已确认承诺人（已确认承诺记忆）。
    PromisedBy,
    /// 已确认受益方（已确认承诺记忆）。
    Benefits,
}

impl std::fmt::Display for EventRelationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl EventRelationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SentBy => "sent_by",
            Self::Mentions => "mentions",
            Self::RepliesTo => "replies_to",
            Self::MemberOfThread => "member_of_thread",
            Self::ThreadRootBy => "thread_root_by",
            Self::RequestedBy => "requested_by",
            Self::AssignedTo => "assigned_to",
            Self::PromisedBy => "promised_by",
            Self::Benefits => "benefits",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "sent_by" => Some(Self::SentBy),
            "mentions" => Some(Self::Mentions),
            "replies_to" => Some(Self::RepliesTo),
            "member_of_thread" => Some(Self::MemberOfThread),
            "thread_root_by" => Some(Self::ThreadRootBy),
            "requested_by" => Some(Self::RequestedBy),
            "assigned_to" => Some(Self::AssignedTo),
            "promised_by" => Some(Self::PromisedBy),
            "benefits" => Some(Self::Benefits),
            _ => None,
        }
    }
}

/// 单条类型化事件关系。带账号作用域、种类、主体参与者、来源事件和确认标记；
/// 未确认语义携带 `confirmed=false`，绝不与已确认事实混同。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRelation {
    pub kind: EventRelationKind,
    pub account: SourceAccountRef,
    /// 关系客体参与者（如 RepliesTo 中被回复的发送者、AssignedTo 中的负责人）。
    pub subject: AccountScopedParticipantRef,
    /// MemberOfThread / ThreadRootBy 关联的有效线程。
    pub thread_id: Option<EventThreadId>,
    /// 支撑来源事件（有界，`MAX_RELATION_SOURCES`）。
    pub source_event_ids: Vec<SourceEventId>,
    pub trust: IdentityTrust,
    pub confirmed: bool,
    pub invalidation_reason: Option<String>,
}

/// 因果上下文中的事件引用（如回复父事件）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CausalEventRef {
    pub source_event_id: SourceEventId,
    pub sender: Option<ParticipantIdentity>,
}

/// 因果上下文中的有效线程引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CausalThreadRef {
    pub thread_id: EventThreadId,
    pub status: ThreadStatus,
    pub root_event_id: SourceEventId,
    /// 线程根事件发送者 = 线程发起人。
    pub root_sender: Option<ParticipantIdentity>,
}

/// 线程参与者的有界摘要（发送者集合，不含正文）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventParticipantSummary {
    pub participant: AccountScopedParticipantRef,
    pub display_name: Option<String>,
    pub group_role: GroupRole,
    pub event_count: u64,
}

/// 单事件的账号作用域因果上下文（THR-011/THR-012）。所有字段有界；
/// 角色列表只容纳已确认语义，未确认时为空且 `ambiguous` 标记未决。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventCausalContextView {
    pub source_event_id: SourceEventId,
    pub account: SourceAccountRef,
    /// 事件发送者。
    pub sender: Option<ParticipantIdentity>,
    /// 回复父事件及其发送者（回复根发送者 ≠ 当前发送者）。
    pub reply_parent: Option<CausalEventRef>,
    /// 有效线程及其根事件（根发送者 = 发起人）。
    pub thread: Option<CausalThreadRef>,
    /// @ 到的参与者（协议观察，有界）。@ 到的人绝不自动进入负责人/承诺人/受益方。
    pub mentioned: Vec<AccountScopedParticipantRef>,
    /// 已确认要求者。
    pub requesters: Vec<AccountScopedParticipantRef>,
    /// 已确认负责人。
    pub assignees: Vec<AccountScopedParticipantRef>,
    /// 已确认承诺人。
    pub promisors: Vec<AccountScopedParticipantRef>,
    /// 已确认受益方。
    pub beneficiaries: Vec<AccountScopedParticipantRef>,
    /// 线程参与者有界列表。
    pub participants: Vec<EventParticipantSummary>,
    /// 全部类型化关系（有界，`MAX_CAUSAL_RELATIONS`）。
    pub relations: Vec<EventRelation>,
    /// 语义无法唯一解析（如同名多候选人）时为 true。
    pub ambiguous: bool,
    /// 精确来源引用（有界，`MAX_CAUSAL_SOURCE_REFS`）。
    pub source_refs: Vec<SourceEventId>,
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

// ===== 项目记忆查询（MEM-003）=====

/// 项目记忆的有界摘要条目（`list_projects` 返回）。
/// 只含导航信息；完整上下文通过 `query_project` 获取。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMemorySummary {
    pub project_key: String,
    pub goal: String,
    /// 成员数量（有界；详情在 `ProjectContextView` 中展开）。
    pub member_count: usize,
    pub progress: Option<String>,
    pub risk_count: usize,
    pub blocker_count: usize,
    pub fact_id: crate::MemoryFactId,
    pub updated_at_unix_secs: Option<i64>,
}

/// 单个项目的完整上下文视图（`query_project` 返回）。
/// 所有字段有界；成员携带身份类型；来源事件引用可回读。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContextView {
    pub project_key: String,
    pub goal: String,
    /// 带身份类型的项目成员列表。旧数据回退到 `member_actor_ids` 时，
    /// 身份类型标记为 External（调用方应在展示层标注"未知"）。
    pub members: Vec<crate::ProjectMemberRef>,
    /// 是否为旧数据（仅有 member_actor_ids、无 member_actor_refs）。
    pub legacy_member_ids: bool,
    pub progress: Option<String>,
    pub risks: Vec<String>,
    pub blockers: Vec<String>,
    pub artifact_refs: Vec<String>,
    pub decision_ids: Vec<crate::ThreadDecisionId>,
    pub fact_id: crate::MemoryFactId,
    pub confidence_bps: u16,
    /// 精确来源事件引用（可回读）。
    pub source_event_ids: Vec<SourceEventId>,
    pub valid_until_unix_secs: Option<i64>,
}

// ===== 承诺记忆查询（MEM-004）=====

/// 承诺查询的过滤条件。所有字段可选，组合使用。
#[derive(Debug, Clone)]
pub struct CommitmentQuery {
    pub account: SourceAccountRef,
    /// 承诺状态过滤：pending/fulfilled/cancelled。
    pub status: Option<crate::CommitmentStatus>,
    /// 截止时间范围起始（Unix 秒，含）。
    pub due_since_unix_secs: Option<i64>,
    /// 截止时间范围结束（Unix 秒，含）。
    pub due_until_unix_secs: Option<i64>,
    /// 承诺人过滤（平台身份种类 + 稳定主体 ID）。None = 不过滤；
    /// Some 时 SQL 同时匹配 kind 和 actor_id（旧数据无 kind 字段不命中）。
    pub promisor: Option<crate::ProjectMemberRef>,
    /// 受益方过滤（平台身份种类 + 稳定主体 ID）。None = 不过滤；
    /// Some 时 SQL 同时匹配 kind 和 actor_id。
    pub beneficiary: Option<crate::ProjectMemberRef>,
    /// 返回上限。1..=100。
    pub limit: u16,
}

/// 承诺记忆的有界条目（`list_commitments` 返回）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitmentSummary {
    pub fact_id: crate::MemoryFactId,
    pub promisor: crate::ProjectMemberRef,
    pub beneficiary: crate::ProjectMemberRef,
    pub action: String,
    pub due_at_unix_secs: Option<i64>,
    pub status: crate::CommitmentStatus,
    pub source_event_ids: Vec<SourceEventId>,
    pub follow_up_id: Option<String>,
    pub follow_up_status: Option<String>,
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

    /// 列出账号最近的 N 条事件证据视图，包含发送者、@、Reply、Thread 和内容策略。
    /// 返回按时间正序排列；数据库先倒序取最近 N 条，再反转为正序。
    /// 不应用内容策略过滤（由 `RetrieverUseCase` 层负责）。
    async fn list_recent_event_views(
        &self,
        account: &SourceAccountRef,
        limit: u16,
    ) -> Result<Vec<AgentEventView>, InboundEventStoreError>;

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

    /// 构建单事件的账号作用域因果上下文（THR-011/THR-012）。
    /// 返回 None 表示该账号下不存在此事件。所有 SQL 强制 `account_id` 过滤；
    /// 相同 actor_id / message_id / 昵称跨账号绝不互相关联。
    async fn event_causal_context(
        &self,
        account: &SourceAccountRef,
        source_event_id: &SourceEventId,
    ) -> Result<Option<EventCausalContextView>, InboundEventStoreError>;

    /// 构建参与者的账号作用域上下文（ID-004/ID-005/MEM-002）。
    /// 返回 None 表示该账号内没有任何该参与者的证据。
    /// `conversation`/`thread_id` 用于约束会话作用域观察（群名片/群角色），
    /// 不是原样回显：未提供时群属性返回未知，绝不跨会话猜测。
    /// 身份种类是档案键的一部分：同一账号内相同稳定 ID 存在多个身份命名空间
    /// 的档案时（上游绑定冲突），本查询 fail-closed 返回错误，绝不静默合并。
    async fn participant_context(
        &self,
        account: &SourceAccountRef,
        actor_id: &str,
        conversation: Option<&ConversationRef>,
        thread_id: Option<&EventThreadId>,
    ) -> Result<Option<ParticipantContextView>, InboundEventStoreError>;

    /// 按完整账号作用域参与者引用（账号 + 身份种类 + 稳定 ID）精确读取上下文。
    /// 调用方（如按名解析的 Effect）已知身份种类时必须用本方法：档案按三元组
    /// 精确命中，同账号下相同稳定 ID 的不同身份命名空间不会互相干扰，也不会
    /// 触发宽松查询的歧义拒绝。
    async fn participant_context_by_ref(
        &self,
        participant: &AccountScopedParticipantRef,
        conversation: Option<&ConversationRef>,
        thread_id: Option<&EventThreadId>,
    ) -> Result<Option<ParticipantContextView>, InboundEventStoreError>;

    /// 按显示名/别名/群名片有界解析参与者候选（THR-013 复合查询的第一阶段）。
    /// 同一账号内匹配，跨账号绝不关联；最多返回 `limit`（1..=5）个候选。
    /// 仅用于指代解析，绝不用于授权；解析歧义由调用方要求 Owner 澄清。
    async fn participants_by_display_name(
        &self,
        account: &SourceAccountRef,
        name: &str,
        conversation: Option<&ConversationRef>,
        thread_id: Option<&EventThreadId>,
        limit: u16,
    ) -> Result<Vec<AccountScopedParticipantRef>, InboundEventStoreError>;

    /// 列出当前账号的所有活跃项目记忆（Confirmed、未过期/删除/取代、来源有效）。
    /// 返回有界列表，每个条目只含导航信息；详情通过 `query_project` 获取。
    async fn list_projects(
        &self,
        account: &SourceAccountRef,
        limit: u16,
    ) -> Result<Vec<ProjectMemorySummary>, InboundEventStoreError>;

    /// 查询单个项目的完整上下文：目标、成员、进展、风险、阻塞、决策、Artifact 引用
    /// 和精确来源事件引用。只返回 Confirmed/未过期/来源有效的记忆；
    /// 不同账号的相同 project_key 完全隔离。
    async fn query_project(
        &self,
        account: &SourceAccountRef,
        project_key: &str,
    ) -> Result<Option<ProjectContextView>, InboundEventStoreError>;

    /// 查询承诺记忆（MEM-004 B2）。支持按状态、截止时间范围、参与者过滤；
    /// 返回有界列表，包含关联的 FollowUp 引用。
    async fn list_commitments(
        &self,
        query: &CommitmentQuery,
    ) -> Result<Vec<CommitmentSummary>, InboundEventStoreError>;
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

// ===== 参与者上下文与因果上下文校验（ID-004/ID-005/THR-011/THR-012）=====

/// 校验参与者上下文的所有硬上限（约束 7）。
pub fn validate_participant_context(view: &ParticipantContextView) -> Result<(), RetrieverError> {
    if view.aliases.len() > MAX_PARTICIPANT_ALIASES {
        return Err(RetrieverError::InvalidData(format!(
            "ParticipantContextView.aliases exceeds {}",
            MAX_PARTICIPANT_ALIASES
        )));
    }
    for alias in &view.aliases {
        if alias.chars().count() > MAX_ATTRIBUTE_VALUE_CHARS {
            return Err(RetrieverError::InvalidData(format!(
                "alias exceeds {} chars",
                MAX_ATTRIBUTE_VALUE_CHARS
            )));
        }
    }
    if view.attributes.len() > MAX_PARTICIPANT_ATTRIBUTES {
        return Err(RetrieverError::InvalidData(format!(
            "ParticipantContextView.attributes exceeds {}",
            MAX_PARTICIPANT_ATTRIBUTES
        )));
    }
    if view.related_event_ids.len() > MAX_RELATED_EVENT_REFS {
        return Err(RetrieverError::InvalidData(format!(
            "ParticipantContextView.related_event_ids exceeds {}",
            MAX_RELATED_EVENT_REFS
        )));
    }
    if let Some(display) = &view.display_name
        && display.chars().count() > MAX_ATTRIBUTE_VALUE_CHARS
    {
        return Err(RetrieverError::InvalidData("display_name too long".into()));
    }
    if let Some(card) = &view.group_card
        && card.chars().count() > MAX_ATTRIBUTE_VALUE_CHARS
    {
        return Err(RetrieverError::InvalidData("group_card too long".into()));
    }
    validate_attributes(&view.attributes)
}

/// 校验参与者上下文中的权限边界（Section 四/七）：
/// 权限属性只能来自系统 Owner 的已验证账号绑定；昵称、群名片、群角色
/// （含群主/管理员）或任何聊天内容、LLM 推断都不构成权限属性。
/// 返回违反描述列表；空列表表示通过。
pub fn check_participant_permission_boundary(view: &ParticipantContextView) -> Vec<String> {
    let mut violations = Vec::new();
    for attribute in &view.attributes {
        if attribute.kind == ParticipantAttributeKind::Permission {
            let identity = &view.participant.identity;
            let allowed = identity.platform_kind == PlatformIdentityKind::Owner
                && identity.trust == IdentityTrust::Verified;
            if !allowed {
                violations.push(format!(
                    "permission attribute {} is not backed by verified owner binding",
                    attribute.value.chars().take(80).collect::<String>()
                ));
            }
        }
    }
    violations
}

/// 校验事件因果上下文的所有硬上限（约束 7）。
pub fn validate_causal_context(view: &EventCausalContextView) -> Result<(), RetrieverError> {
    if view.mentioned.len() > MAX_CAUSAL_MENTIONED {
        return Err(RetrieverError::InvalidData(format!(
            "EventCausalContextView.mentioned exceeds {}",
            MAX_CAUSAL_MENTIONED
        )));
    }
    if view.participants.len() > MAX_CAUSAL_PARTICIPANTS {
        return Err(RetrieverError::InvalidData(format!(
            "EventCausalContextView.participants exceeds {}",
            MAX_CAUSAL_PARTICIPANTS
        )));
    }
    if view.relations.len() > MAX_CAUSAL_RELATIONS {
        return Err(RetrieverError::InvalidData(format!(
            "EventCausalContextView.relations exceeds {}",
            MAX_CAUSAL_RELATIONS
        )));
    }
    if view.source_refs.len() > MAX_CAUSAL_SOURCE_REFS {
        return Err(RetrieverError::InvalidData(format!(
            "EventCausalContextView.source_refs exceeds {}",
            MAX_CAUSAL_SOURCE_REFS
        )));
    }
    for relation in &view.relations {
        if relation.source_event_ids.len() > MAX_RELATION_SOURCES {
            return Err(RetrieverError::InvalidData(format!(
                "EventRelation {} source_event_ids exceeds {}",
                relation.kind.as_str(),
                MAX_RELATION_SOURCES
            )));
        }
    }
    Ok(())
}

/// 严格角色语义校验（THR-012）。返回违反描述列表；空列表表示通过。
/// 校验的不变量：
/// 1. 角色列表（要求者/负责人/承诺人/受益方）中的参与者必须被对应种类的
///    已确认关系支撑 —— 被 @ 到不等于被指派，仅 Mentions 证据不足以支撑角色；
/// 2. 已确认角色关系必须携带可回读来源，且来源出现在 `source_refs` 中；
/// 3. 未确认的关系（`confirmed=false`）绝不进入已确认角色列表。
///
/// “提及 ≠ 指派”同时由仓储构造保证：@ 段只产生 Mentions 关系，
/// 角色关系只从已确认 Thread Claim / 承诺记忆 / Owner 确认派生。
pub fn check_causal_role_strictness(view: &EventCausalContextView) -> Vec<String> {
    let mut violations = Vec::new();

    let confirmed_roles: Vec<&EventRelation> = view
        .relations
        .iter()
        .filter(|r| {
            r.confirmed
                && matches!(
                    r.kind,
                    EventRelationKind::RequestedBy
                        | EventRelationKind::AssignedTo
                        | EventRelationKind::PromisedBy
                        | EventRelationKind::Benefits
                )
        })
        .collect();

    // 不变量 1：未确认语义不得出现在已确认角色列表中。
    let role_lists: [(&str, &[AccountScopedParticipantRef], EventRelationKind); 4] = [
        (
            "requesters",
            &view.requesters,
            EventRelationKind::RequestedBy,
        ),
        ("assignees", &view.assignees, EventRelationKind::AssignedTo),
        ("promisors", &view.promisors, EventRelationKind::PromisedBy),
        (
            "beneficiaries",
            &view.beneficiaries,
            EventRelationKind::Benefits,
        ),
    ];
    for (list_name, list, kind) in role_lists {
        for participant in list {
            let supported = confirmed_roles.iter().any(|r| {
                r.kind == kind
                    && r.subject.account == participant.account
                    && r.subject.identity.stable_id == participant.identity.stable_id
            });
            if !supported {
                violations.push(format!(
                    "{list_name} contains {} without a confirmed {kind} relation",
                    participant.identity.stable_id
                ));
            }
        }
    }

    // 不变量 2：已确认角色关系必须携带可回读来源。
    for relation in &confirmed_roles {
        if relation.source_event_ids.is_empty()
            || !relation
                .source_event_ids
                .iter()
                .any(|id| view.source_refs.contains(id))
        {
            violations.push(format!(
                "confirmed {} relation lacks a readable source ref",
                relation.kind.as_str()
            ));
        }
    }
    violations
}

/// 判定参与者是否拥有系统 Owner 权限。Owner 权限只能来自已验证的账号绑定
/// 或平台签名验证；昵称、群名片、群角色（含群主/管理员）、聊天内容
/// 或 LLM 推断一律不构成授权。任何输入都返回确定的 bool，绝不猜测。
pub fn grants_owner_authority(
    kind: PlatformIdentityKind,
    trust: IdentityTrust,
    _group_role: GroupRole,
) -> bool {
    kind == PlatformIdentityKind::Owner && trust == IdentityTrust::Verified
}

fn validate_attributes(attributes: &[ParticipantAttribute]) -> Result<(), RetrieverError> {
    for attribute in attributes {
        if attribute.value.chars().count() > MAX_ATTRIBUTE_VALUE_CHARS {
            return Err(RetrieverError::InvalidData(format!(
                "ParticipantAttribute {} value exceeds {} chars",
                attribute.kind.as_str(),
                MAX_ATTRIBUTE_VALUE_CHARS
            )));
        }
        if attribute.source_event_ids.len() > MAX_RELATION_SOURCES {
            return Err(RetrieverError::InvalidData(format!(
                "ParticipantAttribute {} source_event_ids exceeds {}",
                attribute.kind.as_str(),
                MAX_RELATION_SOURCES
            )));
        }
        if attribute.confirmed && attribute.source_event_ids.is_empty() {
            // 目录快照或 Owner 绑定可作来源；只有完全没有来源时才拒绝。
            if attribute.directory_snapshot_id.is_none() {
                return Err(RetrieverError::InvalidData(format!(
                    "confirmed ParticipantAttribute {} lacks any source",
                    attribute.kind.as_str()
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MessageSource, SourceAccountRef};

    fn account() -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, "account-1").unwrap()
    }

    fn actor_ref(account: &SourceAccountRef, id: &str) -> AccountScopedParticipantRef {
        AccountScopedParticipantRef::new(
            account.clone(),
            PlatformIdentityKind::External,
            id,
            IdentityTrust::Observed,
        )
        .unwrap()
    }

    fn role_relation(
        account: &SourceAccountRef,
        kind: EventRelationKind,
        subject: &AccountScopedParticipantRef,
        source_event_id: &str,
        confirmed: bool,
    ) -> EventRelation {
        EventRelation {
            kind,
            account: account.clone(),
            subject: subject.clone(),
            thread_id: None,
            source_event_ids: vec![SourceEventId::new(source_event_id).unwrap()],
            trust: IdentityTrust::Verified,
            confirmed,
            invalidation_reason: None,
        }
    }

    /// 9.1 域表驱动测试：严格角色分离、未确认语义不升级、昵称/群角色不授权。
    #[test]
    fn causal_role_strictness_table_driven() {
        let account = account();
        let alice = actor_ref(&account, "alice-10001");
        let bob = actor_ref(&account, "bob-10002");
        let carol = actor_ref(&account, "carol-10003");
        let event = SourceEventId::new("evt-1").unwrap();

        // 合法视图：发送者 alice；回复根发送者 ≠ 当前发送者；@ 到 carol；
        // 已确认要求者 alice 带来源；carol 仅被提及，绝不自动成为负责人。
        let legal = EventCausalContextView {
            source_event_id: event.clone(),
            account: account.clone(),
            sender: Some(
                ParticipantIdentity::new(
                    PlatformIdentityKind::External,
                    "alice-10001",
                    IdentityTrust::Observed,
                )
                .unwrap(),
            ),
            reply_parent: None,
            thread: None,
            mentioned: vec![carol.clone()],
            requesters: vec![alice.clone()],
            assignees: Vec::new(),
            promisors: Vec::new(),
            beneficiaries: Vec::new(),
            participants: Vec::new(),
            relations: vec![
                role_relation(&account, EventRelationKind::SentBy, &alice, "evt-1", true),
                role_relation(&account, EventRelationKind::Mentions, &carol, "evt-1", true),
                role_relation(
                    &account,
                    EventRelationKind::RequestedBy,
                    &alice,
                    "evt-1",
                    true,
                ),
            ],
            ambiguous: false,
            source_refs: vec![event.clone()],
        };
        assert!(
            check_causal_role_strictness(&legal).is_empty(),
            "合法角色视图不应有违规"
        );

        // 违规 1：carol 仅被 @（Mentions），却出现在负责人列表且无已确认关系 → 提及 ≠ 指派。
        let mention_is_not_assignee = EventCausalContextView {
            assignees: vec![carol.clone()],
            ..legal.clone()
        };
        let violations = check_causal_role_strictness(&mention_is_not_assignee);
        assert!(
            violations.iter().any(|v| v.contains("assignees")),
            "被@参与者不得自动成为负责人: {violations:?}"
        );

        // 违规 2：未确认（confirmed=false）的 RequestedBy 进入要求者列表 → 低置信不升级。
        let unconfirmed_semantics = EventCausalContextView {
            requesters: vec![bob.clone()],
            relations: vec![role_relation(
                &account,
                EventRelationKind::RequestedBy,
                &bob,
                "evt-1",
                false,
            )],
            ..legal.clone()
        };
        let violations = check_causal_role_strictness(&unconfirmed_semantics);
        assert!(
            violations.iter().any(|v| v.contains("requesters")),
            "未确认语义不得伪装成已确认: {violations:?}"
        );

        // 违规 3：已确认角色关系缺少可回读来源。
        let missing_source = EventCausalContextView {
            source_refs: Vec::new(),
            ..legal.clone()
        };
        let violations = check_causal_role_strictness(&missing_source);
        assert!(
            violations.iter().any(|v| v.contains("source")),
            "已确认角色关系必须带可回读来源: {violations:?}"
        );
    }

    /// 9.1 权限边界：昵称、群名片、群角色（含群主/管理员）绝不构成 Owner 权限。
    #[test]
    fn nickname_and_group_role_never_grant_owner_permission() {
        let account = account();

        // 只有"已验证的 Owner 身份"才拥有系统 Owner 权限。
        assert!(grants_owner_authority(
            PlatformIdentityKind::Owner,
            IdentityTrust::Verified,
            GroupRole::Member
        ));
        // 群主/管理员/普通成员 + 观察身份 ≠ Owner。
        for role in [GroupRole::Owner, GroupRole::Admin, GroupRole::Member] {
            assert!(!grants_owner_authority(
                PlatformIdentityKind::External,
                IdentityTrust::Observed,
                role
            ));
            assert!(!grants_owner_authority(
                PlatformIdentityKind::External,
                IdentityTrust::Inferred,
                role
            ));
        }
        // 协议字段解析：未知值一律 Unknown，绝不猜测。
        assert_eq!(GroupRole::parse_protocol(Some("owner")), GroupRole::Owner);
        assert_eq!(GroupRole::parse_protocol(Some("admin")), GroupRole::Admin);
        assert_eq!(GroupRole::parse_protocol(Some("member")), GroupRole::Member);
        assert_eq!(GroupRole::parse_protocol(None), GroupRole::Unknown);
        assert_eq!(
            GroupRole::parse_protocol(Some("super_admin")),
            GroupRole::Unknown
        );

        // 参与者上下文权限边界：昵称/群角色派生的 Permission 属性被拒绝。
        let nickname_only = ParticipantContextView {
            participant: AccountScopedParticipantRef::new(
                account.clone(),
                PlatformIdentityKind::External,
                "bob-10002",
                IdentityTrust::Observed,
            )
            .unwrap(),
            display_name: Some("群主小明".into()),
            group_card: None,
            aliases: Vec::new(),
            group_role: GroupRole::Owner,
            attributes: vec![ParticipantAttribute {
                kind: ParticipantAttributeKind::Permission,
                value: "群主".into(),
                trust: IdentityTrust::Observed,
                confirmed: false,
                source_event_ids: vec![SourceEventId::new("evt-1").unwrap()],
                directory_snapshot_id: None,
                invalidated: false,
                invalidation_reason: None,
            }],
            conversation: None,
            thread_id: None,
            related_event_ids: Vec::new(),
            unresolved_ambiguity: false,
            expired_or_invalidated: false,
        };
        let violations = check_participant_permission_boundary(&nickname_only);
        assert!(
            violations.iter().any(|v| v.contains("permission")),
            "昵称/群角色派生的权限属性必须被拒绝: {violations:?}"
        );

        // 已验证 Owner 的权限属性被允许。
        let verified_owner = ParticipantContextView {
            participant: AccountScopedParticipantRef::new(
                account.clone(),
                PlatformIdentityKind::Owner,
                "owner-1",
                IdentityTrust::Verified,
            )
            .unwrap(),
            display_name: None,
            group_card: None,
            aliases: Vec::new(),
            group_role: GroupRole::Unknown,
            attributes: vec![ParticipantAttribute {
                kind: ParticipantAttributeKind::Permission,
                value: "系统 Owner（账号绑定）".into(),
                trust: IdentityTrust::Verified,
                confirmed: true,
                source_event_ids: Vec::new(),
                directory_snapshot_id: None,
                invalidated: false,
                invalidation_reason: None,
            }],
            conversation: None,
            thread_id: None,
            related_event_ids: Vec::new(),
            unresolved_ambiguity: false,
            expired_or_invalidated: false,
        };
        assert!(check_participant_permission_boundary(&verified_owner).is_empty());
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
