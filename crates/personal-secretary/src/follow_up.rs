use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{AgendaItemKind, CommitmentMemory, SourceAccountRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowUpStatus {
    Scheduled,
    Completed,
    Dismissed,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FollowUpScanReport {
    pub commitments_materialized: u64,
    pub items_reconciled: u64,
    /// 本次由仍有效的 FollowUp 来源新建的通知候选数。
    pub notification_candidates_created: u64,
    /// 本次由上述候选新建的 generation-1 求值请求数。
    pub notification_evaluation_requests_created: u64,
    pub memories_expired: u64,
    pub response_expectations_materialized: u64,
    pub response_expectations_resolved: u64,
    pub project_blockers_materialized: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NotificationId(String);

impl NotificationId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err("notification id must not be empty".into());
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NotificationLeaseToken(String);

impl NotificationLeaseToken {
    pub fn generate() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OwnerNotificationContent {
    FollowUp {
        commitment: CommitmentMemory,
    },
    Agenda {
        kind: AgendaItemKind,
        title: String,
    },
    ResponseExpectation {
        question_id: String,
        thread_id: String,
        raised_by_actor_id: String,
        question_excerpt: String,
    },
    ProjectBlocker {
        project_key: String,
        blockers: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedOwnerNotification {
    pub notification_id: NotificationId,
    pub lease_token: NotificationLeaseToken,
    pub managed_account: SourceAccountRef,
    pub content: OwnerNotificationContent,
    pub due_at_unix_secs: i64,
    pub attempt: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationFailureKind {
    Retryable,
    Permanent,
    UnknownCommit,
}
