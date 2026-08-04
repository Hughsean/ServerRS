//! 跨阶段有界工作上下文（CMD-009 目标 A）。
//!
//! 与 `planning_observations` 的职责边界：
//! - `planning_observations` 保存 Replan 查询的原始观察（proposal_id/typed_events），
//!   供下一轮 Planner 直接读取；它是查询回执的镜像。
//! - 工作上下文保存跨 Plan / Retrieve / Replan / Suspend / Resume 的结构化引用与
//!   未决状态：本轮已选择的证据引用、已解析的会话/Thread/参与者/记忆事实引用、
//!   尚未解决的指代或冲突、最近一次检索触发类型。它是状态机自身的导航状态，
//!   而不是某一轮查询的产物。
//!
//! 有界性：每类列表有硬上限（常量见下），整个上下文序列化字节数有上限
//! （`MAX_WORKING_BYTES`）。只保存结构化引用（真实稳定 ID 仅内部使用），
//! 不保存完整消息正文；每轮重新读取正文与内容策略，撤回 / envelope_only /
//! never_long_term 之后旧正文不可继续使用。LLM 投影由适配层替换为请求内临时引用
//! （fact_ref / evt_N / actor_N / conv_N / thread_N），真实稳定 ID 不进入模型输入。
//!
//! 去重顺序确定：所有列表按首次出现顺序保序去重（Vec + contains），
//! 不使用无序集合直接决定序列化结果。旧 Checkpoint 缺少本字段时通过
//! `#[serde(default)]` 安全恢复为 None（空上下文）。

use serde::{Deserialize, Serialize};

use crate::{
    ConversationRef, EventThreadId, MemoryCandidateId, MemoryFactId, MemoryPayload, ParticipantRef,
    SourceEventId,
};

// ===== 有界常量 =====

/// 当前工作上下文版本。Checkpoint 中旧 JSON 缺失字段时由 serde(default) 恢复为 1。
pub const WORKING_CONTEXT_VERSION: u8 = 1;
/// 本轮已选择证据事件引用的最大数量。
pub const MAX_WORKING_EVIDENCE_REFS: usize = 20;
/// 已解析会话引用的最大数量。
pub const MAX_WORKING_RESOLVED_CONVERSATIONS: usize = 8;
/// 已解析 Thread 引用的最大数量。
pub const MAX_WORKING_RESOLVED_THREADS: usize = 8;
/// 已解析参与者引用的最大数量。
pub const MAX_WORKING_RESOLVED_PARTICIPANTS: usize = 20;
/// 已解析记忆事实引用的最大数量。
pub const MAX_WORKING_RESOLVED_FACTS: usize = 8;
/// 未解决指代条目的最大数量。
pub const MAX_WORKING_OPEN_REFERENCES: usize = 10;
/// 单条未解决指代的来源事件引用上限。
pub const MAX_OPEN_REFERENCE_SOURCES: usize = 5;
/// 工作上下文中自由文本（label/reason/summary/fact_summary）的单条字符上限。
pub const MAX_WORKING_TEXT_CHARS: usize = 1_000;
/// 工作上下文整体序列化字节上限（含冲突回读摘要）。
pub const MAX_WORKING_BYTES: usize = 32 * 1024;

fn default_working_context_version() -> u8 {
    WORKING_CONTEXT_VERSION
}

/// 最近一次检索的触发类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalTriggerKind {
    /// 初始 OwnerCommand 的账号范围检索。
    InitialOwnerCommand,
    /// Replan 轮次的查询工具观察。
    ReplanObservation,
    /// 记忆候选冲突驱动的现行事实回读。
    MemoryConflictReRead,
}

impl RetrievalTriggerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InitialOwnerCommand => "initial_owner_command",
            Self::ReplanObservation => "replan_observation",
            Self::MemoryConflictReRead => "memory_conflict_re_read",
        }
    }
}

/// 未解决指代的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenReferenceKind {
    /// 指代解析出现多个候选，尚未唯一确定。
    AmbiguousReference,
}

/// 一条尚未解决的指代。只保存有界描述与来源引用，不保存正文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenReference {
    pub kind: OpenReferenceKind,
    /// 有界中文描述（不含平台稳定标识；LLM 投影时由适配层脱敏）。
    pub label: String,
    /// 支撑来源事件引用（有界，`MAX_OPEN_REFERENCE_SOURCES`）。
    pub source_event_ids: Vec<SourceEventId>,
    /// 有界原因说明。
    pub reason: String,
}

