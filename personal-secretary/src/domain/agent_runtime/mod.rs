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
mod working_context;

pub use action::{
    FollowUpControlTarget, ResponseExpectationControlTarget, SecretaryAction,
    SecretaryActionEffect, SecretaryActionProposal, SecretaryActionReceipt, SecretaryRiskLevel,
    SecretaryToolKind, SecretaryToolPolicy,
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
pub use working_context::{
    AgentWorkingContextV1, MAX_WORKING_BYTES, MAX_WORKING_EVIDENCE_REFS,
    MAX_WORKING_OPEN_REFERENCES, MAX_WORKING_RESOLVED_CONVERSATIONS, MAX_WORKING_RESOLVED_FACTS,
    MAX_WORKING_RESOLVED_PARTICIPANTS, MAX_WORKING_RESOLVED_THREADS, MAX_WORKING_TEXT_CHARS,
    MemoryCandidateConflictContext, MemoryConflictReasonCode, OpenReference, OpenReferenceKind,
    RetrievalTriggerKind, WorkingContextError, WorkingContextProjection, WorkingContextUpdate,
    summarize_memory_payload, validate_working_context_projection,
};

#[cfg(test)]
mod tests;
