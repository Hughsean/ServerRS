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
    /// 原始 OwnerCommand 保持运行幂等身份；审批消息仅作为恢复操作的审计证据。
    pub command_source_event_id: SourceEventId,
    pub approval_source_event_id: Option<SourceEventId>,
}