/// 记忆候选冲突原因码（确定性、有界；不是基础设施异常）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryConflictReasonCode {
    /// 现行 active fact 与候选 payload 内容不同（确定性业务冲突）。
    ActiveFactPayloadDiffers,
    /// 回读时现行事实或来源已失效/被撤回/不再允许长期记忆（fail-closed）。
    ReReadSourcesInvalidated,
    /// 回读时现行事实不属于本账号（fail-closed）。
    ReReadAccountMismatch,
    /// 回读失败（存储不可用等）。
    ReReadFailed,
}

impl MemoryConflictReasonCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ActiveFactPayloadDiffers => "active_fact_payload_differs",
            Self::ReReadSourcesInvalidated => "re_read_sources_invalidated",
            Self::ReReadAccountMismatch => "re_read_account_mismatch",
            Self::ReReadFailed => "re_read_failed",
        }
    }
}

/// 记忆候选冲突上下文（进入工作上下文与 Checkpoint）。
///
/// 冲突是确定性业务结果：携带现行 active fact 的内部引用、冲突 candidate 引用、
/// 现行事实的有效来源引用与有界原因码。回读通过现有 `MemoryUseCase::evidence`
/// 完成，并重新检查账号、事实状态、撤回与内容策略；任一关键来源失效时
/// `re_read_valid = false`（fail-closed，LLM 只能请求澄清，不能把旧事实当有效）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCandidateConflictContext {
    /// 冲突的候选引用（内部 ID，不出现在 LLM 输入）。
    pub candidate_id: MemoryCandidateId,
    /// 现行 active fact 的内部引用（LLM 输入用临时 fact_ref）。
    pub fact_id: MemoryFactId,
    /// 事实种类（person/project/commitment，有界）。
    pub fact_kind: String,
    pub reason_code: MemoryConflictReasonCode,
    /// 有界中文冲突说明（预算耗尽 / 兜底响应用）。
    pub summary: String,
    /// 回读后仍有效的现行事实来源引用（有界）。
    pub source_event_ids: Vec<SourceEventId>,
    /// 现行事实内容的有界中文摘要（回读成功且来源有效时 Some）。
    pub fact_summary: Option<String>,
    /// 回读是否成功且来源全部有效；false 时不得把旧事实呈现为有效。
    pub re_read_valid: bool,
}

impl MemoryCandidateConflictContext {
    /// 构造一个回读有效（fail-open 数据完整）的冲突上下文。
    pub fn valid(
        candidate_id: MemoryCandidateId,
        fact_id: MemoryFactId,
        fact_kind: impl Into<String>,
        reason_code: MemoryConflictReasonCode,
        summary: impl Into<String>,
        source_event_ids: Vec<SourceEventId>,
        fact_summary: impl Into<String>,
    ) -> Result<Self, WorkingContextError> {
        let context = Self {
            candidate_id,
            fact_id,
            fact_kind: fact_kind.into(),
            reason_code,
            summary: summary.into(),
            source_event_ids,
            fact_summary: Some(fact_summary.into()),
            re_read_valid: true,
        };
        validate_conflict_context(&context)?;
        Ok(context)
    }

    /// 构造一个回读失败 / 来源失效的冲突上下文（fail-closed）。
    pub fn invalid(
        candidate_id: MemoryCandidateId,
        fact_id: MemoryFactId,
        reason_code: MemoryConflictReasonCode,
        summary: impl Into<String>,
    ) -> Result<Self, WorkingContextError> {
        let context = Self {
            candidate_id,
            fact_id,
            fact_kind: String::new(),
            reason_code,
            summary: summary.into(),
            source_event_ids: Vec::new(),
            fact_summary: None,
            re_read_valid: false,
        };
        validate_conflict_context(&context)?;
        Ok(context)
    }
}

