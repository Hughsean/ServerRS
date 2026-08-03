use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    ContentTrustLevel, ConversationRef, SourceAccountRef, SourceEventId, ThreadActorRef,
    ThreadDecisionId,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryFactId(String);

impl MemoryFactId {
    pub fn new(value: impl Into<String>) -> Result<Self, MemoryFactError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(MemoryFactError::Invalid(
                "memory_fact_id must not be empty".into(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryFactStatus {
    Proposed,
    Confirmed,
    Superseded,
    Expired,
    Deleted,
}

impl MemoryFactStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Confirmed => "confirmed",
            Self::Superseded => "superseded",
            Self::Expired => "expired",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitmentStatus {
    Proposed,
    Pending,
    Fulfilled,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonMemory {
    pub person: ThreadActorRef,
    pub relationship: Option<String>,
    pub responsibilities: Vec<String>,
    pub communication_preferences: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectMemory {
    pub project_key: String,
    pub goal: String,
    pub member_actor_ids: Vec<String>,
    pub progress: Option<String>,
    pub decision_ids: Vec<ThreadDecisionId>,
    pub risks: Vec<String>,
    pub blockers: Vec<String>,
    pub artifact_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitmentMemory {
    pub promisor: ThreadActorRef,
    pub beneficiary: ThreadActorRef,
    pub action: String,
    pub due_at_unix_secs: Option<i64>,
    pub status: CommitmentStatus,
    pub completion_source_event_id: Option<SourceEventId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum MemoryPayload {
    Person(PersonMemory),
    Project(ProjectMemory),
    Commitment(CommitmentMemory),
}

impl MemoryPayload {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Person(_) => "person",
            Self::Project(_) => "project",
            Self::Commitment(_) => "commitment",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryFact {
    pub fact_id: MemoryFactId,
    pub account: SourceAccountRef,
    pub subject_key: String,
    pub payload: MemoryPayload,
    pub status: MemoryFactStatus,
    pub confidence_bps: u16,
    pub source_event_ids: Vec<SourceEventId>,
    pub valid_until_unix_secs: Option<i64>,
    pub supersedes_fact_id: Option<MemoryFactId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryWriteReceipt {
    pub fact_id: MemoryFactId,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySourceExcerpt {
    pub source_event_id: SourceEventId,
    pub conversation_kind: String,
    pub conversation_id: String,
    pub actor_id: String,
    pub occurred_at_unix_secs: i64,
    pub excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryFactView {
    pub fact: MemoryFact,
    pub sources: Vec<MemorySourceExcerpt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryDeleteInput {
    pub fact_id: MemoryFactId,
    pub command_source_event_id: SourceEventId,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryDeleteReceipt {
    pub fact_id: MemoryFactId,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationMemoryModeInput {
    pub account: SourceAccountRef,
    pub conversation: ConversationRef,
    pub command_source_event_id: SourceEventId,
    pub mode: ContentTrustLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationMemoryModeReceipt {
    pub changed: bool,
    pub previous_mode: ContentTrustLevel,
    pub current_mode: ContentTrustLevel,
}

pub fn validate_memory_delete(input: &MemoryDeleteInput) -> Result<(), MemoryFactError> {
    validate_text("memory delete reason", &input.reason, 1000)
}

pub fn validate_memory_fact(fact: &MemoryFact) -> Result<(), MemoryFactError> {
    if fact.subject_key.trim().is_empty() || fact.subject_key.chars().count() > 191 {
        return Err(MemoryFactError::Invalid(
            "subject_key must contain 1..=191 characters".into(),
        ));
    }
    if !matches!(
        fact.status,
        MemoryFactStatus::Proposed | MemoryFactStatus::Confirmed
    ) {
        return Err(MemoryFactError::Invalid(
            "new memory fact status must be proposed or confirmed".into(),
        ));
    }
    if fact.confidence_bps > 10_000 {
        return Err(MemoryFactError::Invalid(
            "memory confidence_bps must not exceed 10000".into(),
        ));
    }
    if fact.supersedes_fact_id.as_ref() == Some(&fact.fact_id) {
        return Err(MemoryFactError::Invalid(
            "memory fact cannot supersede itself".into(),
        ));
    }
    validate_memory_payload(&fact.payload, &fact.account, &fact.source_event_ids)
}

/// 校验记忆 payload 本身与来源引用；供 MemoryFact 与记忆候选共用，
/// 避免候选校验复制一套稍有不同的字段上限。
pub fn validate_memory_payload(
    payload: &MemoryPayload,
    account: &SourceAccountRef,
    source_event_ids: &[SourceEventId],
) -> Result<(), MemoryFactError> {
    if source_event_ids.is_empty() || source_event_ids.len() > 100 {
        return Err(MemoryFactError::Invalid(
            "memory fact must reference 1..=100 source events".into(),
        ));
    }
    if source_event_ids
        .iter()
        .map(SourceEventId::as_str)
        .collect::<HashSet<_>>()
        .len()
        != source_event_ids.len()
    {
        return Err(MemoryFactError::Invalid(
            "memory fact contains duplicate source events".into(),
        ));
    }
    match payload {
        MemoryPayload::Person(person) => {
            ensure_actor_account(account, &person.person)?;
            validate_optional_text("relationship", person.relationship.as_deref(), 1000)?;
            validate_text_list("responsibilities", &person.responsibilities, 50, 1000)?;
            validate_text_list(
                "communication_preferences",
                &person.communication_preferences,
                50,
                1000,
            )?;
        }
        MemoryPayload::Project(project) => {
            validate_text("project_key", &project.project_key, 191)?;
            validate_text("goal", &project.goal, 4000)?;
            validate_optional_text("progress", project.progress.as_deref(), 4000)?;
            validate_text_list("member_actor_ids", &project.member_actor_ids, 100, 191)?;
            validate_text_list("risks", &project.risks, 100, 1000)?;
            validate_text_list("blockers", &project.blockers, 100, 1000)?;
            validate_text_list("artifact_refs", &project.artifact_refs, 100, 1000)?;
        }
        MemoryPayload::Commitment(commitment) => {
            ensure_actor_account(account, &commitment.promisor)?;
            ensure_actor_account(account, &commitment.beneficiary)?;
            validate_text("commitment.action", &commitment.action, 4000)?;
            if commitment.status == CommitmentStatus::Fulfilled
                && commitment.completion_source_event_id.is_none()
            {
                return Err(MemoryFactError::Invalid(
                    "fulfilled commitment requires completion evidence".into(),
                ));
            }
            if let Some(completion) = &commitment.completion_source_event_id
                && !source_event_ids.contains(completion)
            {
                return Err(MemoryFactError::Invalid(
                    "completion evidence must be included in source_event_ids".into(),
                ));
            }
        }
    }
    Ok(())
}

fn ensure_actor_account(
    account: &SourceAccountRef,
    actor: &ThreadActorRef,
) -> Result<(), MemoryFactError> {
    if &actor.account != account {
        return Err(MemoryFactError::Invalid(
            "memory actor must belong to the managed account scope".into(),
        ));
    }
    validate_text("actor_id", &actor.actor_id, 191)
}

fn validate_optional_text(
    field: &str,
    value: Option<&str>,
    max: usize,
) -> Result<(), MemoryFactError> {
    if let Some(value) = value {
        validate_text(field, value, max)?;
    }
    Ok(())
}

fn validate_text(field: &str, value: &str, max: usize) -> Result<(), MemoryFactError> {
    if value.trim().is_empty() || value.chars().count() > max {
        return Err(MemoryFactError::Invalid(format!(
            "{field} must contain 1..={max} characters"
        )));
    }
    Ok(())
}

fn validate_text_list(
    field: &str,
    values: &[String],
    max_items: usize,
    max_chars: usize,
) -> Result<(), MemoryFactError> {
    if values.len() > max_items {
        return Err(MemoryFactError::Invalid(format!(
            "{field} must not exceed {max_items} items"
        )));
    }
    for value in values {
        validate_text(field, value, max_chars)?;
    }
    Ok(())
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MemoryFactError {
    #[error("invalid memory fact: {0}")]
    Invalid(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MessageSource;

    fn account() -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, "account").unwrap()
    }

    #[test]
    fn fulfilled_commitment_requires_source_backed_completion() {
        let managed = account();
        let fact = MemoryFact {
            fact_id: MemoryFactId::generate(),
            account: managed.clone(),
            subject_key: "commitment:quote".into(),
            payload: MemoryPayload::Commitment(CommitmentMemory {
                promisor: ThreadActorRef {
                    account: managed.clone(),
                    actor_id: "alice".into(),
                    platform_identity_kind: None,
                },
                beneficiary: ThreadActorRef {
                    account: managed,
                    actor_id: "owner".into(),
                    platform_identity_kind: None,
                },
                action: "发送报价单".into(),
                due_at_unix_secs: Some(1_800_000_000),
                status: CommitmentStatus::Fulfilled,
                completion_source_event_id: None,
            }),
            status: MemoryFactStatus::Confirmed,
            confidence_bps: 10_000,
            source_event_ids: vec![SourceEventId::new("source").unwrap()],
            valid_until_unix_secs: None,
            supersedes_fact_id: None,
        };
        assert!(matches!(
            validate_memory_fact(&fact),
            Err(MemoryFactError::Invalid(_))
        ));
    }
}
