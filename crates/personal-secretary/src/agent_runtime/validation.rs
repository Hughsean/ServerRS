//! 有界约束校验与错误类型。
//!
//! 所有验证函数集中在领域层；配置层只调用领域校验。
//! `bounded_text` 使用字符数（而非字节）限制，保证多字节内容稳定。

use std::collections::HashSet;

use thiserror::Error;

use crate::{NotificationOutcome, NotificationPolicyError, SourceEventId};

use super::action::{SecretaryAction, SecretaryActionProposal, SecretaryRiskLevel};
use super::response::{OwnerResponseDraft, ResponseSegment};
use super::state::SecretaryAgentState;

// 有界上下文常量。跨重启的序列化状态仍受这些上限约束。
pub(super) const MAX_GOAL_CHARS: usize = 2_000;
pub(super) const MAX_INVARIANTS: usize = 50;
pub(super) const MAX_EVIDENCE: usize = 100;
pub(super) const MAX_RECENT_EVENTS: usize = 8;
pub(super) const MAX_TEXT_CHARS: usize = 4_000;
pub(super) const MAX_RESPONSE_SEGMENTS: usize = 20;
pub(super) const MAX_SEGMENT_CHARS: usize = 1_000;
pub(super) const MAX_DRAFT_TOTAL_CHARS: usize = 8_000;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SecretaryAgentRuntimeError {
    #[error("invalid secretary action proposal: {0}")]
    InvalidProposal(String),
    #[error("invalid secretary agent state: {0}")]
    InvalidState(String),
    #[error("invalid owner response draft: {0}")]
    InvalidResponseDraft(String),
}

/// 策略门是唯一把模型 Proposal 转为 Effect/Suspend 的入口。
pub fn gate_secretary_action(
    proposal: SecretaryActionProposal,
) -> Result<
    NodeResult<
        super::state::SecretaryAgentUpdate,
        super::action::SecretaryActionEffect,
        super::approval::SecretaryActionApprovalRequest,
    >,
    SecretaryAgentRuntimeError,
> {
    validate_action_proposal(&proposal)?;
    let policy = proposal.action.kind().policy();
    let update = agent_core::AgentUpdate::Business(
        super::state::SecretaryAgentUpdate::ProposalAccepted(proposal.clone()),
    );

    if matches!(
        proposal.action,
        SecretaryAction::AskOwnerClarification { .. }
    ) {
        return Ok(NodeResult::suspend(
            vec![update],
            Vec::new(),
            UsageDelta::default(),
            SuspendRequest::new(
                SuspendReason::ExternalInput,
                approval_request(&proposal, policy),
            ),
        ));
    }
    if policy.requires_confirmation {
        return Ok(NodeResult::suspend(
            vec![update],
            Vec::new(),
            UsageDelta::default(),
            SuspendRequest::new(SuspendReason::Approval, approval_request(&proposal, policy)),
        ));
    }
    Ok(NodeResult::with_effect(
        vec![update],
        super::action::SecretaryActionEffect { proposal },
        UsageDelta::default(),
    ))
}

fn approval_request(
    proposal: &SecretaryActionProposal,
    policy: super::action::SecretaryToolPolicy,
) -> super::approval::SecretaryActionApprovalRequest {
    super::approval::SecretaryActionApprovalRequest {
        proposal_id: proposal.proposal_id.clone(),
        tool: proposal.action.kind(),
        risk: policy.risk,
        summary: proposal.rationale.clone(),
        source_event_ids: proposal.source_event_ids.clone(),
    }
}