/// 版本化、有界的工作上下文 v1。
///
/// 所有列表按首次出现顺序保序去重；超出硬上限 fail-closed（返回错误），
/// 不做静默截断。通过 Checkpoint JSON 持久化，跨 Plan→Retrieve→Replan 与
/// Suspend→进程重建→Resume 保持。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentWorkingContextV1 {
    #[serde(default = "default_working_context_version")]
    pub version: u8,
    /// 本轮已选择的证据事件引用。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_evidence_refs: Vec<SourceEventId>,
    /// 已解析的会话引用。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_conversation_refs: Vec<ConversationRef>,
    /// 已解析的 Thread 引用。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_thread_refs: Vec<EventThreadId>,
    /// 已解析的参与者引用（身份种类 + 稳定主体 ID，账号由状态自身隐含）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_participant_refs: Vec<ParticipantRef>,
    /// 已解析的记忆事实引用。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_fact_refs: Vec<MemoryFactId>,
    /// 尚未解决的指代条目。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_references: Vec<OpenReference>,
    /// 最近一次检索的触发类型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_retrieval: Option<RetrievalTriggerKind>,
    /// 记忆候选冲突上下文（未解决冲突时 Some）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<MemoryCandidateConflictContext>,
}

impl Default for AgentWorkingContextV1 {
    fn default() -> Self {
        Self {
            version: WORKING_CONTEXT_VERSION,
            selected_evidence_refs: Vec::new(),
            resolved_conversation_refs: Vec::new(),
            resolved_thread_refs: Vec::new(),
            resolved_participant_refs: Vec::new(),
            resolved_fact_refs: Vec::new(),
            open_references: Vec::new(),
            last_retrieval: None,
            conflict: None,
        }
    }
}

impl AgentWorkingContextV1 {
    pub fn new() -> Self {
        Self::default()
    }

    /// 合并初始检索结果（PlanNode 首轮）：登记证据引用与命令会话引用，更新触发类型。
    pub fn merge_initial_retrieval(
        &mut self,
        evidence_refs: Vec<SourceEventId>,
        resolved_conversation_refs: Vec<ConversationRef>,
        trigger: RetrievalTriggerKind,
    ) -> Result<(), WorkingContextError> {
        append_unique(&mut self.selected_evidence_refs, evidence_refs)?;
        append_unique(
            &mut self.resolved_conversation_refs,
            resolved_conversation_refs,
        )?;
        self.last_retrieval = Some(trigger);
        self.validate()
    }

    /// 合并 Replan 观察（ReplanDecisionNode）：登记新证据、解析引用与未解决指代。
    pub fn merge_replan_evidence(
        &mut self,
        evidence_refs: Vec<SourceEventId>,
        resolved_thread_refs: Vec<EventThreadId>,
        resolved_participant_refs: Vec<ParticipantRef>,
        resolved_fact_refs: Vec<MemoryFactId>,
        open_references: Vec<OpenReference>,
    ) -> Result<(), WorkingContextError> {
        append_unique(&mut self.selected_evidence_refs, evidence_refs)?;
        append_unique(&mut self.resolved_thread_refs, resolved_thread_refs)?;
        append_unique(
            &mut self.resolved_participant_refs,
            resolved_participant_refs,
        )?;
        append_unique(&mut self.resolved_fact_refs, resolved_fact_refs)?;
        for open in open_references {
            // 同 kind + label 不重复追加（保序去重）。
            if !self
                .open_references
                .iter()
                .any(|existing| existing.kind == open.kind && existing.label == open.label)
            {
                self.open_references.push(open);
            }
        }
        self.last_retrieval = Some(RetrievalTriggerKind::ReplanObservation);
        self.validate()
    }

    /// 合并冲突回读结果：同一候选冲突只登记一次（幂等），并登记回读来源与事实引用。
    pub fn merge_conflict(
        &mut self,
        conflict: MemoryCandidateConflictContext,
    ) -> Result<(), WorkingContextError> {
        // 同候选冲突已登记（Checkpoint 恢复 / 幂等重放）：不覆盖回读结果。
        if let Some(existing) = &self.conflict
            && existing.candidate_id == conflict.candidate_id
        {
            return Ok(());
        }
        append_unique(&mut self.resolved_fact_refs, vec![conflict.fact_id.clone()])?;
        append_unique(
            &mut self.selected_evidence_refs,
            conflict.source_event_ids.clone(),
        )?;
        self.conflict = Some(conflict);
        self.last_retrieval = Some(RetrievalTriggerKind::MemoryConflictReRead);
        self.validate()
    }

    /// 生成 Planner 接收的有界投影（内部真实 ID；LLM 适配层映射为临时引用）。
    pub fn projection(&self) -> WorkingContextProjection {
        WorkingContextProjection {
            evidence_refs: self.selected_evidence_refs.clone(),
            resolved_conversation_refs: self.resolved_conversation_refs.clone(),
            resolved_thread_refs: self.resolved_thread_refs.clone(),
            resolved_participant_refs: self.resolved_participant_refs.clone(),
            resolved_fact_refs: self.resolved_fact_refs.clone(),
            open_references: self.open_references.clone(),
            last_retrieval: self.last_retrieval,
            conflict: self.conflict.clone(),
        }
    }

