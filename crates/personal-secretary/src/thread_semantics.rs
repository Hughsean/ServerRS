use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    ClaimStatus, DecisionStatus, EventThreadId, MessageRole, OpenQuestion, OpenQuestionId,
    QuestionStatus, SourceEventId, ThreadActorRef, ThreadClaim, ThreadClaimId, ThreadDecision,
    ThreadDecisionId, ThreadStatus,
};

macro_rules! semantic_id {
    ($name:ident, $field:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ThreadSemanticError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(ThreadSemanticError::InvalidData(
                        concat!($field, " must not be empty").into(),
                    ));
                }
                Ok(Self(value))
            }

            pub fn generate() -> Self {
                Self(Uuid::new_v4().to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

semantic_id!(ThreadSemanticLeaseToken, "thread_semantic_lease_token");
semantic_id!(ThreadStatusChangeId, "thread_status_change_id");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimKind {
    Request,
    Objection,
    Confirmation,
}

impl ClaimKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Objection => "objection",
            Self::Confirmation => "confirmation",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleAuthority {
    EvidenceDerived,
    OwnerConfirmed,
    SystemRecovery,
}

impl LifecycleAuthority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EvidenceDerived => "evidence_derived",
            Self::OwnerConfirmed => "owner_confirmed",
            Self::SystemRecovery => "system_recovery",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadSemanticCursor {
    pub added_at_unix_micros: i64,
    pub source_event_id: SourceEventId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadSemanticEvent {
    pub source_event_id: SourceEventId,
    pub actor: ThreadActorRef,
    pub role: MessageRole,
    pub occurred_at_unix_secs: i64,
    pub normalized_text: String,
    /// 正文超过本轮字符预算时为 true；提取器必须跳过，不能基于截断文本推断事实。
    pub content_omitted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedThreadSemanticBatch {
    pub lease_token: ThreadSemanticLeaseToken,
    pub thread_id: EventThreadId,
    pub current_status: ThreadStatus,
    pub confirmed_decision_ids: Vec<ThreadDecisionId>,
    pub open_question_ids: Vec<OpenQuestionId>,
    pub events: Vec<ThreadSemanticEvent>,
    pub next_cursor: ThreadSemanticCursor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadClaimCandidate {
    pub claim_id: ThreadClaimId,
    pub thread_id: EventThreadId,
    pub kind: ClaimKind,
    pub claimant: ThreadActorRef,
    pub statement: String,
    pub confidence_bps: u16,
    pub source_event_ids: Vec<SourceEventId>,
}

impl ThreadClaimCandidate {
    pub fn into_claim(self) -> ThreadClaim {
        ThreadClaim {
            claim_id: self.claim_id,
            thread_id: self.thread_id,
            kind: self.kind,
            claimant: self.claimant,
            statement: self.statement,
            status: ClaimStatus::Proposed,
            confidence_bps: self.confidence_bps,
            source_event_ids: self.source_event_ids,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadDecisionCandidate {
    pub decision_id: ThreadDecisionId,
    pub thread_id: EventThreadId,
    pub statement: String,
    pub confidence_bps: u16,
    pub supersedes: Option<ThreadDecisionId>,
    pub source_event_ids: Vec<SourceEventId>,
}

impl ThreadDecisionCandidate {
    pub fn into_decision(self) -> ThreadDecision {
        ThreadDecision {
            decision_id: self.decision_id,
            thread_id: self.thread_id,
            statement: self.statement,
            status: DecisionStatus::Proposed,
            confidence_bps: self.confidence_bps,
            supersedes: self.supersedes,
            source_event_ids: self.source_event_ids,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenQuestionCandidate {
    pub question_id: OpenQuestionId,
    pub thread_id: EventThreadId,
    pub question: String,
    pub raised_by: ThreadActorRef,
    pub confidence_bps: u16,
    pub source_event_ids: Vec<SourceEventId>,
}

impl OpenQuestionCandidate {
    pub fn into_question(self) -> OpenQuestion {
        OpenQuestion {
            question_id: self.question_id,
            thread_id: self.thread_id,
            question: self.question,
            raised_by: self.raised_by,
            status: QuestionStatus::Open,
            confidence_bps: self.confidence_bps,
            source_event_ids: self.source_event_ids,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadLifecycleChange {
    pub change_id: ThreadStatusChangeId,
    pub thread_id: EventThreadId,
    pub from: ThreadStatus,
    pub to: ThreadStatus,
    pub authority: LifecycleAuthority,
    pub reason: String,
    pub source_event_ids: Vec<SourceEventId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSemanticPatch {
    pub claims: Vec<ThreadClaimCandidate>,
    pub decisions: Vec<ThreadDecisionCandidate>,
    pub questions: Vec<OpenQuestionCandidate>,
    pub lifecycle_change: Option<ThreadLifecycleChange>,
}

/// 对提取器输出执行不可绕过的业务校验。提取器无论是规则、LLM 还是人工控制面，都只能
/// 生成候选补丁；来源越界、静默覆盖结论或无权限关闭线程会在此被拒绝。
pub fn validate_semantic_patch(
    batch: &ClaimedThreadSemanticBatch,
    patch: &ThreadSemanticPatch,
) -> Result<(), ThreadSemanticError> {
    const MAX_ITEMS_PER_KIND: usize = 1000;
    if patch.claims.len() > MAX_ITEMS_PER_KIND
        || patch.decisions.len() > MAX_ITEMS_PER_KIND
        || patch.questions.len() > MAX_ITEMS_PER_KIND
    {
        return Err(ThreadSemanticError::InvalidData(
            "semantic patch exceeds the maximum number of candidates".into(),
        ));
    }
    let event_ids = batch
        .events
        .iter()
        .map(|event| event.source_event_id.as_str())
        .collect::<HashSet<_>>();
    let mut candidate_ids = HashSet::new();

    for claim in &patch.claims {
        require_thread(&batch.thread_id, &claim.thread_id)?;
        require_text("claim.statement", &claim.statement)?;
        require_confidence(claim.confidence_bps)?;
        require_sources(&event_ids, &claim.source_event_ids)?;
        if !candidate_ids.insert(format!("claim:{}", claim.claim_id.as_str())) {
            return Err(ThreadSemanticError::InvalidData(
                "semantic patch contains a duplicate candidate id".into(),
            ));
        }
        if !batch
            .events
            .iter()
            .any(|event| event.actor == claim.claimant)
        {
            return Err(ThreadSemanticError::InvalidData(
                "claimant must be an actor observed in the claimed batch".into(),
            ));
        }
    }
    for decision in &patch.decisions {
        require_thread(&batch.thread_id, &decision.thread_id)?;
        require_text("decision.statement", &decision.statement)?;
        require_confidence(decision.confidence_bps)?;
        require_sources(&event_ids, &decision.source_event_ids)?;
        if !candidate_ids.insert(format!("decision:{}", decision.decision_id.as_str())) {
            return Err(ThreadSemanticError::InvalidData(
                "semantic patch contains a duplicate candidate id".into(),
            ));
        }
        if let Some(supersedes) = &decision.supersedes
            && !batch.confirmed_decision_ids.contains(supersedes)
        {
            return Err(ThreadSemanticError::InvalidData(
                "decision may supersede only a confirmed decision in the same thread".into(),
            ));
        }
    }
    for question in &patch.questions {
        require_thread(&batch.thread_id, &question.thread_id)?;
        require_text("question.question", &question.question)?;
        require_confidence(question.confidence_bps)?;
        require_sources(&event_ids, &question.source_event_ids)?;
        if !candidate_ids.insert(format!("question:{}", question.question_id.as_str())) {
            return Err(ThreadSemanticError::InvalidData(
                "semantic patch contains a duplicate candidate id".into(),
            ));
        }
        if !batch
            .events
            .iter()
            .any(|event| event.actor == question.raised_by)
        {
            return Err(ThreadSemanticError::InvalidData(
                "question raiser must be an actor observed in the claimed batch".into(),
            ));
        }
    }
    if let Some(change) = &patch.lifecycle_change {
        require_thread(&batch.thread_id, &change.thread_id)?;
        if change.from != batch.current_status {
            return Err(ThreadSemanticError::InvalidTransition {
                from: batch.current_status,
                to: change.to,
            });
        }
        validate_thread_transition(change.from, change.to)?;
        require_text_with_max("lifecycle.reason", &change.reason, 1000)?;
        require_sources(&event_ids, &change.source_event_ids)?;
        if change.to == ThreadStatus::Closed {
            if change.authority != LifecycleAuthority::OwnerConfirmed {
                return Err(ThreadSemanticError::CloseRequiresOwnerConfirmation);
            }
            if !batch.open_question_ids.is_empty() {
                return Err(ThreadSemanticError::CloseWithOpenQuestions);
            }
            let owner_command_is_source = batch.events.iter().any(|event| {
                event.role == MessageRole::OwnerCommand
                    && change.source_event_ids.contains(&event.source_event_id)
            });
            if !owner_command_is_source {
                return Err(ThreadSemanticError::CloseRequiresOwnerConfirmation);
            }
        }
    }
    Ok(())
}

pub fn validate_thread_transition(
    from: ThreadStatus,
    to: ThreadStatus,
) -> Result<(), ThreadSemanticError> {
    if from == to {
        return Ok(());
    }
    let allowed = matches!(
        (from, to),
        (ThreadStatus::Open, ThreadStatus::Waiting)
            | (ThreadStatus::Open, ThreadStatus::Resolved)
            | (ThreadStatus::Waiting, ThreadStatus::Open)
            | (ThreadStatus::Waiting, ThreadStatus::Resolved)
            | (ThreadStatus::Resolved, ThreadStatus::Closed)
            | (ThreadStatus::Resolved, ThreadStatus::Reopened)
            | (ThreadStatus::Closed, ThreadStatus::Reopened)
            | (ThreadStatus::Reopened, ThreadStatus::Open)
            | (ThreadStatus::Reopened, ThreadStatus::Waiting)
            | (ThreadStatus::Reopened, ThreadStatus::Resolved)
    );
    if allowed {
        Ok(())
    } else {
        Err(ThreadSemanticError::InvalidTransition { from, to })
    }
}

fn require_thread(
    expected: &EventThreadId,
    actual: &EventThreadId,
) -> Result<(), ThreadSemanticError> {
    if expected == actual {
        Ok(())
    } else {
        Err(ThreadSemanticError::InvalidData(
            "semantic candidate belongs to another thread".into(),
        ))
    }
}

fn require_text(field: &str, value: &str) -> Result<(), ThreadSemanticError> {
    require_text_with_max(field, value, 4000)
}

fn require_text_with_max(
    field: &str,
    value: &str,
    max_chars: usize,
) -> Result<(), ThreadSemanticError> {
    let len = value.chars().count();
    if value.trim().is_empty() || len > max_chars {
        Err(ThreadSemanticError::InvalidData(format!(
            "{field} must contain 1..={max_chars} characters"
        )))
    } else {
        Ok(())
    }
}

fn require_confidence(value: u16) -> Result<(), ThreadSemanticError> {
    if value <= 10_000 {
        Ok(())
    } else {
        Err(ThreadSemanticError::InvalidData(
            "confidence_bps must be <= 10000".into(),
        ))
    }
}

fn require_sources(
    allowed: &HashSet<&str>,
    sources: &[SourceEventId],
) -> Result<(), ThreadSemanticError> {
    if sources.is_empty() {
        return Err(ThreadSemanticError::InvalidData(
            "semantic candidate must cite at least one source event".into(),
        ));
    }
    if sources.len() > 32 {
        return Err(ThreadSemanticError::InvalidData(
            "semantic candidate may cite at most 32 source events".into(),
        ));
    }
    let unique = sources
        .iter()
        .map(SourceEventId::as_str)
        .collect::<HashSet<_>>();
    if unique.len() != sources.len() {
        return Err(ThreadSemanticError::InvalidData(
            "semantic candidate contains duplicate source events".into(),
        ));
    }
    if sources.iter().all(|id| allowed.contains(id.as_str())) {
        Ok(())
    } else {
        Err(ThreadSemanticError::InvalidData(
            "semantic candidate cites an event outside the claimed thread batch".into(),
        ))
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ThreadSemanticError {
    #[error("invalid thread semantic data: {0}")]
    InvalidData(String),
    #[error("invalid thread lifecycle transition {from:?} -> {to:?}")]
    InvalidTransition {
        from: ThreadStatus,
        to: ThreadStatus,
    },
    #[error("closing a thread requires a verified OwnerCommand source")]
    CloseRequiresOwnerConfirmation,
    #[error("a thread with open questions cannot be closed")]
    CloseWithOpenQuestions,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MessageSource, SourceAccountRef};

    fn actor(id: &str) -> ThreadActorRef {
        ThreadActorRef {
            account: SourceAccountRef::new(MessageSource::NapCat, "account").unwrap(),
            actor_id: id.into(),
        }
    }

    fn batch(status: ThreadStatus, role: MessageRole) -> ClaimedThreadSemanticBatch {
        ClaimedThreadSemanticBatch {
            lease_token: ThreadSemanticLeaseToken::new("lease").unwrap(),
            thread_id: EventThreadId::new("thread").unwrap(),
            current_status: status,
            confirmed_decision_ids: vec![ThreadDecisionId::new("old-decision").unwrap()],
            open_question_ids: Vec::new(),
            events: vec![ThreadSemanticEvent {
                source_event_id: SourceEventId::new("event").unwrap(),
                actor: actor("alice"),
                role,
                occurred_at_unix_secs: 1,
                normalized_text: "确认关闭".into(),
                content_omitted: false,
            }],
            next_cursor: ThreadSemanticCursor {
                added_at_unix_micros: 1,
                source_event_id: SourceEventId::new("event").unwrap(),
            },
        }
    }

    #[test]
    fn source_event_must_belong_to_the_claimed_batch() {
        let batch = batch(ThreadStatus::Open, MessageRole::ExternalObservation);
        let patch = ThreadSemanticPatch {
            claims: vec![ThreadClaimCandidate {
                claim_id: ThreadClaimId::new("claim").unwrap(),
                thread_id: batch.thread_id.clone(),
                kind: ClaimKind::Request,
                claimant: actor("alice"),
                statement: "发送报价".into(),
                confidence_bps: 9000,
                source_event_ids: vec![SourceEventId::new("another-event").unwrap()],
            }],
            ..ThreadSemanticPatch::default()
        };
        assert!(validate_semantic_patch(&batch, &patch).is_err());
    }

    #[test]
    fn decision_revision_must_point_to_a_confirmed_decision() {
        let batch = batch(ThreadStatus::Open, MessageRole::ExternalObservation);
        let patch = ThreadSemanticPatch {
            decisions: vec![ThreadDecisionCandidate {
                decision_id: ThreadDecisionId::new("new").unwrap(),
                thread_id: batch.thread_id.clone(),
                statement: "改到周五".into(),
                confidence_bps: 9000,
                supersedes: Some(ThreadDecisionId::new("unknown").unwrap()),
                source_event_ids: vec![SourceEventId::new("event").unwrap()],
            }],
            ..ThreadSemanticPatch::default()
        };
        assert!(validate_semantic_patch(&batch, &patch).is_err());
    }

    #[test]
    fn close_requires_owner_command_and_no_open_questions() {
        let mut batch = batch(ThreadStatus::Resolved, MessageRole::OwnerObservation);
        let close = |batch: &ClaimedThreadSemanticBatch| ThreadSemanticPatch {
            lifecycle_change: Some(ThreadLifecycleChange {
                change_id: ThreadStatusChangeId::new("change").unwrap(),
                thread_id: batch.thread_id.clone(),
                from: ThreadStatus::Resolved,
                to: ThreadStatus::Closed,
                authority: LifecycleAuthority::OwnerConfirmed,
                reason: "Owner 确认关闭".into(),
                source_event_ids: vec![SourceEventId::new("event").unwrap()],
            }),
            ..ThreadSemanticPatch::default()
        };
        assert_eq!(
            validate_semantic_patch(&batch, &close(&batch)),
            Err(ThreadSemanticError::CloseRequiresOwnerConfirmation)
        );
        batch.events[0].role = MessageRole::OwnerCommand;
        batch.open_question_ids = vec![OpenQuestionId::new("question").unwrap()];
        assert_eq!(
            validate_semantic_patch(&batch, &close(&batch)),
            Err(ThreadSemanticError::CloseWithOpenQuestions)
        );
        batch.open_question_ids.clear();
        assert!(validate_semantic_patch(&batch, &close(&batch)).is_ok());
    }

    #[test]
    fn closed_thread_can_only_reopen() {
        assert!(validate_thread_transition(ThreadStatus::Closed, ThreadStatus::Reopened).is_ok());
        assert!(validate_thread_transition(ThreadStatus::Closed, ThreadStatus::Open).is_err());
    }
}
