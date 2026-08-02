//! 结构化记忆候选的协议无关领域模型。
//!
//! 候选是"待 Owner 确认"的记忆草案：提取器从有界 SourceEvent 上下文生成
//! Person/Project/Commitment 三类候选，Owner 批准后才落为 `MemoryFactStatus::Confirmed`
//! 的正式记忆。本模块只定义类型、校验与状态转换，不依赖 NapCat、SeaORM 或 LLM。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    CommitmentMemory, CommitmentStatus, ContentTrustLevel, ConversationRef, MemoryFact,
    MemoryFactError, MemoryFactId, MemoryFactStatus, MemoryPayload, MessageRole, SourceAccountRef,
    SourceEventId, ThreadActorRef, validate_memory_fact, validate_memory_payload,
};

/// 单个候选引用的来源事件上限（同时约束 source_event_ids 数组）。
pub const MAX_CANDIDATE_SOURCES: usize = 20;
/// 单个候选序列化后的最大字节数（16 KiB）。
pub const MAX_CANDIDATE_PAYLOAD_BYTES: usize = 16 * 1024;
/// 候选版本起点：从 1 开始，每次状态变化精确 +1。
pub const INITIAL_CANDIDATE_VERSION: u64 = 1;
/// 批准候选时写入 MemoryFact 的置信度（候选不携带置信度，v1 以满置信度确认；
/// 置信度建模留待后续切片，不在此伪造来源证据）。
pub const APPROVED_CANDIDATE_CONFIDENCE_BPS: u16 = 10_000;

/// 持久化候选的有界非空标识（`secretary_memory_candidates.candidate_id`，CHAR(36)）。
/// 在 Action 边界使用类型而不是裸字符串，避免与 proposal_id/run_id/effect_id/fact_id 混用。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryCandidateId(String);

impl MemoryCandidateId {
    pub fn new(value: impl Into<String>) -> Result<Self, MemoryCandidateError> {
        let value = value.into();
        if value.trim().is_empty() || value.len() > 36 {
            return Err(MemoryCandidateError::InvalidData(
                "candidate_id must contain 1..=36 bytes".into(),
            ));
        }
        Ok(Self(value))
    }

    /// 候选 ID 使用稳定 UUID（提取器生成，重放不产生新 ID）。
    pub fn generate() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 候选类型；与 `MemoryPayload::kind()` 输出保持一致（person/project/commitment）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCandidateKind {
    Person,
    Project,
    Commitment,
}

impl MemoryCandidateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Project => "project",
            Self::Commitment => "commitment",
        }
    }

    pub fn from_payload(payload: &MemoryPayload) -> Self {
        match payload {
            MemoryPayload::Person(_) => Self::Person,
            MemoryPayload::Project(_) => Self::Project,
            MemoryPayload::Commitment(_) => Self::Commitment,
        }
    }
}

/// 候选状态机：proposed -> approved / rejected / invalidated。
/// 已 approved/rejected/invalidated 的候选不再参与审批；invalidated 只由
/// Worker 依据来源失效（撤回、会话切换为 never_long_term 等）写入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCandidateStatus {
    Proposed,
    Approved,
    Rejected,
    Invalidated,
}

impl MemoryCandidateStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Invalidated => "invalidated",
        }
    }
}

/// 候选版本（1 起，每次状态变化精确 +1；MySQL 列为 BIGINT UNSIGNED）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryCandidateVersion(u64);

