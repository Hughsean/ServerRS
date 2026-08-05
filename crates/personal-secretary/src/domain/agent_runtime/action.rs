//! 类型化动作白名单：Agent 只能选择白名单中的动作，不能构造任意
//! SQL、HTTP、Shell 或文件操作。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ContentTrustLevel, ConversationRef, MemoryCandidateId, MemoryCandidateKind,
    MemoryCandidateStatus, MemoryFactId, MemoryPayload, NotificationCandidateRef,
    NotificationMatchKeyV1, NotificationOutcome, PolicyFamilyId, QuietHoursRule, SourceEventId,
};

use super::state::SecretaryAgentUpdate;
use super::validation::{SecretaryAgentRuntimeError, validate_action_proposal};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretaryRiskLevel {
    L0ReadOnly,
    L1Reversible,
    L2Impactful,
    L3ExternalSideEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretaryToolKind {
    SearchRecentEvents,
    ReadSourceEvent,
    SearchEventThreads,
    ResolveReference,
    ListUpcomingItems,
    GetSecretaryStatus,
    ListPendingOwnerWork,
    GetThreadContext,
    GetEventCausalContext,
    GetParticipantContext,
    GetParticipantContextByName,
    DraftReminder,
    CreateSchedule,
    RescheduleItem,
    CancelItem,
    CreateTask,
    CreateReminder,
    CompleteItem,
    SnoozeItem,
    SendOwnerMessage,
    AskOwnerClarification,
    ListNotificationPolicies,
    ExplainNotificationDecision,
    SetAccountDefaultNotificationMode,
    SetConversationNotificationMode,
    SetQuietHours,
    SetImportantContact,
    SetNotificationCategoryImportance,
    RecordNotificationFeedback,
    CreateSimilarNotificationRule,
    DisableNotificationPolicy,
    SetAutomaticReplyDeniedForContact,
    ListMemoryFacts,
    ReadMemoryFactSources,
    CorrectMemoryFact,
    DeleteMemoryFact,
    SetMemoryFactTtl,
    SetConversationMemoryMode,
    ConfirmThreadDecision,
    RevokeThreadDecision,
    DismissThreadQuestion,
    SetThreadLifecycle,
    DismissFollowUp,
    SnoozeFollowUp,
    DismissFollowUps,
    SnoozeFollowUps,
    CompleteFollowUp,
    CompleteFollowUps,
    DismissResponseExpectation,
    DismissResponseExpectations,
    ListMemoryCandidates,
    ApproveMemoryCandidate,
    RejectMemoryCandidate,
    ListThreadLinkCandidates,
    ListProjects,
    QueryProject,
    ListCommitments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretaryToolPolicy {
    pub risk: SecretaryRiskLevel,
    pub requires_confirmation: bool,
    pub reversible: bool,
    pub timeout_ms: u64,
    pub max_retries: u8,
}

impl SecretaryToolKind {
    pub fn policy(self) -> SecretaryToolPolicy {
        use SecretaryRiskLevel::{L0ReadOnly, L1Reversible, L2Impactful, L3ExternalSideEffect};
        match self {
            Self::SearchRecentEvents
            | Self::ReadSourceEvent
            | Self::SearchEventThreads
            | Self::ResolveReference
            | Self::ListUpcomingItems
            | Self::GetSecretaryStatus
            | Self::ListPendingOwnerWork
            | Self::GetThreadContext
            | Self::GetEventCausalContext
            | Self::GetParticipantContext
            | Self::GetParticipantContextByName
            | Self::ListNotificationPolicies
            | Self::ExplainNotificationDecision
            | Self::ListMemoryFacts
            | Self::ReadMemoryFactSources
            | Self::ListMemoryCandidates
            | Self::ListThreadLinkCandidates
            | Self::ListProjects
            | Self::QueryProject
            | Self::ListCommitments => SecretaryToolPolicy {
                risk: L0ReadOnly,
                requires_confirmation: false,
                reversible: true,
                timeout_ms: 10_000,
                max_retries: 2,
            },
            Self::DraftReminder | Self::RecordNotificationFeedback => SecretaryToolPolicy {
                risk: L1Reversible,
                requires_confirmation: false,
                reversible: true,
                timeout_ms: 5_000,
                max_retries: 1,
            },
            Self::CreateSchedule
            | Self::RescheduleItem
            | Self::CancelItem
            | Self::CreateTask
            | Self::CreateReminder
            | Self::CompleteItem
            | Self::SnoozeItem
            | Self::SetAccountDefaultNotificationMode
            | Self::SetConversationNotificationMode
            | Self::SetQuietHours
            | Self::SetImportantContact
            | Self::SetNotificationCategoryImportance
            | Self::CreateSimilarNotificationRule
            | Self::DisableNotificationPolicy
            | Self::SetAutomaticReplyDeniedForContact => SecretaryToolPolicy {
                risk: L2Impactful,
                requires_confirmation: true,
                reversible: true,
                timeout_ms: 15_000,
                max_retries: 1,
            },
            Self::CorrectMemoryFact
            | Self::DeleteMemoryFact
            | Self::SetMemoryFactTtl
            | Self::SetConversationMemoryMode
            | Self::ConfirmThreadDecision
            | Self::RevokeThreadDecision
            | Self::DismissThreadQuestion
            | Self::SetThreadLifecycle
            | Self::DismissFollowUp
            | Self::SnoozeFollowUp
            | Self::DismissFollowUps
            | Self::SnoozeFollowUps => SecretaryToolPolicy {
                risk: L2Impactful,
                requires_confirmation: true,
                reversible: true,
                timeout_ms: 15_000,
                max_retries: 1,
            },
            Self::CompleteFollowUp
            | Self::CompleteFollowUps
            | Self::DismissResponseExpectation
            | Self::DismissResponseExpectations
            | Self::ApproveMemoryCandidate
            | Self::RejectMemoryCandidate => SecretaryToolPolicy {
                risk: L2Impactful,
                requires_confirmation: true,
                // v1 没有自动撤销入口；不能向 Owner 暗示完成、关闭或审批可以自动恢复。
                reversible: false,
                timeout_ms: 15_000,
                max_retries: 1,
            },
            Self::SendOwnerMessage => SecretaryToolPolicy {
                risk: L3ExternalSideEffect,
                requires_confirmation: true,
                reversible: false,
                timeout_ms: 30_000,
                max_retries: 0,
            },
            Self::AskOwnerClarification => SecretaryToolPolicy {
                risk: L0ReadOnly,
                requires_confirmation: false,
                reversible: true,
                timeout_ms: 0,
                max_retries: 0,
            },
        }
    }
}

/// 批量忽略动作的单个目标；`expected_source_version` 必须来自
/// ListPendingOwnerWork 展示的 version N，落库时与行内 source_version CAS 比较。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowUpControlTarget {
    pub follow_up_id: crate::FollowUpId,
    pub expected_source_version: u64,
}

/// 批量关闭回复期待动作的单个目标；`expected_source_version` 必须来自
/// ListPendingOwnerWork 展示的 version N，落库时与行内 source_version CAS 比较。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseExpectationControlTarget {
    pub expectation_id: crate::ResponseExpectationId,
    pub expected_source_version: u64,
}