    /// 校验所有硬上限（数量、字符、去重、版本与整体字节数）。
    pub fn validate(&self) -> Result<(), WorkingContextError> {
        if self.version != WORKING_CONTEXT_VERSION {
            return Err(WorkingContextError::UnsupportedVersion {
                version: self.version,
            });
        }
        check_len(
            "selected_evidence_refs",
            &self.selected_evidence_refs,
            MAX_WORKING_EVIDENCE_REFS,
        )?;
        check_len(
            "resolved_conversation_refs",
            &self.resolved_conversation_refs,
            MAX_WORKING_RESOLVED_CONVERSATIONS,
        )?;
        check_len(
            "resolved_thread_refs",
            &self.resolved_thread_refs,
            MAX_WORKING_RESOLVED_THREADS,
        )?;
        check_len(
            "resolved_participant_refs",
            &self.resolved_participant_refs,
            MAX_WORKING_RESOLVED_PARTICIPANTS,
        )?;
        check_len(
            "resolved_fact_refs",
            &self.resolved_fact_refs,
            MAX_WORKING_RESOLVED_FACTS,
        )?;
        check_len(
            "open_references",
            &self.open_references,
            MAX_WORKING_OPEN_REFERENCES,
        )?;
        ensure_unique("selected_evidence_refs", &self.selected_evidence_refs)?;
        ensure_unique(
            "resolved_conversation_refs",
            &self.resolved_conversation_refs,
        )?;
        ensure_unique("resolved_thread_refs", &self.resolved_thread_refs)?;
        ensure_unique("resolved_participant_refs", &self.resolved_participant_refs)?;
        ensure_unique("resolved_fact_refs", &self.resolved_fact_refs)?;
        for open in &self.open_references {
            validate_open_reference(open)?;
        }
        if let Some(conflict) = &self.conflict {
            validate_conflict_context(conflict)?;
        }
        // 整体序列化字节上限：跨 Checkpoint 持久化时仍然有界。
        let size = serde_json::to_string(self)
            .map(|json| json.len())
            .unwrap_or(usize::MAX);
        if size > MAX_WORKING_BYTES {
            return Err(WorkingContextError::SerializedTooLarge {
                size,
                max: MAX_WORKING_BYTES,
            });
        }
        Ok(())
    }
}

/// Planner 接收的有界工作上下文投影。只含引用与有界文本，不含正文；
/// LLM 适配层把真实稳定 ID 替换为请求内临时引用后才进入模型输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingContextProjection {
    pub evidence_refs: Vec<SourceEventId>,
    pub resolved_conversation_refs: Vec<ConversationRef>,
    pub resolved_thread_refs: Vec<EventThreadId>,
    pub resolved_participant_refs: Vec<ParticipantRef>,
    pub resolved_fact_refs: Vec<MemoryFactId>,
    pub open_references: Vec<OpenReference>,
    pub last_retrieval: Option<RetrievalTriggerKind>,
    pub conflict: Option<MemoryCandidateConflictContext>,
}

/// 校验工作上下文投影的有界约束（供 validate_planner_input 复用）。
pub fn validate_working_context_projection(
    projection: &WorkingContextProjection,
) -> Result<(), WorkingContextError> {
    check_len(
        "evidence_refs",
        &projection.evidence_refs,
        MAX_WORKING_EVIDENCE_REFS,
    )?;
    check_len(
        "resolved_conversation_refs",
        &projection.resolved_conversation_refs,
        MAX_WORKING_RESOLVED_CONVERSATIONS,
    )?;
    check_len(
        "resolved_thread_refs",
        &projection.resolved_thread_refs,
        MAX_WORKING_RESOLVED_THREADS,
    )?;
    check_len(
        "resolved_participant_refs",
        &projection.resolved_participant_refs,
        MAX_WORKING_RESOLVED_PARTICIPANTS,
    )?;
    check_len(
        "resolved_fact_refs",
        &projection.resolved_fact_refs,
        MAX_WORKING_RESOLVED_FACTS,
    )?;
    check_len(
        "open_references",
        &projection.open_references,
        MAX_WORKING_OPEN_REFERENCES,
    )?;
    ensure_unique("evidence_refs", &projection.evidence_refs)?;
    ensure_unique(
        "resolved_conversation_refs",
        &projection.resolved_conversation_refs,
    )?;
    ensure_unique("resolved_thread_refs", &projection.resolved_thread_refs)?;
    ensure_unique(
        "resolved_participant_refs",
        &projection.resolved_participant_refs,
    )?;
    ensure_unique("resolved_fact_refs", &projection.resolved_fact_refs)?;
    for open in &projection.open_references {
        validate_open_reference(open)?;
    }
    if let Some(conflict) = &projection.conflict {
        validate_conflict_context(conflict)?;
    }
    Ok(())
}