impl MemoryCandidateVersion {
    pub fn new(value: u64) -> Result<Self, MemoryCandidateError> {
        if value == 0 {
            return Err(MemoryCandidateError::InvalidData(
                "candidate version must be >= 1".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// 单个候选的精确来源（来源事件 + 发送者 + 时间 + 当时的内容信任级别）。
/// 来源必须属于当前提取批次；信任级别用于批准时的长期记忆复验。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCandidateSource {
    pub source_event_id: SourceEventId,
    pub actor: ThreadActorRef,
    pub occurred_at_unix_secs: i64,
    pub content_trust_level: ContentTrustLevel,
}

/// 一个待确认的结构化记忆候选。payload 必须通过 `MemoryPayload` 领域校验；
/// 序列化后不得超过 16 KiB；fingerprint 由 store/use case 重算校验，防止提取器伪造。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCandidate {
    pub candidate_id: MemoryCandidateId,
    pub account: SourceAccountRef,
    pub subject_key: String,
    pub payload: MemoryPayload,
    pub status: MemoryCandidateStatus,
    pub version: MemoryCandidateVersion,
    pub extractor_version: String,
    /// account + kind + canonical subject + canonical payload + 排序去重后的来源 ID
    /// + extractor_version 的稳定派生（UUIDv5）；同账号同 fingerprint 只能存在一个候选。
    pub deterministic_fingerprint: String,
    pub sources: Vec<MemoryCandidateSource>,
}

/// 提取批次中的单条事件。正文超过单条或整批字符预算时 `content_omitted=true`，
/// 提取器必须跳过该事件，不能基于截断文本推断事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCandidateEvent {
    pub source_event_id: SourceEventId,
    pub actor: ThreadActorRef,
    pub role: MessageRole,
    pub occurred_at_unix_secs: i64,
    /// 事件当时的内容信任级别；提取器原样落到候选来源上，批准时据此复验。
    pub content_trust_level: ContentTrustLevel,
    pub normalized_text: String,
    pub content_omitted: bool,
}

/// 提取批次：账号作用域的有界事件集合 + 持久化游标 + 租约。
/// 候选只能引用本批次内的 SourceEvent 与 Actor。
/// 批次按会话分界：同一批次只含一个 conversation 的事件，避免把不同群/私聊的
/// 上下文混进同一候选的承诺、项目或人物判断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCandidateBatch {
    pub account: SourceAccountRef,
    /// 本批次全部事件所属的会话（claim 时按会话分批）。
    pub conversation: ConversationRef,
    pub lease_token: MemoryCandidateLeaseToken,
    pub events: Vec<MemoryCandidateEvent>,
    pub next_cursor: MemoryCandidateCursor,
}

/// 持久化游标（`received_at` + 事件 ID 二元组，与线程语义游标同构）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCandidateCursor {
    pub received_at_unix_micros: i64,
    pub source_event_id: SourceEventId,
}

/// 提取租约令牌。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCandidateLeaseToken(String);