/// Agent 只能选择白名单中的类型化动作，不能构造任意 SQL、HTTP、Shell 或文件操作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum SecretaryAction {
    /// 有界事件搜索（CMD-009 目标 B）。名称保留 `SearchRecentEvents` 以兼容旧序列化，
    /// 语义已扩展为有界事件搜索：未指定 `since_unix_secs` 时可检索 24 小时以前的
    /// 长期事件，不暗中补 24 小时下限；排序确定（硬过滤 → 文本相关性 → 时间 →
    /// source_event_id）。旧 JSON（只有 query/limit）通过 serde(default) 兼容。
    SearchRecentEvents {
        query: String,
        limit: u16,
        /// 起始时间（Unix 秒，含）。None = 不限。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since_unix_secs: Option<i64>,
        /// 截止时间（Unix 秒，含）。None = 不限；指定时不得无理由越过可信当前时间。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until_unix_secs: Option<i64>,
        /// 会话硬过滤（账号作用域内；OwnerCommand 初始检索默认不限定会话）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation: Option<ConversationRef>,
        /// 线程硬过滤。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<crate::EventThreadId>,
        /// 发送者 Actor 稳定 ID 硬过滤。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor_id: Option<String>,
    },
    ReadSourceEvent {
        source_event_id: SourceEventId,
    },
    SearchEventThreads {
        query: String,
        limit: u16,
    },
    /// 解析非显式指代（"他""那条消息"等）。CMD-010 防线 C：
    /// 默认只能在显式作用域（已登记 conversation_ref/thread_ref）内解析；
    /// 无作用域时不猜唯一解，返回澄清/OpenReference。
    ResolveReference {
        expression: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_ref: Option<crate::ConversationRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<crate::EventThreadId>,
    },
    ListUpcomingItems {
        horizon_secs: u64,
    },
    GetSecretaryStatus,
    ListPendingOwnerWork {
        limit: u16,
    },
    GetThreadContext {
        thread_id: crate::EventThreadId,
    },
    /// 读取单事件的账号作用域因果上下文（THR-011/THR-012，L0 只读）。
    GetEventCausalContext {
        source_event_id: crate::SourceEventId,
    },
    /// 读取参与者的账号作用域上下文（ID-004/ID-005/MEM-002，L0 只读）。
    GetParticipantContext {
        /// 身份种类与稳定 ID 构成完整三元组身份：档案按 (account, kind, actor_id)
        /// 精确读取，同账号下不同身份命名空间的相同 ID 不合并；由 TempRefMap
        /// 从模型输出的 actor_ref 恢复，模型永远不直接输出真实 ID。
        actor_kind: crate::PlatformIdentityKind,
        actor_id: String,
        conversation_ref: Option<crate::ConversationRef>,
        thread_id: Option<crate::EventThreadId>,
    },
    /// 按显示名/别名/群名片解析人物并读取上下文（THR-013 复合查询，L0 只读）。
    /// 解决"张三负责什么"式名字查询无法两轮执行的不可达链：解析与上下文在一个
    /// 动作内完成；歧义时返回有界候选列表要求 Owner 澄清。
    GetParticipantContextByName {
        name: String,
        conversation_ref: Option<crate::ConversationRef>,
        thread_id: Option<crate::EventThreadId>,
    },
    DraftReminder {
        text: String,
        due_at_unix: i64,
    },
    CreateSchedule {
        title: String,
        starts_at_unix: i64,
        timezone: String,
    },
    RescheduleItem {
        item_id: String,
        expected_version: u64,
        starts_at_unix: i64,
        timezone: String,
    },
    CancelItem {
        item_id: String,
        expected_version: u64,
        reason: String,
    },
    CreateTask {
        title: String,
        due_at_unix: Option<i64>,
        timezone: String,
    },
    CreateReminder {
        text: String,
        due_at_unix: i64,
        timezone: String,
    },
    CompleteItem {
        item_id: String,
        expected_version: u64,
    },
    SnoozeItem {
        item_id: String,
        expected_version: u64,
        due_at_unix: i64,
        timezone: String,
    },
    SendOwnerMessage {
        text: String,
    },
    AskOwnerClarification {
        question: String,
    },
    ListNotificationPolicies {
        limit: u16,
    },
    ExplainNotificationDecision {
        decision_id: String,
    },
    SetAccountDefaultNotificationMode {
        canonical_scope_key: String,
        match_key: NotificationMatchKeyV1,
        outcome: NotificationOutcome,
        bypass_quiet: bool,
    },
    SetConversationNotificationMode {
        canonical_scope_key: String,
        match_key: NotificationMatchKeyV1,
        outcome: NotificationOutcome,
        bypass_quiet: bool,
        fully_silent: bool,
        allow_bypass: bool,
    },
    SetQuietHours {
        canonical_scope_key: String,
        match_key: NotificationMatchKeyV1,
        quiet_hours: QuietHoursRule,
    },
    SetImportantContact {
        canonical_scope_key: String,
        match_key: NotificationMatchKeyV1,
        outcome: NotificationOutcome,
        bypass_quiet: bool,
    },
    SetNotificationCategoryImportance {
        canonical_scope_key: String,
        match_key: NotificationMatchKeyV1,
        outcome: NotificationOutcome,
        bypass_quiet: bool,
    },
    RecordNotificationFeedback {
        candidate: NotificationCandidateRef,
        match_key: NotificationMatchKeyV1,
        important: bool,
        promote_to_rule: bool,
    },
    CreateSimilarNotificationRule {
        canonical_scope_key: String,
        match_key: NotificationMatchKeyV1,
        outcome: NotificationOutcome,
        bypass_quiet: bool,
    },
    DisableNotificationPolicy {
        policy_family_id: PolicyFamilyId,
        expected_generation: u64,
    },
    SetAutomaticReplyDeniedForContact {
        canonical_scope_key: String,
        match_key: NotificationMatchKeyV1,
    },
    ListMemoryFacts {
        limit: u16,
    },
    ReadMemoryFactSources {
        fact_id: MemoryFactId,
        max_excerpt_chars: u16,
    },
    CorrectMemoryFact {
        fact_id: MemoryFactId,
        replacement: MemoryPayload,
        confidence_bps: u16,
        source_event_ids: Vec<SourceEventId>,
        valid_until_unix_secs: Option<i64>,
    },
    DeleteMemoryFact {
        fact_id: MemoryFactId,
        reason: String,
    },
    SetMemoryFactTtl {
        fact_id: MemoryFactId,
        valid_until_unix_secs: Option<i64>,
    },
    SetConversationMemoryMode {
        conversation: ConversationRef,
        mode: ContentTrustLevel,
    },
    ConfirmThreadDecision {
        decision_id: crate::ThreadDecisionId,
    },
    RevokeThreadDecision {
        decision_id: crate::ThreadDecisionId,
        reason: String,
    },
    DismissThreadQuestion {
        question_id: crate::OpenQuestionId,
        reason: String,
    },
    SetThreadLifecycle {
        thread_id: crate::EventThreadId,
        expected_status: crate::ThreadStatus,
        target_status: crate::ThreadStatus,
        reason: String,
    },
    DismissFollowUp {
        follow_up_id: crate::FollowUpId,
        /// 审批时刻的期望来源版本（>= 1），落库时与行内 source_version CAS 比较。
        expected_source_version: u64,
        reason: String,
    },
    SnoozeFollowUp {
        follow_up_id: crate::FollowUpId,
        /// 审批时刻的期望来源版本（>= 1），落库时与行内 source_version CAS 比较。
        expected_source_version: u64,
        /// 新的到期时间（UTC Unix 秒）；执行时以数据库当前 UTC 时间复验，
        /// 必须晚于当前 due 且不超过数据库当前时间后 365 天。
        snooze_until_unix_secs: i64,
        reason: String,
    },
    /// 批量忽略。targets 数量必须为 1..=20 且 follow_up_id 不重复；
    /// 全有或全无，任一目标校验失败则整个事务回滚。
    DismissFollowUps {
        targets: Vec<FollowUpControlTarget>,
        reason: String,
    },
    /// 批量推迟：整批目标共用同一个新到期时间；若需要不同时间应拆成多个 Action。
    /// targets 数量必须为 1..=20 且 follow_up_id 不重复；全有或全无，
    /// 任一目标校验失败则整个事务回滚。执行时以数据库当前 UTC 时间复验新时间：
    /// 必须晚于每个目标当前 due 且不超过数据库当前时间后 365 天。
    SnoozeFollowUps {
        targets: Vec<FollowUpControlTarget>,
        snooze_until_unix_secs: i64,
        reason: String,
    },
    /// 单条完成：Owner 明确确认承诺或跟进事项已经完成。
    /// 落库后 scheduled -> completed，source_version 精确 +1，due 不变；
    /// 关联通知被压制，Scheduler 不得重新创建该事项。
    CompleteFollowUp {
        follow_up_id: crate::FollowUpId,
        /// 审批时刻的期望来源版本（>= 1），落库时与行内 source_version CAS 比较。
        expected_source_version: u64,
        reason: String,
    },
    /// 批量完成：targets 数量必须为 1..=20 且 follow_up_id 不重复；
    /// 全有或全无，任一目标校验失败则整个事务回滚。
    CompleteFollowUps {
        targets: Vec<FollowUpControlTarget>,
        reason: String,
    },
    /// 单条关闭回复期待：Owner 明确表示不再需要继续提醒回复，不是声称已经回复。
    /// 落库后 active -> dismissed，source_version 精确 +1，due 不变；
    /// 不修改原始聊天消息、EventThread、OpenQuestion 状态或已投递通知；
    /// `resolved` 只保留给真实回复、问题关闭或线程终态等自动事实路径。
    DismissResponseExpectation {
        expectation_id: crate::ResponseExpectationId,
        /// 审批时刻的期望来源版本（>= 1），落库时与行内 source_version CAS 比较。
        expected_source_version: u64,
        reason: String,
    },
    /// 批量关闭回复期待：targets 数量必须为 1..=20 且 expectation_id 不重复；
    /// 全有或全无，任一目标校验失败则整个事务回滚。
    DismissResponseExpectations {
        targets: Vec<ResponseExpectationControlTarget>,
        reason: String,
    },
    /// 列出当前账号的结构化记忆候选。status/kind 可选过滤；limit 1..=100。
    /// 只读，不回显完整聊天正文。
    ListMemoryCandidates {
        status: Option<MemoryCandidateStatus>,
        kind: Option<MemoryCandidateKind>,
        limit: u16,
    },
    /// 列出当前账号待 Owner 确认的跨会话线程关联候选。所有候选保持
    /// `proposed`；置信度只用于确认话术，绝不自动合并线程。
    ListThreadLinkCandidates {
        limit: u16,
    },
    /// 批准一个记忆候选：候选 proposal -> approved（版本精确 +1），并原子写入
    /// Confirmed MemoryFact 与精确来源。没有自动撤销入口。
    /// expected_candidate_version 必须来自 ListMemoryCandidates 展示的版本 N。
    ApproveMemoryCandidate {
        candidate_id: MemoryCandidateId,
        expected_candidate_version: u64,
        reason: String,
    },
    /// 拒绝一个记忆候选：proposal -> rejected（版本精确 +1），只写审计与 Receipt，
    /// 不创建 MemoryFact/FollowUp/Outbox。没有自动撤销入口。
    RejectMemoryCandidate {
        candidate_id: MemoryCandidateId,
        expected_candidate_version: u64,
        reason: String,
    },
    /// 列出当前账号的所有活跃项目记忆（L0 只读，有界）。
    ListProjects {
        limit: u16,
    },
    /// 查询单个项目的完整上下文：目标、成员、进展、风险、阻塞、决策和来源（L0 只读）。
    QueryProject {
        project_key: String,
    },
    /// 查询承诺记忆（MEM-004 B2）。支持按状态、截止时间、参与者过滤。
    ListCommitments {
        status: Option<crate::CommitmentStatus>,
        due_since_unix_secs: Option<i64>,
        due_until_unix_secs: Option<i64>,
        /// 承诺人过滤（平台身份种类 + 稳定主体 ID）。None = 不过滤。
        promisor: Option<crate::ProjectMemberRef>,
        /// 受益方过滤（平台身份种类 + 稳定主体 ID）。None = 不过滤。
        beneficiary: Option<crate::ProjectMemberRef>,
        limit: u16,
    },
}