pub fn validate_action_proposal(
    proposal: &SecretaryActionProposal,
) -> Result<(), SecretaryAgentRuntimeError> {
    if proposal.proposal_id.trim().is_empty() || proposal.proposal_id.len() > 191 {
        return Err(SecretaryAgentRuntimeError::InvalidProposal(
            "proposal_id must contain 1..=191 bytes".into(),
        ));
    }
    bounded_text("rationale", &proposal.rationale, 1, 1_000)?;
    if proposal.source_event_ids.len() > 20 {
        return Err(SecretaryAgentRuntimeError::InvalidProposal(
            "a proposal may reference at most 20 source events".into(),
        ));
    }
    let unique = proposal
        .source_event_ids
        .iter()
        .map(SourceEventId::as_str)
        .collect::<HashSet<_>>();
    if unique.len() != proposal.source_event_ids.len() {
        return Err(SecretaryAgentRuntimeError::InvalidProposal(
            "proposal source events must be unique".into(),
        ));
    }
    validate_action(&proposal.action)?;
    let policy = proposal.action.kind().policy();
    if policy.risk >= SecretaryRiskLevel::L2Impactful
        && proposal
            .idempotency_key
            .as_deref()
            .is_none_or(|key| key.trim().is_empty() || key.len() > 191)
    {
        return Err(SecretaryAgentRuntimeError::InvalidProposal(
            "L2/L3 actions require a bounded idempotency_key".into(),
        ));
    }
    Ok(())
}

