//! 个人秘书 Agent 运行时：类型化动作白名单、有界工作状态、审批门与响应草稿。
//!
//! 子模块按职责拆分：动作白名单（[`action`]）、审批类型（[`approval`]）、
//! 响应草稿（[`response`]）、状态机（[`state`]）、有界校验与策略门（[`validation`]）。
//! 所有 public 类型通过本模块根重新导出，保持 `personal_secretary::*` 调用方稳定。

mod action;
mod approval;
mod response;
mod state;
mod validation;

pub use action::{
    FollowUpControlTarget, SecretaryAction, SecretaryActionEffect, SecretaryActionProposal,
    SecretaryActionReceipt, SecretaryRiskLevel, SecretaryToolKind, SecretaryToolPolicy,
};
pub use approval::{
    SecretaryActionApprovalRequest, SecretaryActionResumeInput, SecretaryApprovalDecision,
};
pub use response::{
    OwnerResponseDraft, RecentEventRef, ResponseSegment, build_action_response_draft,
};
pub use state::{SecretaryAgentPhase, SecretaryAgentState, SecretaryAgentUpdate};
pub use validation::{
    SecretaryAgentRuntimeError, gate_secretary_action, validate_action_proposal,
    validate_response_draft,
};

#[cfg(test)]
mod tests;
