use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{ContentSegment, ConversationRef, EventThreadId, SourceAccountRef, SourceEventId};

macro_rules! link_id {
    ($name:ident, $field:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ThreadLinkError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(ThreadLinkError::InvalidData(
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

link_id!(ThreadLinkCandidateId, "thread_link_candidate_id");
link_id!(ThreadLinkLeaseToken, "thread_link_lease_token");
link_id!(ThreadLinkReviewId, "thread_link_review_id");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadLinkSignalKind {
    ExplicitProjectId,
    ExactFileSourceKey,
    ExplicitFileVersion,
    ExactForwardSourceKey,
    ExactRichContentKey,
    SharedActor,
    SimilarTopic,
    SameFileName,
}

impl ThreadLinkSignalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitProjectId => "explicit_project_id",
            Self::ExactFileSourceKey => "exact_file_source_key",
            Self::ExplicitFileVersion => "explicit_file_version",
            Self::ExactForwardSourceKey => "exact_forward_source_key",
            Self::ExactRichContentKey => "exact_rich_content_key",
            Self::SharedActor => "shared_actor",
            Self::SimilarTopic => "similar_topic",
            Self::SameFileName => "same_file_name",
        }
    }

    pub fn is_strong(self) -> bool {
        matches!(
            self,
            Self::ExplicitProjectId
                | Self::ExactFileSourceKey
                | Self::ExplicitFileVersion
                | Self::ExactForwardSourceKey
                | Self::ExactRichContentKey
        )
    }

    pub fn confidence_bps(self) -> u16 {
        match self {
            Self::ExplicitProjectId => 9500,
            Self::ExactFileSourceKey => 9000,
            Self::ExplicitFileVersion => 9800,
            Self::ExactForwardSourceKey => 9000,
            Self::ExactRichContentKey => 8500,
            Self::SharedActor | Self::SimilarTopic | Self::SameFileName => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadLinkCandidateStatus {
    Proposed,
    Accepted,
    Rejected,
    Expired,
}

impl ThreadLinkCandidateStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadLinkEvent {
    pub source_event_id: SourceEventId,
    pub account: SourceAccountRef,
    pub conversation: ConversationRef,
    pub thread_id: EventThreadId,
    pub normalized_text: String,
    pub segments: Vec<ContentSegment>,
    pub content_omitted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedThreadLinkBatch {
    pub lease_token: ThreadLinkLeaseToken,
    pub events: Vec<ThreadLinkEvent>,
}

/// 只保存不可逆指纹，不把项目编号或文件源键复制到关联索引。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadLinkHint {
    pub source_event_id: SourceEventId,
    pub account: SourceAccountRef,
    pub conversation: ConversationRef,
    pub thread_id: EventThreadId,
    pub kind: ThreadLinkSignalKind,
    pub fingerprint_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadLinkEvidence {
    pub kind: ThreadLinkSignalKind,
    pub fingerprint_sha256: String,
    pub left_source_event_id: SourceEventId,
    pub right_source_event_id: SourceEventId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadLinkCandidate {
    pub candidate_id: ThreadLinkCandidateId,
    pub account: SourceAccountRef,
    pub left_thread_id: EventThreadId,
    pub right_thread_id: EventThreadId,
    pub left_conversation: ConversationRef,
    pub right_conversation: ConversationRef,
    pub status: ThreadLinkCandidateStatus,
    pub confidence_bps: u16,
    pub reason_code: String,
    pub evidence: ThreadLinkEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadLinkReviewAction {
    Accept,
    Reject,
}

impl ThreadLinkReviewAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Reject => "reject",
        }
    }

    pub fn target_status(self) -> ThreadLinkCandidateStatus {
        match self {
            Self::Accept => ThreadLinkCandidateStatus::Accepted,
            Self::Reject => ThreadLinkCandidateStatus::Rejected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadLinkReviewCommand {
    pub source_event_id: SourceEventId,
    pub actor: crate::ThreadActorRef,
    pub role: crate::MessageRole,
    /// 由本地配置建立的显式 Owner 绑定所授权管理的账号，不从聊天文本推断。
    pub authorized_account: SourceAccountRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadLinkReviewContext {
    pub candidate_id: ThreadLinkCandidateId,
    pub candidate_account: SourceAccountRef,
    pub candidate_status: ThreadLinkCandidateStatus,
    pub command: ThreadLinkReviewCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedThreadLinkReview {
    pub review_id: ThreadLinkReviewId,
    pub candidate_id: ThreadLinkCandidateId,
    pub action: ThreadLinkReviewAction,
    pub target_status: ThreadLinkCandidateStatus,
    pub command_source_event_id: SourceEventId,
    pub owner: crate::ThreadActorRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadLinkCandidateCursor {
    pub created_at_unix_micros: i64,
    pub candidate_id: ThreadLinkCandidateId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadLinkSourceExcerpt {
    pub source_event_id: SourceEventId,
    pub conversation: ConversationRef,
    pub actor_id: String,
    pub occurred_at_unix_secs: i64,
    /// 面向 Owner 的有界原文片段；不得写入日志。
    pub excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadLinkCandidateView {
    pub candidate_id: ThreadLinkCandidateId,
    pub left_thread_id: EventThreadId,
    pub right_thread_id: EventThreadId,
    pub left_conversation: ConversationRef,
    pub right_conversation: ConversationRef,
    pub status: ThreadLinkCandidateStatus,
    pub confidence_bps: u16,
    pub reason_code: String,
    pub sources: Vec<ThreadLinkSourceExcerpt>,
    pub cursor: ThreadLinkCandidateCursor,
}

pub fn validate_thread_link_review(
    context: &ThreadLinkReviewContext,
    action: ThreadLinkReviewAction,
) -> Result<ValidatedThreadLinkReview, ThreadLinkError> {
    if context.candidate_status != ThreadLinkCandidateStatus::Proposed
        && context.candidate_status != action.target_status()
    {
        return Err(ThreadLinkError::ReviewConflict(
            context.candidate_status.as_str().into(),
        ));
    }
    if context.command.role != crate::MessageRole::OwnerCommand {
        return Err(ThreadLinkError::OwnerCommandRequired);
    }
    if context.command.authorized_account != context.candidate_account {
        return Err(ThreadLinkError::CrossAccountReview);
    }
    if context.command.actor.actor_id.trim().is_empty() {
        return Err(ThreadLinkError::InvalidData(
            "review owner actor id must not be empty".into(),
        ));
    }
    Ok(ValidatedThreadLinkReview {
        review_id: ThreadLinkReviewId::generate(),
        candidate_id: context.candidate_id.clone(),
        action,
        target_status: action.target_status(),
        command_source_event_id: context.command.source_event_id.clone(),
        owner: context.command.actor.clone(),
    })
}

pub fn validate_thread_link_candidate(
    candidate: &ThreadLinkCandidate,
) -> Result<(), ThreadLinkError> {
    if candidate.status != ThreadLinkCandidateStatus::Proposed {
        return Err(ThreadLinkError::InvalidData(
            "new cross-conversation link candidate must be proposed".into(),
        ));
    }
    if candidate.left_thread_id == candidate.right_thread_id
        || candidate.left_conversation == candidate.right_conversation
    {
        return Err(ThreadLinkError::InvalidData(
            "cross-conversation link candidate requires two different conversations and threads"
                .into(),
        ));
    }
    if candidate.left_thread_id.as_str() >= candidate.right_thread_id.as_str() {
        return Err(ThreadLinkError::InvalidData(
            "thread link pair must use canonical lexical order".into(),
        ));
    }
    if !candidate.evidence.kind.is_strong() {
        return Err(ThreadLinkError::WeakEvidence(
            candidate.evidence.kind.as_str().into(),
        ));
    }
    if candidate.confidence_bps == 0 || candidate.confidence_bps > 10_000 {
        return Err(ThreadLinkError::InvalidData(
            "confidence_bps must be between 1 and 10000".into(),
        ));
    }
    if candidate.evidence.fingerprint_sha256.len() != 64
        || !candidate
            .evidence
            .fingerprint_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ThreadLinkError::InvalidData(
            "link evidence fingerprint must be SHA-256 hex".into(),
        ));
    }
    if candidate.reason_code != candidate.evidence.kind.as_str() {
        return Err(ThreadLinkError::InvalidData(
            "reason_code must match the typed evidence kind".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ThreadLinkError {
    #[error("invalid thread link data: {0}")]
    InvalidData(String),
    #[error("weak evidence cannot create a cross-conversation candidate: {0}")]
    WeakEvidence(String),
    #[error("thread link review requires a verified OwnerCommand event")]
    OwnerCommandRequired,
    #[error("thread link review command belongs to another account")]
    CrossAccountReview,
    #[error("thread link candidate has already left proposed state: {0}")]
    ReviewConflict(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConversationKind, MessageSource};

    fn candidate(kind: ThreadLinkSignalKind) -> ThreadLinkCandidate {
        ThreadLinkCandidate {
            candidate_id: ThreadLinkCandidateId::new("candidate").unwrap(),
            account: SourceAccountRef::new(MessageSource::NapCat, "account").unwrap(),
            left_thread_id: EventThreadId::new("a").unwrap(),
            right_thread_id: EventThreadId::new("b").unwrap(),
            left_conversation: ConversationRef::new(ConversationKind::Group, "group").unwrap(),
            right_conversation: ConversationRef::new(ConversationKind::Private, "private").unwrap(),
            status: ThreadLinkCandidateStatus::Proposed,
            confidence_bps: kind.confidence_bps(),
            reason_code: kind.as_str().into(),
            evidence: ThreadLinkEvidence {
                kind,
                fingerprint_sha256: "a".repeat(64),
                left_source_event_id: SourceEventId::new("left").unwrap(),
                right_source_event_id: SourceEventId::new("right").unwrap(),
            },
        }
    }

    #[test]
    fn weak_signals_never_create_candidates() {
        for kind in [
            ThreadLinkSignalKind::SharedActor,
            ThreadLinkSignalKind::SimilarTopic,
            ThreadLinkSignalKind::SameFileName,
        ] {
            assert_eq!(
                validate_thread_link_candidate(&candidate(kind)),
                Err(ThreadLinkError::WeakEvidence(kind.as_str().into()))
            );
        }
    }

    #[test]
    fn strong_signal_still_only_creates_a_proposed_candidate() {
        for kind in [
            ThreadLinkSignalKind::ExplicitProjectId,
            ThreadLinkSignalKind::ExactFileSourceKey,
            ThreadLinkSignalKind::ExplicitFileVersion,
            ThreadLinkSignalKind::ExactForwardSourceKey,
            ThreadLinkSignalKind::ExactRichContentKey,
        ] {
            let value = candidate(kind);
            assert!(validate_thread_link_candidate(&value).is_ok());
            assert_eq!(value.status, ThreadLinkCandidateStatus::Proposed);
        }
    }

    #[test]
    fn review_requires_same_account_owner_command() {
        let account = SourceAccountRef::new(MessageSource::NapCat, "account").unwrap();
        let mut context = ThreadLinkReviewContext {
            candidate_id: ThreadLinkCandidateId::new("candidate").unwrap(),
            candidate_account: account.clone(),
            candidate_status: ThreadLinkCandidateStatus::Proposed,
            command: ThreadLinkReviewCommand {
                source_event_id: SourceEventId::new("command").unwrap(),
                actor: crate::ThreadActorRef {
                    account,
                    actor_id: "owner".into(),
                    platform_identity_kind: None,
                },
                role: crate::MessageRole::OwnerObservation,
                authorized_account: SourceAccountRef::new(MessageSource::NapCat, "account")
                    .unwrap(),
            },
        };
        assert_eq!(
            validate_thread_link_review(&context, ThreadLinkReviewAction::Accept),
            Err(ThreadLinkError::OwnerCommandRequired)
        );
        context.command.role = crate::MessageRole::OwnerCommand;
        let review = validate_thread_link_review(&context, ThreadLinkReviewAction::Accept).unwrap();
        assert_eq!(review.target_status, ThreadLinkCandidateStatus::Accepted);
    }
}