fn validate_action(action: &SecretaryAction) -> Result<(), SecretaryAgentRuntimeError> {
    match action {
        SecretaryAction::SearchRecentEvents { query, limit }
        | SecretaryAction::SearchEventThreads { query, limit } => {
            bounded_text("query", query, 1, 1_000)?;
            if !(1..=100).contains(limit) {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "search limit must be in 1..=100".into(),
                ));
            }
        }
        SecretaryAction::ResolveReference { expression } => {
            bounded_text("expression", expression, 1, 1_000)?;
        }
        SecretaryAction::ListUpcomingItems { horizon_secs } if *horizon_secs == 0 => {
            return Err(SecretaryAgentRuntimeError::InvalidProposal(
                "horizon_secs must be positive".into(),
            ));
        }
        SecretaryAction::DraftReminder { text, .. }
        | SecretaryAction::CreateReminder { text, .. }
        | SecretaryAction::SendOwnerMessage { text } => {
            bounded_text("text", text, 1, MAX_TEXT_CHARS)?;
        }
        SecretaryAction::CreateSchedule { title, .. }
        | SecretaryAction::CreateTask { title, .. } => {
            bounded_text("title", title, 1, 500)?;
        }
        SecretaryAction::RescheduleItem { item_id, .. }
        | SecretaryAction::CompleteItem { item_id, .. }
        | SecretaryAction::SnoozeItem { item_id, .. } => {
            bounded_text("item_id", item_id, 1, 191)?;
        }
        SecretaryAction::CancelItem {
            item_id, reason, ..
        } => {
            bounded_text("item_id", item_id, 1, 191)?;
            bounded_text("reason", reason, 1, 1_000)?;
        }
        SecretaryAction::AskOwnerClarification { question } => {
            bounded_text("question", question, 1, 1_000)?;
        }
        SecretaryAction::ReadSourceEvent { .. }
        | SecretaryAction::ListUpcomingItems { .. }
        | SecretaryAction::GetSecretaryStatus
        | SecretaryAction::GetThreadContext { .. } => {}
        SecretaryAction::ListPendingOwnerWork { limit } => {
            if !(1..=20).contains(limit) {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "pending owner work limit must be in 1..=20".into(),
                ));
            }
        }
        SecretaryAction::ListNotificationPolicies { limit } => {
            if !(1..=20).contains(limit) {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "notification policy list limit must be in 1..=20".into(),
                ));
            }
        }
        SecretaryAction::ExplainNotificationDecision { decision_id } => {
            bounded_text("decision_id", decision_id, 1, 191)?;
        }
        SecretaryAction::SetAccountDefaultNotificationMode {
            canonical_scope_key,
            match_key,
            outcome,
            ..
        }
        | SecretaryAction::SetImportantContact {
            canonical_scope_key,
            match_key,
            outcome,
            ..
        }
        | SecretaryAction::SetNotificationCategoryImportance {
            canonical_scope_key,
            match_key,
            outcome,
            ..
        }
        | SecretaryAction::CreateSimilarNotificationRule {
            canonical_scope_key,
            match_key,
            outcome,
            ..
        } => {
            validate_policy_scope(canonical_scope_key, match_key)?;
            validate_policy_outcome(*outcome)?;
        }
        SecretaryAction::SetConversationNotificationMode {
            canonical_scope_key,
            match_key,
            outcome,
            ..
        } => {
            validate_policy_scope(canonical_scope_key, match_key)?;
            validate_policy_outcome(*outcome)?;
        }
        SecretaryAction::SetQuietHours {
            canonical_scope_key,
            match_key,
            quiet_hours,
        } => {
            validate_policy_scope(canonical_scope_key, match_key)?;
            validate_quiet_hours_shape(quiet_hours)?;
        }
        SecretaryAction::RecordNotificationFeedback {
            candidate,
            match_key,
            promote_to_rule,
            ..
        } => {
            if candidate.account != match_key.account {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "notification feedback candidate and match key accounts must agree".into(),
                ));
            }
            if *promote_to_rule {
                match_key
                    .eligibility_for_long_term_rule()
                    .map_err(|error| {
                        SecretaryAgentRuntimeError::InvalidProposal(match error {
                            NotificationPolicyError::UnknownMatchMetadata => {
                                "similar notification rule requires known or absent metadata".into()
                            }
                            _ => "invalid similar notification match key".into(),
                        })
                    })?;
            }
        }
        SecretaryAction::DisableNotificationPolicy {
            expected_generation,
            ..
        } if *expected_generation == 0 => {
            return Err(SecretaryAgentRuntimeError::InvalidProposal(
                "policy expected_generation must be positive".into(),
            ));
        }
        SecretaryAction::DisableNotificationPolicy { .. } => {}
        SecretaryAction::SetAutomaticReplyDeniedForContact {
            canonical_scope_key,
            match_key,
        } => {
            validate_policy_scope(canonical_scope_key, match_key)?;
            if !matches!(match_key.actor_id, crate::MatchField::Known(_)) {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "automatic reply denial requires a known actor identity".into(),
                ));
            }
        }
        SecretaryAction::ListMemoryFacts { limit } => {
            if !(1..=20).contains(limit) {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "memory fact list limit must be in 1..=20".into(),
                ));
            }
        }
        SecretaryAction::ReadMemoryFactSources {
            max_excerpt_chars, ..
        } => {
            if !(1..=1000).contains(max_excerpt_chars) {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "memory source excerpt limit must be in 1..=1000".into(),
                ));
            }
        }
        SecretaryAction::CorrectMemoryFact {
            confidence_bps,
            source_event_ids,
            valid_until_unix_secs,
            ..
        } => {
            if *confidence_bps > 10_000
                || source_event_ids.is_empty()
                || source_event_ids.len() > 20
            {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "memory correction requires confidence <= 10000 and 1..=20 sources".into(),
                ));
            }
            if valid_until_unix_secs.is_some_and(|value| value <= 0) {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "memory valid_until_unix_secs must be positive".into(),
                ));
            }
        }
        SecretaryAction::DeleteMemoryFact { reason, .. } => {
            bounded_text("memory delete reason", reason, 1, 1_000)?;
        }
        SecretaryAction::SetMemoryFactTtl {
            valid_until_unix_secs,
            ..
        } => {
            if valid_until_unix_secs.is_some_and(|value| value <= 0) {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "memory valid_until_unix_secs must be positive".into(),
                ));
            }
        }
        SecretaryAction::SetConversationMemoryMode { conversation, .. } => {
            bounded_text("conversation.id", &conversation.id, 1, 191)?;
        }
        SecretaryAction::ConfirmThreadDecision { .. } => {}
        SecretaryAction::RevokeThreadDecision { reason, .. }
        | SecretaryAction::DismissThreadQuestion { reason, .. } => {
            bounded_text("thread control reason", reason, 1, 1_000)?;
        }
        SecretaryAction::SetThreadLifecycle {
            expected_status,
            target_status,
            reason,
            ..
        } => {
            bounded_text("thread lifecycle reason", reason, 1, 1_000)?;
            if expected_status == target_status
                || !matches!(
                    target_status,
                    crate::ThreadStatus::Closed | crate::ThreadStatus::Reopened
                )
            {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "thread lifecycle target must be closed or reopened and differ from expected"
                        .into(),
                ));
            }
        }
        SecretaryAction::DismissFollowUp {
            follow_up_id,
            expected_source_version,
            reason,
        } => {
            // serde 直通可能绕过构造校验，这里在提案边界再次约束 ID 有界。
            bounded_text("follow_up_id", follow_up_id.as_str(), 1, 36)?;
            if *expected_source_version == 0 {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "follow_up expected_source_version must be positive".into(),
                ));
            }
            bounded_text("follow up reason", reason, 1, 1_000)?;
        }
        SecretaryAction::SnoozeFollowUp {
            follow_up_id,
            expected_source_version,
            snooze_until_unix_secs,
            reason,
        } => {
            bounded_text("follow_up_id", follow_up_id.as_str(), 1, 36)?;
            if *expected_source_version == 0 {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "follow_up expected_source_version must be positive".into(),
                ));
            }
            // 是否“未来”以执行时数据库 UTC 时间为准；这里只拒绝明显非法的时间戳。
            if *snooze_until_unix_secs <= 0 {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "follow_up snooze_until_unix_secs must be positive".into(),
                ));
            }
            bounded_text("follow up snooze reason", reason, 1, 1_000)?;
        }
        SecretaryAction::DismissFollowUps { targets, reason }
        | SecretaryAction::SnoozeFollowUps {
            targets, reason, ..
        } => {
            if targets.is_empty() || targets.len() > 20 {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "follow_up batch targets must contain 1..=20 items".into(),
                ));
            }
            // 同一批次禁止重复 FollowUp ID，重复必须在进入数据库前拒绝。
            let mut seen = HashSet::new();
            for target in targets {
                bounded_text("follow_up_id", target.follow_up_id.as_str(), 1, 36)?;
                if target.expected_source_version == 0 {
                    return Err(SecretaryAgentRuntimeError::InvalidProposal(
                        "follow_up expected_source_version must be positive".into(),
                    ));
                }
                if !seen.insert(target.follow_up_id.as_str()) {
                    return Err(SecretaryAgentRuntimeError::InvalidProposal(
                        "follow_up batch targets must not repeat follow_up_id".into(),
                    ));
                }
            }
            bounded_text("follow up reason", reason, 1, 1_000)?;
            if let SecretaryAction::SnoozeFollowUps {
                snooze_until_unix_secs,
                ..
            } = action
                && *snooze_until_unix_secs <= 0
            {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "follow_up snooze_until_unix_secs must be positive".into(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_policy_scope(
    canonical_scope_key: &str,
    match_key: &crate::NotificationMatchKeyV1,
) -> Result<(), SecretaryAgentRuntimeError> {
    if canonical_scope_key.trim().is_empty()
        || canonical_scope_key.len() > crate::MAX_CANONICAL_SCOPE_KEY_BYTES
    {
        return Err(SecretaryAgentRuntimeError::InvalidProposal(
            "canonical_scope_key must contain 1..=512 bytes".into(),
        ));
    }
    match_key
        .validate()
        .map_err(|_| SecretaryAgentRuntimeError::InvalidProposal("invalid policy match key".into()))
}

fn validate_policy_outcome(outcome: NotificationOutcome) -> Result<(), SecretaryAgentRuntimeError> {
    if !matches!(
        outcome,
        NotificationOutcome::Remind | NotificationOutcome::Delay | NotificationOutcome::Suppress
    ) {
        return Err(SecretaryAgentRuntimeError::InvalidProposal(
            "policy outcome must be remind, delay, or suppress".into(),
        ));
    }
    Ok(())
}

fn validate_quiet_hours_shape(
    rule: &crate::QuietHoursRule,
) -> Result<(), SecretaryAgentRuntimeError> {
    if rule.timezone_name.trim().is_empty()
        || rule.timezone_name.len() > 128
        || rule.start_local_time.len() != 5
        || rule.end_local_time.len() != 5
        || rule
            .effective_from_local_date
            .as_deref()
            .is_some_and(|date| date.len() != 10)
        || rule
            .effective_until_local_date
            .as_deref()
            .is_some_and(|date| date.len() != 10)
    {
        return Err(SecretaryAgentRuntimeError::InvalidProposal(
            "quiet-hours fields must use bounded IANA timezone and local date/time shapes".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_agent_state(
    state: &SecretaryAgentState,
) -> Result<(), SecretaryAgentRuntimeError> {
    bounded_text("goal", state.goal(), 1, MAX_GOAL_CHARS)?;
    if state.invariants().len() > MAX_INVARIANTS
        || state.evidence_source_event_ids().len() > MAX_EVIDENCE
        || state.recent_events().len() > MAX_RECENT_EVENTS
    {
        return Err(SecretaryAgentRuntimeError::InvalidState(
            "agent state exceeds bounded context limits".into(),
        ));
    }
    for invariant in state.invariants() {
        bounded_text("invariant", invariant, 1, 1_000)?;
    }
    for recent in state.recent_events() {
        bounded_text("recent event summary", &recent.summary, 1, 500)?;
    }
    Ok(())
}

/// 校验 Owner 响应草稿的有界约束（约束 7）。
///
/// - segments 数量上限 `MAX_RESPONSE_SEGMENTS`
/// - 每条片段正文上限 `MAX_SEGMENT_CHARS`
/// - 全部片段总字符数上限 `MAX_DRAFT_TOTAL_CHARS`
/// - source_event_ids 数量上限 `MAX_EVIDENCE`，且必须唯一
/// - created_at_unix_secs 非负
///
/// 序列化字节数的 64KB 限制由应用层在持久化时验证。
pub fn validate_response_draft(
    draft: &OwnerResponseDraft,
) -> Result<(), SecretaryAgentRuntimeError> {
    if draft.segments().is_empty() {
        return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(
            "response draft must contain at least one segment".into(),
        ));
    }
    if draft.segments().len() > MAX_RESPONSE_SEGMENTS {
        return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(format!(
            "response draft must not exceed {MAX_RESPONSE_SEGMENTS} segments"
        )));
    }
    for segment in draft.segments() {
        let text = segment.text();
        let count = text.chars().count();
        if count > MAX_SEGMENT_CHARS {
            return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(format!(
                "segment text must not exceed {MAX_SEGMENT_CHARS} characters"
            )));
        }
        if let ResponseSegment::Excerpt { text, .. } = segment
            && text.is_empty()
        {
            // envelope_only 摘录允许空文本；但 Summary 必须非空。
            continue;
        }
        if segment.text().trim().is_empty() {
            return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(
                "segment text must not be blank".into(),
            ));
        }
    }
    let total = draft.total_char_count();
    if total > MAX_DRAFT_TOTAL_CHARS {
        return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(format!(
            "response draft total characters must not exceed {MAX_DRAFT_TOTAL_CHARS}"
        )));
    }
    if draft.source_event_ids().len() > MAX_EVIDENCE {
        return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(format!(
            "response draft source_event_ids must not exceed {MAX_EVIDENCE} items"
        )));
    }
    let unique = draft
        .source_event_ids()
        .iter()
        .map(SourceEventId::as_str)
        .collect::<HashSet<_>>();
    if unique.len() != draft.source_event_ids().len() {
        return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(
            "response draft source_event_ids must be unique".into(),
        ));
    }
    if draft.created_at_unix_secs() < 0 {
        return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(
            "created_at_unix_secs must not be negative".into(),
        ));
    }
    Ok(())
}

fn bounded_text(
    field: &str,
    value: &str,
    min: usize,
    max: usize,
) -> Result<(), SecretaryAgentRuntimeError> {
    let count = value.chars().count();
    if !(min..=max).contains(&count) {
        return Err(SecretaryAgentRuntimeError::InvalidProposal(format!(
            "{field} must contain {min}..={max} characters"
        )));
    }
    Ok(())
}

// re-export agent_core 图类型，避免 policy/gate 函数签名依赖重复完整路径。
use agent_core::graph::{NodeResult, SuspendReason, SuspendRequest, UsageDelta};