impl MemoryCandidateLeaseToken {
    pub fn generate() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 候选的来源内容信任门槛：normal 总是可提取；local_only 仅当模型端点被
/// 明确验证为 loopback 本地端点；envelope_only/never_long_term 一律禁止
/// 进入记忆提取模型（与 `is_allowed_for_model` 的判定保持一致）。
pub fn is_eligible_for_candidate_extraction(
    trust: ContentTrustLevel,
    allow_local_only: bool,
) -> bool {
    match trust {
        ContentTrustLevel::Normal => true,
        ContentTrustLevel::LocalOnly => allow_local_only,
        ContentTrustLevel::EnvelopeOnly | ContentTrustLevel::NeverLongTerm => false,
    }
}

/// 稳定派生 deterministic_fingerprint。canonical payload 用 serde 的确定性
/// 输出；来源 ID 排序去重后再参与派生，顺序无关。
pub fn candidate_fingerprint(
    account: &SourceAccountRef,
    payload: &MemoryPayload,
    subject_key: &str,
    sources: &[MemoryCandidateSource],
    extractor_version: &str,
) -> String {
    let mut source_ids = sources
        .iter()
        .map(|source| source.source_event_id.as_str())
        .collect::<Vec<_>>();
    source_ids.sort_unstable();
    source_ids.dedup();
    let canonical = serde_json::json!({
        "account": [account.channel.as_str(), account.account_id],
        "kind": payload.kind(),
        "subject": subject_key.trim(),
        "payload": payload,
        "sources": source_ids,
        "extractor_version": extractor_version,
    })
    .to_string();
    Uuid::new_v5(&Uuid::NAMESPACE_OID, canonical.as_bytes()).to_string()
}

/// 对提取器输出执行不可绕过的校验。提取器无论是规则、LLM 还是人工控制面，
/// 都只能生成候选；来源/主体越界、超过大小上限或 fingerprint 不匹配会在此被拒绝。
pub fn validate_memory_candidate(
    candidate: &MemoryCandidate,
    batch: &MemoryCandidateBatch,
) -> Result<(), MemoryCandidateError> {
    if candidate.status != MemoryCandidateStatus::Proposed
        || candidate.version.as_u64() != INITIAL_CANDIDATE_VERSION
    {
        return Err(MemoryCandidateError::InvalidData(
            "candidate must start as proposed with version 1".into(),
        ));
    }
    if candidate.account != batch.account {
        return Err(MemoryCandidateError::InvalidData(
            "candidate account must match the claimed batch account".into(),
        ));
    }
    if candidate.subject_key.trim().is_empty() || candidate.subject_key.chars().count() > 191 {
        return Err(MemoryCandidateError::InvalidData(
            "subject_key must contain 1..=191 characters".into(),
        ));
    }
    if candidate.extractor_version.trim().is_empty() || candidate.extractor_version.len() > 32 {
        return Err(MemoryCandidateError::InvalidData(
            "extractor_version must contain 1..=32 bytes".into(),
        ));
    }
    if candidate.sources.is_empty() || candidate.sources.len() > MAX_CANDIDATE_SOURCES {
        return Err(MemoryCandidateError::InvalidData(
            "candidate must cite 1..=20 source events".into(),
        ));
    }
    let event_ids = batch
        .events
        .iter()
        .map(|event| event.source_event_id.as_str())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    for source in &candidate.sources {
        if source.actor.account != batch.account {
            return Err(MemoryCandidateError::InvalidData(
                "candidate source actor must belong to the managed account".into(),
            ));
        }
        if !event_ids.contains(source.source_event_id.as_str()) {
            return Err(MemoryCandidateError::InvalidData(
                "candidate cites an event outside the claimed batch".into(),
            ));
        }
        if !seen.insert(source.source_event_id.as_str()) {
            return Err(MemoryCandidateError::InvalidData(
                "candidate contains duplicate source events".into(),
            ));
        }
        // 来源 Actor 必须等于被引用 SourceEvent 的权威 Actor：事实身份与证据
        // 必须强一致，提取器不得伪造来源身份（审批时还会在 DB 层复验一次）。
        let batch_event = batch
            .events
            .iter()
            .find(|event| event.source_event_id == source.source_event_id)
            .ok_or_else(|| MemoryCandidateError::InvalidData("source event not found".into()))?;
        if batch_event.actor.actor_id != source.actor.actor_id {
            return Err(MemoryCandidateError::InvalidData(
                "candidate source actor does not match the authoritative source event actor".into(),
            ));
        }
        if source.content_trust_level != batch_event.content_trust_level
            || !is_eligible_for_candidate_extraction(batch_event.content_trust_level, true)
        {
            return Err(MemoryCandidateError::InvalidData(
                "candidate source trust level does not match the batch event".into(),
            ));
        }
    }
    // payload 引用的每个身份都必须有来源证据覆盖（Person 主体、Commitment 的
    // promisor/beneficiary、Project 成员）；没有证据的身份不得出现在事实里。
    let source_actors = candidate
        .sources
        .iter()
        .map(|source| source.actor.actor_id.as_str())
        .collect::<HashSet<_>>();
    let payload_actors = match &candidate.payload {
        MemoryPayload::Person(person) => vec![person.person.actor_id.as_str()],
        MemoryPayload::Project(project) => project
            .member_actor_ids
            .iter()
            .map(String::as_str)
            .collect(),
        MemoryPayload::Commitment(commitment) => vec![
            commitment.promisor.actor_id.as_str(),
            commitment.beneficiary.actor_id.as_str(),
        ],
    };
    if let Some(uncovered) = payload_actors
        .into_iter()
        .find(|actor_id| !source_actors.contains(actor_id))
    {
        return Err(MemoryCandidateError::InvalidData(format!(
            "candidate payload actor {uncovered} has no source evidence"
        )));
    }
    let source_ids = candidate
        .sources
        .iter()
        .map(|source| source.source_event_id.clone())
        .collect::<Vec<_>>();
    validate_memory_payload(&candidate.payload, &candidate.account, &source_ids)
        .map_err(|error| MemoryCandidateError::InvalidData(error.to_string()))?;
    if !payload_actors_observed_in_batch(&candidate.payload, batch)? {
        return Err(MemoryCandidateError::InvalidData(
            "candidate payload references an actor outside the claimed batch".into(),
        ));
    }
    let serialized = serde_json::to_string(&candidate.payload)
        .map_err(|error| MemoryCandidateError::Serialize(error.to_string()))?;
    if serialized.len() > MAX_CANDIDATE_PAYLOAD_BYTES {
        return Err(MemoryCandidateError::InvalidData(
            "candidate payload exceeds the 16 KiB size limit".into(),
        ));
    }
    let expected = candidate_fingerprint(
        &candidate.account,
        &candidate.payload,
        &candidate.subject_key,
        &candidate.sources,
        &candidate.extractor_version,
    );
    if candidate.deterministic_fingerprint != expected {
        return Err(MemoryCandidateError::InvalidData(
            "candidate fingerprint does not match its content".into(),
        ));
    }
    Ok(())
}

/// 候选 payload 中的每个 Actor 引用都必须是批次内可观察的稳定 Actor。
/// Person 主体、Commitment 的 promisor/beneficiary、Project 成员都只能用稳定
/// actor ID 指代，不得引入批次外的身份。
fn payload_actors_observed_in_batch(
    payload: &MemoryPayload,
    batch: &MemoryCandidateBatch,
) -> Result<bool, MemoryCandidateError> {
    let observed = batch
        .events
        .iter()
        .map(|event| event.actor.actor_id.as_str())
        .collect::<HashSet<_>>();
    let required = match payload {
        MemoryPayload::Person(person) => vec![person.person.actor_id.as_str()],
        MemoryPayload::Project(project) => project
            .member_actor_ids
            .iter()
            .map(String::as_str)
            .collect(),
        MemoryPayload::Commitment(commitment) => vec![
            commitment.promisor.actor_id.as_str(),
            commitment.beneficiary.actor_id.as_str(),
        ],
    };
    Ok(required
        .into_iter()
        .all(|actor_id| observed.contains(actor_id)))
}

/// 把 approved 候选转换为 Confirmed MemoryFact：
/// - 候选状态必须是 proposed（由调用方在事务内已校验）；
/// - 写为 `MemoryFactStatus::Confirmed`；
/// - Commitment 候选的状态从 `Proposed` 改为 `Pending`（批准后才可跟进，不得自动声称完成）。
pub fn candidate_to_confirmed_fact(
    candidate: &MemoryCandidate,
    fact_id: MemoryFactId,
) -> Result<MemoryFact, MemoryCandidateError> {
    if candidate.status != MemoryCandidateStatus::Proposed {
        return Err(MemoryCandidateError::InvalidData(
            "only a proposed candidate can be approved".into(),
        ));
    }
    let payload = match &candidate.payload {
        MemoryPayload::Commitment(commitment) => MemoryPayload::Commitment(CommitmentMemory {
            promisor: commitment.promisor.clone(),
            beneficiary: commitment.beneficiary.clone(),
            action: commitment.action.clone(),
            due_at_unix_secs: commitment.due_at_unix_secs,
            status: CommitmentStatus::Pending,
            completion_source_event_id: None,
        }),
        other => other.clone(),
    };
    let fact = MemoryFact {
        fact_id,
        account: candidate.account.clone(),
        subject_key: candidate.subject_key.clone(),
        payload,
        status: MemoryFactStatus::Confirmed,
        confidence_bps: APPROVED_CANDIDATE_CONFIDENCE_BPS,
        source_event_ids: candidate
            .sources
            .iter()
            .map(|source| source.source_event_id.clone())
            .collect(),
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    validate_memory_fact(&fact)
        .map_err(|error| MemoryCandidateError::InvalidData(error.to_string()))?;
    Ok(fact)
}

/// 用于 List 展示的有界来源摘录（不内联完整正文）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCandidateSourceExcerpt {
    pub source_event_id: SourceEventId,
    pub actor_id: String,
    pub occurred_at_unix_secs: i64,
    pub content_trust_level: ContentTrustLevel,
}

/// 供 Owner 决策的有界候选视图。conflicts_with_active_fact 表示是否已存在
/// 相同 subject 的 active MemoryFact（完全相同或内容不同由批准事务区分）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCandidateView {
    pub candidate_id: MemoryCandidateId,
    pub kind: MemoryCandidateKind,
    pub subject_key: String,
    pub status: MemoryCandidateStatus,
    pub version: MemoryCandidateVersion,
    pub payload: MemoryPayload,
    pub source_excerpts: Vec<MemoryCandidateSourceExcerpt>,
    pub conflicts_with_active_fact: bool,
}