impl SecretaryAction {
    pub fn kind(&self) -> SecretaryToolKind {
        match self {
            Self::SearchRecentEvents { .. } => SecretaryToolKind::SearchRecentEvents,
            Self::ReadSourceEvent { .. } => SecretaryToolKind::ReadSourceEvent,
            Self::SearchEventThreads { .. } => SecretaryToolKind::SearchEventThreads,
            Self::ResolveReference { .. } => SecretaryToolKind::ResolveReference,
            Self::ListUpcomingItems { .. } => SecretaryToolKind::ListUpcomingItems,
            Self::GetSecretaryStatus => SecretaryToolKind::GetSecretaryStatus,
            Self::ListPendingOwnerWork { .. } => SecretaryToolKind::ListPendingOwnerWork,
            Self::GetThreadContext { .. } => SecretaryToolKind::GetThreadContext,
            Self::GetEventCausalContext { .. } => SecretaryToolKind::GetEventCausalContext,
            Self::GetParticipantContext { .. } => SecretaryToolKind::GetParticipantContext,
            Self::GetParticipantContextByName { .. } => {
                SecretaryToolKind::GetParticipantContextByName
            }
            Self::DraftReminder { .. } => SecretaryToolKind::DraftReminder,
            Self::CreateSchedule { .. } => SecretaryToolKind::CreateSchedule,
            Self::RescheduleItem { .. } => SecretaryToolKind::RescheduleItem,
            Self::CancelItem { .. } => SecretaryToolKind::CancelItem,
            Self::CreateTask { .. } => SecretaryToolKind::CreateTask,
            Self::CreateReminder { .. } => SecretaryToolKind::CreateReminder,
            Self::CompleteItem { .. } => SecretaryToolKind::CompleteItem,
            Self::SnoozeItem { .. } => SecretaryToolKind::SnoozeItem,
            Self::SendOwnerMessage { .. } => SecretaryToolKind::SendOwnerMessage,
            Self::AskOwnerClarification { .. } => SecretaryToolKind::AskOwnerClarification,
            Self::ListNotificationPolicies { .. } => SecretaryToolKind::ListNotificationPolicies,
            Self::ExplainNotificationDecision { .. } => {
                SecretaryToolKind::ExplainNotificationDecision
            }
            Self::SetAccountDefaultNotificationMode { .. } => {
                SecretaryToolKind::SetAccountDefaultNotificationMode
            }
            Self::SetConversationNotificationMode { .. } => {
                SecretaryToolKind::SetConversationNotificationMode
            }
            Self::SetQuietHours { .. } => SecretaryToolKind::SetQuietHours,
            Self::SetImportantContact { .. } => SecretaryToolKind::SetImportantContact,
            Self::SetNotificationCategoryImportance { .. } => {
                SecretaryToolKind::SetNotificationCategoryImportance
            }
            Self::RecordNotificationFeedback { .. } => {
                SecretaryToolKind::RecordNotificationFeedback
            }
            Self::CreateSimilarNotificationRule { .. } => {
                SecretaryToolKind::CreateSimilarNotificationRule
            }
            Self::DisableNotificationPolicy { .. } => SecretaryToolKind::DisableNotificationPolicy,
            Self::SetAutomaticReplyDeniedForContact { .. } => {
                SecretaryToolKind::SetAutomaticReplyDeniedForContact
            }
            Self::ListMemoryFacts { .. } => SecretaryToolKind::ListMemoryFacts,
            Self::ReadMemoryFactSources { .. } => SecretaryToolKind::ReadMemoryFactSources,
            Self::CorrectMemoryFact { .. } => SecretaryToolKind::CorrectMemoryFact,
            Self::DeleteMemoryFact { .. } => SecretaryToolKind::DeleteMemoryFact,
            Self::SetMemoryFactTtl { .. } => SecretaryToolKind::SetMemoryFactTtl,
            Self::SetConversationMemoryMode { .. } => SecretaryToolKind::SetConversationMemoryMode,
            Self::ConfirmThreadDecision { .. } => SecretaryToolKind::ConfirmThreadDecision,
            Self::RevokeThreadDecision { .. } => SecretaryToolKind::RevokeThreadDecision,
            Self::DismissThreadQuestion { .. } => SecretaryToolKind::DismissThreadQuestion,
            Self::SetThreadLifecycle { .. } => SecretaryToolKind::SetThreadLifecycle,
            Self::DismissFollowUp { .. } => SecretaryToolKind::DismissFollowUp,
            Self::SnoozeFollowUp { .. } => SecretaryToolKind::SnoozeFollowUp,
            Self::DismissFollowUps { .. } => SecretaryToolKind::DismissFollowUps,
            Self::SnoozeFollowUps { .. } => SecretaryToolKind::SnoozeFollowUps,
            Self::CompleteFollowUp { .. } => SecretaryToolKind::CompleteFollowUp,
            Self::CompleteFollowUps { .. } => SecretaryToolKind::CompleteFollowUps,
            Self::DismissResponseExpectation { .. } => {
                SecretaryToolKind::DismissResponseExpectation
            }
            Self::DismissResponseExpectations { .. } => {
                SecretaryToolKind::DismissResponseExpectations
            }
            Self::ListMemoryCandidates { .. } => SecretaryToolKind::ListMemoryCandidates,
            Self::ApproveMemoryCandidate { .. } => SecretaryToolKind::ApproveMemoryCandidate,
            Self::RejectMemoryCandidate { .. } => SecretaryToolKind::RejectMemoryCandidate,
            Self::ListThreadLinkCandidates { .. } => SecretaryToolKind::ListThreadLinkCandidates,
            Self::ListProjects { .. } => SecretaryToolKind::ListProjects,
            Self::QueryProject { .. } => SecretaryToolKind::QueryProject,
            Self::ListCommitments { .. } => SecretaryToolKind::ListCommitments,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretaryActionProposal {
    pub proposal_id: String,
    pub action: SecretaryAction,
    pub rationale: String,
    pub source_event_ids: Vec<SourceEventId>,
    pub idempotency_key: Option<String>,
}

impl SecretaryActionProposal {
    pub fn new(
        action: SecretaryAction,
        rationale: impl Into<String>,
        source_event_ids: Vec<SourceEventId>,
        idempotency_key: Option<String>,
    ) -> Result<Self, SecretaryAgentRuntimeError> {
        let proposal = Self {
            proposal_id: Uuid::new_v4().to_string(),
            action,
            rationale: rationale.into(),
            source_event_ids,
            idempotency_key,
        };
        validate_action_proposal(&proposal)?;
        Ok(proposal)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretaryActionEffect {
    pub proposal: SecretaryActionProposal,
}

impl agent_core::graph::AgentEffect for SecretaryActionEffect {
    type Update = SecretaryAgentUpdate;
    type Receipt = SecretaryActionReceipt;

    fn receipt_updates(receipt: &Self::Receipt) -> Vec<agent_core::AgentUpdate<Self::Update>> {
        vec![agent_core::AgentUpdate::Business(
            SecretaryAgentUpdate::ActionCompleted(receipt.clone()),
        )]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretaryActionReceipt {
    pub proposal_id: String,
    pub result_ref: String,
    /// 产生此回执的 Action 类型；通知策略 Action 通过此字段区分响应工件解析方式。
    #[serde(default)]
    pub tool_kind: Option<SecretaryToolKind>,
}