/// 把记忆 payload 转为有界中文摘要（不含任何稳定 ID；供冲突回读与展示使用）。
pub fn summarize_memory_payload(payload: &MemoryPayload, max_chars: usize) -> String {
    let summary = match payload {
        MemoryPayload::Person(person) => match &person.relationship {
            Some(relationship) => format!("人物记忆（关系：{relationship}）"),
            None => "人物记忆".to_owned(),
        },
        MemoryPayload::Project(project) => format!("项目记忆（目标：{}）", project.goal),
        MemoryPayload::Commitment(commitment) => {
            format!("承诺记忆（行动：{}）", commitment.action)
        }
    };
    summary.chars().take(max_chars).collect()
}

// ===== 合并与校验辅助 =====

/// 保序去重追加；超出上限 fail-closed。
fn append_unique<T: PartialEq>(
    list: &mut Vec<T>,
    items: Vec<T>,
) -> Result<(), WorkingContextError> {
    for item in items {
        if !list.contains(&item) {
            list.push(item);
        }
    }
    Ok(())
}

fn check_len<T>(field: &'static str, items: &[T], max: usize) -> Result<(), WorkingContextError> {
    if items.len() > max {
        return Err(WorkingContextError::TooLarge { field, max });
    }
    Ok(())
}

fn ensure_unique<T: PartialEq>(
    field: &'static str,
    items: &[T],
) -> Result<(), WorkingContextError> {
    for (index, item) in items.iter().enumerate() {
        if items[..index].contains(item) {
            return Err(WorkingContextError::Duplicate { field });
        }
    }
    Ok(())
}

fn bounded_text(field: &'static str, value: &str, max: usize) -> Result<(), WorkingContextError> {
    if value.trim().is_empty() {
        return Err(WorkingContextError::Blank { field });
    }
    if value.chars().count() > max {
        return Err(WorkingContextError::TooLarge { field, max });
    }
    Ok(())
}

fn validate_open_reference(open: &OpenReference) -> Result<(), WorkingContextError> {
    bounded_text("open_reference.label", &open.label, MAX_WORKING_TEXT_CHARS)?;
    bounded_text(
        "open_reference.reason",
        &open.reason,
        MAX_WORKING_TEXT_CHARS,
    )?;
    check_len(
        "open_reference.source_event_ids",
        &open.source_event_ids,
        MAX_OPEN_REFERENCE_SOURCES,
    )?;
    ensure_unique("open_reference.source_event_ids", &open.source_event_ids)
}

fn validate_conflict_context(
    conflict: &MemoryCandidateConflictContext,
) -> Result<(), WorkingContextError> {
    bounded_text("conflict.fact_kind", &conflict.fact_kind, 16)?;
    bounded_text(
        "conflict.summary",
        &conflict.summary,
        MAX_WORKING_TEXT_CHARS,
    )?;
    check_len(
        "conflict.source_event_ids",
        &conflict.source_event_ids,
        MAX_WORKING_EVIDENCE_REFS,
    )?;
    ensure_unique("conflict.source_event_ids", &conflict.source_event_ids)?;
    if let Some(fact_summary) = &conflict.fact_summary {
        bounded_text(
            "conflict.fact_summary",
            fact_summary,
            MAX_WORKING_TEXT_CHARS,
        )?;
    }
    if conflict.re_read_valid && conflict.fact_summary.is_none() {
        return Err(WorkingContextError::Blank {
            field: "conflict.fact_summary",
        });
    }
    if conflict.re_read_valid && conflict.source_event_ids.is_empty() {
        return Err(WorkingContextError::Blank {
            field: "conflict.source_event_ids",
        });
    }
    Ok(())
}