/// 候选校验/状态转换错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MemoryCandidateError {
    #[error("invalid memory candidate: {0}")]
    InvalidData(String),
    #[error("memory candidate payload cannot be serialized: {0}")]
    Serialize(String),
}

impl From<MemoryFactError> for MemoryCandidateError {
    fn from(error: MemoryFactError) -> Self {
        Self::InvalidData(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommitmentMemory, ConversationKind, MessageSource, PersonMemory, ProjectMemory};

    fn account() -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, "account-1").unwrap()
    }

    fn actor(id: &str) -> ThreadActorRef {
        ThreadActorRef {
            account: account(),
            actor_id: id.into(),
        }
    }

    fn event(id: &str, text: &str) -> MemoryCandidateEvent {
        MemoryCandidateEvent {
            source_event_id: SourceEventId::new(id).unwrap(),
            actor: actor("alice"),
            role: MessageRole::ExternalObservation,
            occurred_at_unix_secs: 1,
            content_trust_level: ContentTrustLevel::Normal,
            normalized_text: text.into(),
            content_omitted: false,
        }
    }

    fn batch(events: Vec<MemoryCandidateEvent>) -> MemoryCandidateBatch {
        MemoryCandidateBatch {
            account: account(),
            conversation: ConversationRef::new(ConversationKind::Group, "conv-1").unwrap(),
            lease_token: MemoryCandidateLeaseToken::generate(),
            events,
            next_cursor: MemoryCandidateCursor {
                received_at_unix_micros: 1,
                source_event_id: SourceEventId::new("e0").unwrap(),
            },
        }
    }

    fn person_candidate(batch: &MemoryCandidateBatch, subject: &str) -> MemoryCandidate {
        let source = MemoryCandidateSource {
            source_event_id: batch.events[0].source_event_id.clone(),
            actor: batch.events[0].actor.clone(),
            occurred_at_unix_secs: 1,
            content_trust_level: ContentTrustLevel::Normal,
        };
        let payload = MemoryPayload::Person(PersonMemory {
            person: actor("alice"),
            relationship: Some("客户".into()),
            responsibilities: Vec::new(),
            communication_preferences: Vec::new(),
        });
        let fingerprint = candidate_fingerprint(
            &account(),
            &payload,
            subject,
            std::slice::from_ref(&source),
            "v1",
        );
        MemoryCandidate {
            candidate_id: MemoryCandidateId::generate(),
            account: account(),
            subject_key: subject.into(),
            payload,
            status: MemoryCandidateStatus::Proposed,
            version: MemoryCandidateVersion::new(1).unwrap(),
            extractor_version: "v1".into(),
            deterministic_fingerprint: fingerprint,
            sources: vec![source],
        }
    }

    #[test]
    fn out_of_batch_sources_actors_and_size_limits_are_rejected() {
        let batch = batch(vec![event("e0", "承诺：给 alice 发送报价单")]);

        // 越界来源
        let mut candidate = person_candidate(&batch, "person:alice");
        candidate.sources[0].source_event_id = SourceEventId::new("outside").unwrap();
        assert!(validate_memory_candidate(&candidate, &batch).is_err());

        // 越界 Actor（payload 主体不在批次内）
        let mut candidate = person_candidate(&batch, "person:alice");
        if let MemoryPayload::Person(person) = &mut candidate.payload {
            person.person.actor_id = "bob".into();
        }
        assert!(validate_memory_candidate(&candidate, &batch).is_err());

        // 超过 20 个来源
        let mut candidate = person_candidate(&batch, "person:alice");
        for index in 1..=20 {
            candidate.sources.push(MemoryCandidateSource {
                source_event_id: SourceEventId::new(format!("e{index}")).unwrap(),
                actor: actor("alice"),
                occurred_at_unix_secs: index as i64,
                content_trust_level: ContentTrustLevel::Normal,
            });
        }
        assert!(validate_memory_candidate(&candidate, &batch).is_err());

        // 超过 16 KiB 的 payload
        let mut candidate = person_candidate(&batch, "person:alice");
        if let MemoryPayload::Person(person) = &mut candidate.payload {
            person.relationship = Some("长".repeat(20_000));
        }
        assert!(validate_memory_candidate(&candidate, &batch).is_err());
    }

    #[test]
    fn content_trust_boundaries_gate_extraction_eligibility() {
        assert!(is_eligible_for_candidate_extraction(
            ContentTrustLevel::Normal,
            false
        ));
        assert!(!is_eligible_for_candidate_extraction(
            ContentTrustLevel::LocalOnly,
            false
        ));
        assert!(is_eligible_for_candidate_extraction(
            ContentTrustLevel::LocalOnly,
            true
        ));
        assert!(!is_eligible_for_candidate_extraction(
            ContentTrustLevel::EnvelopeOnly,
            true
        ));
        assert!(!is_eligible_for_candidate_extraction(
            ContentTrustLevel::NeverLongTerm,
            true
        ));

        // envelope_only 来源的候选被校验拒绝
        let batch = batch(vec![event("e0", "x")]);
        let mut candidate = person_candidate(&batch, "person:alice");
        candidate.sources[0].content_trust_level = ContentTrustLevel::EnvelopeOnly;
        assert!(validate_memory_candidate(&candidate, &batch).is_err());
    }

    #[test]
    fn three_kinds_convert_to_confirmed_facts_with_commitment_pending() {
        let batch = batch(vec![event("e0", "x")]);

        // Person
        let person = person_candidate(&batch, "person:alice");
        let fact = candidate_to_confirmed_fact(&person, MemoryFactId::new("fact-person").unwrap())
            .unwrap();
        assert_eq!(fact.status, MemoryFactStatus::Confirmed);
        assert_eq!(fact.payload.kind(), "person");

        // Project
        let source = person.sources[0].clone();
        let project_payload = MemoryPayload::Project(ProjectMemory {
            project_key: "alpha".into(),
            goal: "上线".into(),
            member_actor_ids: vec!["alice".into()],
            progress: None,
            decision_ids: Vec::new(),
            risks: Vec::new(),
            blockers: Vec::new(),
            artifact_refs: Vec::new(),
        });
        let project = MemoryCandidate {
            candidate_id: MemoryCandidateId::generate(),
            account: account(),
            subject_key: "project:alpha".into(),
            payload: project_payload.clone(),
            status: MemoryCandidateStatus::Proposed,
            version: MemoryCandidateVersion::new(1).unwrap(),
            extractor_version: "v1".into(),
            deterministic_fingerprint: candidate_fingerprint(
                &account(),
                &project_payload,
                "project:alpha",
                std::slice::from_ref(&source),
                "v1",
            ),
            sources: vec![source.clone()],
        };
        let fact =
            candidate_to_confirmed_fact(&project, MemoryFactId::new("fact-project").unwrap())
                .unwrap();
        assert_eq!(fact.payload.kind(), "project");

        // Commitment：批准后状态必须为 Pending，且 non-proposed 候选被拒绝
        let commitment_payload = MemoryPayload::Commitment(CommitmentMemory {
            promisor: actor("alice"),
            beneficiary: actor("alice"),
            action: "发送报价单".into(),
            due_at_unix_secs: None,
            status: CommitmentStatus::Proposed,
            completion_source_event_id: None,
        });
        let commitment = MemoryCandidate {
            candidate_id: MemoryCandidateId::generate(),
            account: account(),
            subject_key: "commitment:alice:发送报价单".into(),
            payload: commitment_payload.clone(),
            status: MemoryCandidateStatus::Proposed,
            version: MemoryCandidateVersion::new(1).unwrap(),
            extractor_version: "v1".into(),
            deterministic_fingerprint: candidate_fingerprint(
                &account(),
                &commitment_payload,
                "commitment:alice:发送报价单",
                std::slice::from_ref(&source),
                "v1",
            ),
            sources: vec![source],
        };
        let fact =
            candidate_to_confirmed_fact(&commitment, MemoryFactId::new("fact-commitment").unwrap())
                .unwrap();
        match &fact.payload {
            MemoryPayload::Commitment(commitment) => {
                assert_eq!(commitment.status, CommitmentStatus::Pending);
            }
            _ => panic!("expected commitment payload"),
        }

        let mut rejected = commitment.clone();
        rejected.status = MemoryCandidateStatus::Rejected;
        assert!(
            candidate_to_confirmed_fact(&rejected, MemoryFactId::new("fact-x").unwrap()).is_err()
        );
    }
}
