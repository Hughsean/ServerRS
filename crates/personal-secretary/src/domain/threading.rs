use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{ConversationRef, SourceAccountRef, SourceEventId};

macro_rules! thread_id {
    ($name:ident, $field:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ThreadingError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(ThreadingError::InvalidData(
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

thread_id!(EventThreadId, "event_thread_id");
thread_id!(ThreadClaimId, "thread_claim_id");
thread_id!(ThreadDecisionId, "thread_decision_id");
thread_id!(OpenQuestionId, "open_question_id");
thread_id!(ThreadRelationId, "thread_relation_id");
thread_id!(ThreadProjectionLeaseToken, "thread_projection_lease_token");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Open,
    Waiting,
    Resolved,
    Closed,
    Reopened,
}

impl ThreadStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Waiting => "waiting",
            Self::Resolved => "resolved",
            Self::Closed => "closed",
            Self::Reopened => "reopened",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    Proposed,
    Contested,
    Confirmed,
    Withdrawn,
}

impl ClaimStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Contested => "contested",
            Self::Confirmed => "confirmed",
            Self::Withdrawn => "withdrawn",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    Proposed,
    Confirmed,
    Superseded,
    Revoked,
}

impl DecisionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Confirmed => "confirmed",
            Self::Superseded => "superseded",
            Self::Revoked => "revoked",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionStatus {
    Open,
    Answered,
    Dismissed,
}

impl QuestionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Answered => "answered",
            Self::Dismissed => "dismissed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadActorRef {
    pub account: SourceAccountRef,
    pub actor_id: String,
    /// 平台身份种类（身份命名空间）。仅事件派生的 Actor 引用（如 AgentEventView
    /// 的发送者）携带 Some；线程/记忆领域的引用多数不区分身份种类，为 None。
    /// Some 时 TempRefMap 可映射为完整账号作用域参与者引用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_identity_kind: Option<crate::PlatformIdentityKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventThread {
    pub thread_id: EventThreadId,
    pub account: SourceAccountRef,
    pub status: ThreadStatus,
    pub root_event_id: SourceEventId,
    pub latest_event_id: SourceEventId,
    pub opened_at_unix_secs: i64,
    pub updated_at_unix_secs: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadClaim {
    pub claim_id: ThreadClaimId,
    pub thread_id: EventThreadId,
    pub kind: crate::ClaimKind,
    pub claimant: ThreadActorRef,
    pub statement: String,
    pub status: ClaimStatus,
    pub confidence_bps: u16,
    pub source_event_ids: Vec<SourceEventId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadDecision {
    pub decision_id: ThreadDecisionId,
    pub thread_id: EventThreadId,
    pub statement: String,
    pub status: DecisionStatus,
    pub confidence_bps: u16,
    pub supersedes: Option<ThreadDecisionId>,
    pub source_event_ids: Vec<SourceEventId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenQuestion {
    pub question_id: OpenQuestionId,
    pub thread_id: EventThreadId,
    pub question: String,
    pub raised_by: ThreadActorRef,
    pub status: QuestionStatus,
    pub confidence_bps: u16,
    pub source_event_ids: Vec<SourceEventId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadRelationKind {
    Reply,
    SameConversationWindow,
    SameActorWithinConversationWindow,
    ExplicitProjectId,
    FileVersion,
}

impl ThreadRelationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reply => "reply",
            Self::SameConversationWindow => "same_conversation_window",
            Self::SameActorWithinConversationWindow => "same_actor_within_conversation_window",
            Self::ExplicitProjectId => "explicit_project_id",
            Self::FileVersion => "file_version",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRelation {
    pub relation_id: ThreadRelationId,
    pub thread_id: EventThreadId,
    pub from_event_id: SourceEventId,
    pub to_event_id: SourceEventId,
    pub kind: ThreadRelationKind,
    /// 万分制。当前自动落库的边均来自确定性证据，因此固定为 10000。
    pub confidence_bps: u16,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadContextEvent {
    pub source_event_id: SourceEventId,
    pub thread_id: EventThreadId,
    pub actor_id: String,
    pub occurred_at_unix_secs: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadProjectionEvent {
    pub source_event_id: SourceEventId,
    pub account: SourceAccountRef,
    pub conversation: ConversationRef,
    pub actor_id: String,
    pub occurred_at_unix_secs: i64,
    pub reply_to_event_id: Option<SourceEventId>,
    pub reply_parent_thread_id: Option<EventThreadId>,
    /// 父消息晚到时，已投影的回复子消息可把父消息拉回同一线程。
    pub reply_child_thread_id: Option<EventThreadId>,
    pub previous_in_conversation: Option<ThreadContextEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedThreadProjectionBatch {
    pub lease_token: ThreadProjectionLeaseToken,
    pub events: Vec<ThreadProjectionEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadAssignment {
    pub source_event_id: SourceEventId,
    pub thread_id: EventThreadId,
    pub account: SourceAccountRef,
    pub root_event_id: SourceEventId,
    pub occurred_at_unix_secs: i64,
    pub creates_thread: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadProjectionPlan {
    pub lease_token: ThreadProjectionLeaseToken,
    pub assignments: Vec<ThreadAssignment>,
    pub relations: Vec<ThreadRelation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeterministicThreadPolicy {
    pub same_conversation_window_secs: i64,
}

impl DeterministicThreadPolicy {
    pub fn new(same_conversation_window_secs: i64) -> Result<Self, ThreadingError> {
        if same_conversation_window_secs <= 0 {
            return Err(ThreadingError::InvalidData(
                "same_conversation_window_secs must be positive".into(),
            ));
        }
        Ok(Self {
            same_conversation_window_secs,
        })
    }
}

/// 纯领域规划器：不读数据库、不调用 LLM，只消费有界批次和确定性上下文。
pub struct DeterministicThreadPlanner {
    policy: DeterministicThreadPolicy,
}

impl DeterministicThreadPlanner {
    pub fn new(policy: DeterministicThreadPolicy) -> Self {
        Self { policy }
    }

    pub fn plan(
        &self,
        mut batch: ClaimedThreadProjectionBatch,
    ) -> Result<ThreadProjectionPlan, ThreadingError> {
        batch.events.sort_by(|left, right| {
            left.occurred_at_unix_secs
                .cmp(&right.occurred_at_unix_secs)
                .then_with(|| {
                    left.source_event_id
                        .as_str()
                        .cmp(right.source_event_id.as_str())
                })
        });
        batch.events = reply_parents_first(batch.events);

        let mut assigned: HashMap<String, EventThreadId> = HashMap::new();
        let mut latest: HashMap<String, ThreadContextEvent> = HashMap::new();
        let mut assignments = Vec::with_capacity(batch.events.len());
        let mut relations = Vec::new();

        for event in batch.events {
            let conversation_key = format!(
                "{}:{}:{}:{}",
                event.account.channel.as_str(),
                event.account.account_id,
                event.conversation.kind.as_str(),
                event.conversation.id
            );
            let reply_thread = event
                .reply_to_event_id
                .as_ref()
                .and_then(|id| assigned.get(id.as_str()).cloned())
                .or_else(|| event.reply_parent_thread_id.clone())
                .or_else(|| event.reply_child_thread_id.clone());
            let previous = latest
                .get(&conversation_key)
                .cloned()
                .or_else(|| event.previous_in_conversation.clone())
                .filter(|previous| {
                    event
                        .occurred_at_unix_secs
                        .saturating_sub(previous.occurred_at_unix_secs)
                        <= self.policy.same_conversation_window_secs
                });

            let (thread_id, creates_thread, root_event_id) = if let Some(thread_id) = reply_thread {
                let root = event
                    .reply_to_event_id
                    .clone()
                    .unwrap_or_else(|| event.source_event_id.clone());
                (thread_id, false, root)
            } else if let Some(previous) = &previous {
                (
                    previous.thread_id.clone(),
                    false,
                    previous.source_event_id.clone(),
                )
            } else {
                (
                    EventThreadId::generate(),
                    true,
                    event.source_event_id.clone(),
                )
            };

            if let Some(parent_id) = &event.reply_to_event_id {
                relations.push(ThreadRelation {
                    relation_id: ThreadRelationId::generate(),
                    thread_id: thread_id.clone(),
                    from_event_id: event.source_event_id.clone(),
                    to_event_id: parent_id.clone(),
                    kind: ThreadRelationKind::Reply,
                    confidence_bps: 10_000,
                    reason: "structured reply_to_event_id".into(),
                });
            } else if let Some(previous) = &previous {
                relations.push(ThreadRelation {
                    relation_id: ThreadRelationId::generate(),
                    thread_id: thread_id.clone(),
                    from_event_id: event.source_event_id.clone(),
                    to_event_id: previous.source_event_id.clone(),
                    kind: ThreadRelationKind::SameConversationWindow,
                    confidence_bps: 10_000,
                    reason: format!(
                        "same conversation within {} seconds",
                        self.policy.same_conversation_window_secs
                    ),
                });
                if previous.actor_id == event.actor_id {
                    relations.push(ThreadRelation {
                        relation_id: ThreadRelationId::generate(),
                        thread_id: thread_id.clone(),
                        from_event_id: event.source_event_id.clone(),
                        to_event_id: previous.source_event_id.clone(),
                        kind: ThreadRelationKind::SameActorWithinConversationWindow,
                        confidence_bps: 10_000,
                        reason: "same verified actor inside the same conversation window".into(),
                    });
                }
            }

            assigned.insert(event.source_event_id.as_str().to_owned(), thread_id.clone());
            latest.insert(
                conversation_key,
                ThreadContextEvent {
                    source_event_id: event.source_event_id.clone(),
                    thread_id: thread_id.clone(),
                    actor_id: event.actor_id.clone(),
                    occurred_at_unix_secs: event.occurred_at_unix_secs,
                },
            );
            assignments.push(ThreadAssignment {
                source_event_id: event.source_event_id,
                thread_id,
                account: event.account,
                root_event_id,
                occurred_at_unix_secs: event.occurred_at_unix_secs,
                creates_thread,
            });
        }

        Ok(ThreadProjectionPlan {
            lease_token: batch.lease_token,
            assignments,
            relations,
        })
    }
}

/// 在不改变无关事件时间顺序的前提下，把批次内 Reply 父事件移到子事件之前。
/// 历史源时间戳可能漂移，不能假设父消息时间戳一定更小；循环引用则保持原顺序并交给
/// 后续人工修正能力处理，不在这里无限循环。
fn reply_parents_first(mut events: Vec<ThreadProjectionEvent>) -> Vec<ThreadProjectionEvent> {
    let mut ordered = Vec::with_capacity(events.len());
    while !events.is_empty() {
        let next = events.iter().position(|event| {
            event.reply_to_event_id.as_ref().is_none_or(|parent| {
                !events
                    .iter()
                    .any(|candidate| candidate.source_event_id == *parent)
            })
        });
        ordered.push(events.remove(next.unwrap_or(0)));
    }
    ordered
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ThreadingError {
    #[error("invalid deterministic thread data: {0}")]
    InvalidData(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConversationKind, MessageSource};

    fn event(id: &str, conversation: &str, actor: &str, at: i64) -> ThreadProjectionEvent {
        ThreadProjectionEvent {
            source_event_id: SourceEventId::new(id).unwrap(),
            account: SourceAccountRef::new(MessageSource::NapCat, "account-1").unwrap(),
            conversation: ConversationRef::new(ConversationKind::Group, conversation).unwrap(),
            actor_id: actor.into(),
            occurred_at_unix_secs: at,
            reply_to_event_id: None,
            reply_parent_thread_id: None,
            reply_child_thread_id: None,
            previous_in_conversation: None,
        }
    }

    fn planner() -> DeterministicThreadPlanner {
        DeterministicThreadPlanner::new(DeterministicThreadPolicy::new(300).unwrap())
    }

    #[test]
    fn reply_chain_has_priority_and_never_calls_a_model() {
        let parent = event("parent", "group-1", "alice", 100);
        let mut child = event("child", "group-1", "bob", 400);
        child.reply_to_event_id = Some(parent.source_event_id.clone());
        let plan = planner()
            .plan(ClaimedThreadProjectionBatch {
                lease_token: ThreadProjectionLeaseToken::new("lease").unwrap(),
                events: vec![child, parent],
            })
            .unwrap();

        assert_eq!(plan.assignments.len(), 2);
        assert_eq!(plan.assignments[0].thread_id, plan.assignments[1].thread_id);
        assert!(
            plan.relations
                .iter()
                .any(|relation| relation.kind == ThreadRelationKind::Reply)
        );
    }

    #[test]
    fn reply_parent_is_planned_first_even_when_its_timestamp_is_later() {
        let parent = event("parent", "group-1", "alice", 500);
        let mut child = event("child", "group-1", "bob", 100);
        child.reply_to_event_id = Some(parent.source_event_id.clone());
        let plan = planner()
            .plan(ClaimedThreadProjectionBatch {
                lease_token: ThreadProjectionLeaseToken::new("lease").unwrap(),
                events: vec![child, parent],
            })
            .unwrap();

        assert_eq!(plan.assignments[0].source_event_id.as_str(), "parent");
        assert_eq!(plan.assignments[0].thread_id, plan.assignments[1].thread_id);
    }

    #[test]
    fn short_window_aggregates_inside_one_conversation_only() {
        let plan = planner()
            .plan(ClaimedThreadProjectionBatch {
                lease_token: ThreadProjectionLeaseToken::new("lease").unwrap(),
                events: vec![
                    event("one", "group-1", "alice", 100),
                    event("two", "group-1", "alice", 120),
                    event("three", "group-2", "alice", 121),
                ],
            })
            .unwrap();

        assert_eq!(plan.assignments[0].thread_id, plan.assignments[1].thread_id);
        assert_ne!(plan.assignments[1].thread_id, plan.assignments[2].thread_id);
        assert_eq!(
            plan.assignments
                .iter()
                .filter(|item| item.creates_thread)
                .count(),
            2
        );
    }

    #[test]
    fn expired_window_starts_a_new_thread() {
        let plan = planner()
            .plan(ClaimedThreadProjectionBatch {
                lease_token: ThreadProjectionLeaseToken::new("lease").unwrap(),
                events: vec![
                    event("one", "group-1", "alice", 100),
                    event("two", "group-1", "alice", 401),
                ],
            })
            .unwrap();
        assert_ne!(plan.assignments[0].thread_id, plan.assignments[1].thread_id);
    }
}