/// 工作上下文错误（fail-closed，不做静默截断）。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WorkingContextError {
    #[error("working context {field} exceeds max {max}")]
    TooLarge { field: &'static str, max: usize },
    #[error("working context {field} must be unique")]
    Duplicate { field: &'static str },
    #[error("working context {field} must not be blank")]
    Blank { field: &'static str },
    #[error("working context serialized size {size} exceeds max {max}")]
    SerializedTooLarge { size: usize, max: usize },
    #[error("working context version {version} is not supported")]
    UnsupportedVersion { version: u8 },
}

/// 类型化工作上下文更新。状态更新只能通过 `SecretaryAgentUpdate` 进入状态机，
/// 节点不得直接修改状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkingContextUpdate {
    /// 初始检索（PlanNode 首轮）后登记证据引用、命令会话引用与触发类型。
    InitialRetrieval {
        evidence_refs: Vec<SourceEventId>,
        resolved_conversation_refs: Vec<ConversationRef>,
        trigger: RetrievalTriggerKind,
    },
    /// Replan 观察（ReplanDecisionNode 解析查询回执）后登记新证据/解析引用/未解决指代。
    ReplanEvidence {
        evidence_refs: Vec<SourceEventId>,
        resolved_thread_refs: Vec<EventThreadId>,
        resolved_participant_refs: Vec<ParticipantRef>,
        resolved_fact_refs: Vec<MemoryFactId>,
        open_references: Vec<OpenReference>,
    },
    /// 记忆候选冲突回读结果（ReplanDecisionNode 在冲突回执上触发一次回读）。
    ConflictReRead(MemoryCandidateConflictContext),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConversationKind, SecretaryAgentState, SecretaryAgentUpdate};
    use agent_core::AgentBusinessState;

    fn event_id(id: &str) -> SourceEventId {
        SourceEventId::new(id).unwrap()
    }

    fn conversation() -> ConversationRef {
        ConversationRef::new(ConversationKind::Group, "conv-1").unwrap()
    }

    fn conflict_context() -> MemoryCandidateConflictContext {
        MemoryCandidateConflictContext::valid(
            MemoryCandidateId::generate(),
            MemoryFactId::generate(),
            "project",
            MemoryConflictReasonCode::ActiveFactPayloadDiffers,
            "记忆候选与既有记忆内容冲突，未做任何修改",
            vec![event_id("e1")],
            "项目记忆（目标：8 月上线）",
        )
        .unwrap()
    }

    /// 超限 fail-closed：证据引用超过硬上限后合并必须报错，不做静默截断。
    #[test]
    fn evidence_refs_over_limit_fails_closed() {
        let mut context = AgentWorkingContextV1::new();
        let refs: Vec<SourceEventId> = (0..MAX_WORKING_EVIDENCE_REFS as i64)
            .map(|i| event_id(&format!("evt-{i}")))
            .collect();
        context
            .merge_initial_retrieval(
                refs,
                vec![conversation()],
                RetrievalTriggerKind::InitialOwnerCommand,
            )
            .unwrap();
        let error = context
            .merge_replan_evidence(
                vec![event_id("overflow")],
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            WorkingContextError::TooLarge {
                field: "selected_evidence_refs",
                ..
            }
        ));
    }

    /// 保序去重：重复证据与重复会话只保留首次出现顺序。
    #[test]
    fn merge_dedups_preserving_first_order() {
        let mut context = AgentWorkingContextV1::new();
        context
            .merge_initial_retrieval(
                vec![event_id("e2"), event_id("e1"), event_id("e2")],
                vec![conversation()],
                RetrievalTriggerKind::InitialOwnerCommand,
            )
            .unwrap();
        context
            .merge_replan_evidence(
                vec![event_id("e3"), event_id("e1")],
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
            .unwrap();
        let ids: Vec<&str> = context
            .selected_evidence_refs
            .iter()
            .map(SourceEventId::as_str)
            .collect();
        assert_eq!(ids, vec!["e2", "e1", "e3"]);
        // 最近一次检索触发被 Replan 观察覆盖。
        assert_eq!(
            context.last_retrieval,
            Some(RetrievalTriggerKind::ReplanObservation)
        );
    }

    /// Checkpoint 序列化兼容：旧 JSON 缺少 working_context 字段时安全恢复为 None。
    #[test]
    fn legacy_state_json_without_working_context_deserializes() {
        let state =
            SecretaryAgentState::new("目标", Vec::new(), vec![event_id("e1")], Vec::new()).unwrap();
        let json = serde_json::to_string(&state).unwrap();
        // 旧 Checkpoint 没有 working_context 字段 —— 手动剥掉后再反序列化。
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let mut object = value.as_object().unwrap().clone();
        object.remove("working_context");
        let legacy = serde_json::to_string(&serde_json::Value::Object(object)).unwrap();
        let restored: SecretaryAgentState = serde_json::from_str(&legacy).unwrap();
        assert!(restored.working_context().is_none());
    }

    /// Replan 后保留引用：状态经 Checkpoint JSON 往返，证据/解析引用与冲突上下文不丢失。
    #[test]
    fn refs_survive_checkpoint_round_trip_and_replan() {
        let mut state =
            SecretaryAgentState::new("目标", Vec::new(), vec![event_id("e1")], Vec::new()).unwrap();
        state
            .apply_update(SecretaryAgentUpdate::WorkingContext(
                WorkingContextUpdate::InitialRetrieval {
                    evidence_refs: vec![event_id("e1"), event_id("e2")],
                    resolved_conversation_refs: vec![conversation()],
                    trigger: RetrievalTriggerKind::InitialOwnerCommand,
                },
            ))
            .unwrap();
        let conflict = conflict_context();
        let conflict_fact_id = conflict.fact_id.clone();
        state
            .apply_update(SecretaryAgentUpdate::WorkingContext(
                WorkingContextUpdate::ConflictReRead(conflict.clone()),
            ))
            .unwrap();
        let json = serde_json::to_string(&state).unwrap();
        let restored: SecretaryAgentState = serde_json::from_str(&json).unwrap();
        let context = restored.working_context().unwrap();
        assert_eq!(
            context.selected_evidence_refs.len(),
            2,
            "证据引用必须在 Checkpoint 往返后保留"
        );
        assert_eq!(context.resolved_conversation_refs, vec![conversation()]);
        assert_eq!(
            context.conflict.as_ref().map(|c| c.fact_id.clone()),
            Some(conflict_fact_id),
            "冲突上下文必须在 Checkpoint 往返后保留"
        );
        // 冲突事实进入已解析事实引用。
        assert_eq!(context.resolved_fact_refs.len(), 1);
        // 同一候选冲突幂等：再次合并不覆盖、不重复。
        state
            .apply_update(SecretaryAgentUpdate::WorkingContext(
                WorkingContextUpdate::ConflictReRead(conflict.clone()),
            ))
            .unwrap();
        let after = state.working_context().unwrap();
        assert_eq!(after.resolved_fact_refs.len(), 1);
        assert_eq!(after.open_references.len(), 0);
    }

    /// 未解决指代条目可进入工作上下文并保序去重。
    #[test]
    fn open_references_merge_dedup() {
        let mut context = AgentWorkingContextV1::new();
        let open = OpenReference {
            kind: OpenReferenceKind::AmbiguousReference,
            label: "指代「小张」存在多个候选".into(),
            source_event_ids: vec![event_id("e1")],
            reason: "同名候选无法唯一解析".into(),
        };
        context
            .merge_replan_evidence(
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                vec![open.clone(), open.clone()],
            )
            .unwrap();
        assert_eq!(context.open_references.len(), 1);
    }

    /// 整体字节上限 fail-closed：序列化字节数超过 MAX_WORKING_BYTES 时校验必须失败。
    #[test]
    fn serialized_byte_cap_fails_closed() {
        // 直接构造一个条数合法但自由文本撑满上限的上下文（绕过单条校验，直击字节上限）。
        // 每条 label+reason 各约 1000 字符（中文约 3 字节/字符），10 条远超 32KiB。
        let huge_label = "字".repeat(MAX_WORKING_TEXT_CHARS - 4);
        let mut context = AgentWorkingContextV1::new();
        for i in 0..MAX_WORKING_OPEN_REFERENCES {
            context.open_references.push(OpenReference {
                kind: OpenReferenceKind::AmbiguousReference,
                label: format!("{huge_label}-{i}"),
                source_event_ids: vec![event_id(&format!("evt-{i}"))],
                reason: huge_label.clone(),
            });
        }
        let error = context.validate().unwrap_err();
        assert!(
            matches!(error, WorkingContextError::SerializedTooLarge { .. }),
            "10 条 ×2000 字符的指代条目序列化必须超过字节上限: {error:?}"
        );
    }
}
