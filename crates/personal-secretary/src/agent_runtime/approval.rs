//! 动作审批请求、审批决策与恢复输入。

use serde::{Deserialize, Serialize};

use crate::SourceEventId;

use super::action::{SecretaryRiskLevel, SecretaryToolKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretaryActionApprovalRequest {
    pub proposal_id: String,
    pub tool: SecretaryToolKind,
    pub risk: SecretaryRiskLevel,
    pub summary: String,
    pub source_event_ids: Vec<SourceEventId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretaryApprovalDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretaryActionResumeInput {
    pub proposal_id: String,
    pub decision: SecretaryApprovalDecision,
    pub command_source_event_id: SourceEventId,
}
