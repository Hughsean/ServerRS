//! Agent 运行时单元测试：动作策略门、有界状态、响应草稿校验与失效。

use super::validation::{
    MAX_DRAFT_TOTAL_CHARS, MAX_EVIDENCE, MAX_RECENT_EVENTS, MAX_RESPONSE_SEGMENTS,
    MAX_SEGMENT_CHARS,
};
use super::*;
use agent_core::graph::{NodeResult, SuspendReason};
use agent_core::{AgentBusinessState, AgentStateError};

use crate::SourceEventId;

fn proposal(action: SecretaryAction, key: Option<&str>) -> SecretaryActionProposal {
    SecretaryActionProposal::new(
        action,
        "根据已确认事件执行",
        vec![SourceEventId::new("event-1").unwrap()],
        key.map(str::to_owned),
    )
    .unwrap()
}

#[test]
fn read_only_action_executes_without_suspension() {
    let result = gate_secretary_action(proposal(
        SecretaryAction::SearchRecentEvents {
            query: "老板今天找过我吗".into(),
            limit: 20,
        },
        None,
    ))
    .unwrap();
    assert!(matches!(result, NodeResult::Continue { .. }));
    assert_eq!(result.effects().len(), 1);
}

#[test]
fn external_side_effect_always_suspends_without_effect() {
    let result = gate_secretary_action(proposal(
        SecretaryAction::SendOwnerMessage {
            text: "报价单还有两小时截止，是否提醒负责人？".into(),
        },
        Some("notify:quote:deadline"),
    ))
    .unwrap();
    match result {
        NodeResult::Suspend {
            request, effects, ..
        } => {
            assert_eq!(request.reason, SuspendReason::Approval);
            assert_eq!(request.data.risk, SecretaryRiskLevel::L3ExternalSideEffect);
            assert!(effects.is_empty());
        }
        NodeResult::Continue { .. } => panic!("L3 action must suspend"),
    }
}

#[test]
fn impactful_action_without_idempotency_key_is_rejected() {
    let error = SecretaryActionProposal::new(
        SecretaryAction::CreateReminder {
            text: "提交报价单".into(),
            due_at_unix: 1_800_000_000,
            timezone: "Asia/Shanghai".into(),
        },
        "用户要求创建提醒",
        Vec::new(),
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("idempotency_key"));
}

#[test]
fn working_state_rejects_unbounded_recent_window() {
    let events = (0..=MAX_RECENT_EVENTS)
        .map(|index| RecentEventRef {
            source_event_id: SourceEventId::new(format!("event-{index}")).unwrap(),
            summary: "精确来源摘要".into(),
        })
        .collect();
    let error = SecretaryAgentState::new("处理日程", Vec::new(), Vec::new(), events).unwrap_err();
    assert!(matches!(error, SecretaryAgentRuntimeError::InvalidState(_)));
}

#[test]
fn tool_surface_has_no_arbitrary_sql_http_shell_or_filesystem_action() {
    let serialized = serde_json::to_string(&proposal(
        SecretaryAction::ListUpcomingItems {
            horizon_secs: 86_400,
        },
        None,
    ))
    .unwrap();
    for forbidden in ["sql", "http", "shell", "filesystem", "napcat_send"] {
        assert!(!serialized.contains(forbidden));
    }
}

fn excerpt_segment(event_id: &str, text: &str) -> ResponseSegment {
    ResponseSegment::Excerpt {
        source_event_id: SourceEventId::new(event_id).unwrap(),
        text: text.into(),
    }
}

fn summary_segment(text: impl Into<String>) -> ResponseSegment {
    ResponseSegment::Summary { text: text.into() }
}

fn draft(segments: Vec<ResponseSegment>, source_event_ids: Vec<&str>) -> OwnerResponseDraft {
    OwnerResponseDraft::new(
        segments,
        source_event_ids
            .into_iter()
            .map(|id| SourceEventId::new(id).unwrap())
            .collect(),
        1_000,
    )
    .unwrap()
}

#[test]
fn response_draft_accepts_bounded_segments() {
    let d = draft(
        vec![
            excerpt_segment("event-1", "老板说：明天开会"),
            summary_segment("建议明天 10 点提醒"),
        ],
        vec!["event-1"],
    );
    assert_eq!(d.segments().len(), 2);
    assert!(!d.invalidated());
}

#[test]
fn response_draft_rejects_empty_segments() {
    let error = OwnerResponseDraft::new(Vec::new(), Vec::new(), 1_000).unwrap_err();
    assert!(matches!(
        error,
        SecretaryAgentRuntimeError::InvalidResponseDraft(_)
    ));
}

#[test]
fn response_draft_rejects_too_many_segments() {
    let segments: Vec<ResponseSegment> = (0..=MAX_RESPONSE_SEGMENTS)
        .map(|i| summary_segment(format!("段 {i}")))
        .collect();
    let error = OwnerResponseDraft::new(segments, Vec::new(), 1_000).unwrap_err();
    assert!(matches!(
        error,
        SecretaryAgentRuntimeError::InvalidResponseDraft(_)
    ));
}

#[test]
fn response_draft_rejects_oversized_segment() {
    let segment = summary_segment("x".repeat(MAX_SEGMENT_CHARS + 1));
    let error = OwnerResponseDraft::new(vec![segment], Vec::new(), 1_000).unwrap_err();
    assert!(matches!(
        error,
        SecretaryAgentRuntimeError::InvalidResponseDraft(_)
    ));
}

#[test]
fn response_draft_rejects_blank_summary() {
    let segment = summary_segment("   ");
    let error = OwnerResponseDraft::new(vec![segment], Vec::new(), 1_000).unwrap_err();
    assert!(matches!(
        error,
        SecretaryAgentRuntimeError::InvalidResponseDraft(_)
    ));
}

#[test]
fn response_draft_allows_empty_excerpt_text_for_envelope_only() {
    let segment = excerpt_segment("event-1", "");
    let d = OwnerResponseDraft::new(vec![segment], vec![], 1_000).unwrap();
    assert_eq!(d.segments().len(), 1);
}

#[test]
fn response_draft_rejects_total_chars_exceeding_limit() {
    // 每条 MAX_SEGMENT_CHARS，总字符数超过 MAX_DRAFT_TOTAL_CHARS
    let count = (MAX_DRAFT_TOTAL_CHARS / MAX_SEGMENT_CHARS) + 2;
    let segments: Vec<ResponseSegment> = (0..count)
        .map(|_| summary_segment("x".repeat(MAX_SEGMENT_CHARS)))
        .collect();
    let error = OwnerResponseDraft::new(segments, Vec::new(), 1_000).unwrap_err();
    assert!(matches!(
        error,
        SecretaryAgentRuntimeError::InvalidResponseDraft(_)
    ));
}

#[test]
fn response_draft_rejects_negative_created_at() {
    let segment = summary_segment("有效摘要");
    let error = OwnerResponseDraft::new(vec![segment], Vec::new(), -1).unwrap_err();
    assert!(matches!(
        error,
        SecretaryAgentRuntimeError::InvalidResponseDraft(_)
    ));
}

#[test]
fn response_draft_rejects_duplicate_source_event_ids() {
    let segment = excerpt_segment("event-1", "内容");
    let error = OwnerResponseDraft::new(
        vec![segment],
        vec![
            SourceEventId::new("event-1").unwrap(),
            SourceEventId::new("event-1").unwrap(),
        ],
        1_000,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        SecretaryAgentRuntimeError::InvalidResponseDraft(_)
    ));
}

#[test]
fn response_draft_rejects_too_many_source_event_ids() {
    let segment = summary_segment("摘要");
    let ids: Vec<SourceEventId> = (0..=MAX_EVIDENCE)
        .map(|i| SourceEventId::new(format!("event-{i}")).unwrap())
        .collect();
    let error = OwnerResponseDraft::new(vec![segment], ids, 1_000).unwrap_err();
    assert!(matches!(
        error,
        SecretaryAgentRuntimeError::InvalidResponseDraft(_)
    ));
}

#[test]
fn invalidate_marks_draft_when_excerpt_source_removed() {
    let mut d = draft(
        vec![excerpt_segment("event-1", "老板说：明天开会")],
        vec!["event-1"],
    );
    assert!(d.invalidate_if_references(&[SourceEventId::new("event-1").unwrap()]));
    assert!(d.invalidated());
}

#[test]
fn invalidate_marks_draft_when_evidence_source_removed() {
    let mut d = draft(vec![summary_segment("摘要")], vec!["event-1", "event-2"]);
    // 摘要段未直接引用，但 source_event_ids 含 event-2
    assert!(d.invalidate_if_references(&[SourceEventId::new("event-2").unwrap()]));
    assert!(d.invalidated());
}

#[test]
fn invalidate_does_not_mark_draft_when_source_not_referenced() {
    let mut d = draft(vec![excerpt_segment("event-1", "内容")], vec!["event-1"]);
    assert!(!d.invalidate_if_references(&[SourceEventId::new("event-99").unwrap()]));
    assert!(!d.invalidated());
}

#[test]
fn invalidate_is_idempotent() {
    let mut d = draft(vec![excerpt_segment("event-1", "内容")], vec!["event-1"]);
    assert!(d.invalidate_if_references(&[SourceEventId::new("event-1").unwrap()]));
    // 已失效后再次调用返回 false
    assert!(!d.invalidate_if_references(&[SourceEventId::new("event-1").unwrap()]));
}

#[test]
fn response_ready_update_sets_phase_to_respond() {
    let mut state = SecretaryAgentState::new(
        "处理日程",
        Vec::new(),
        vec![SourceEventId::new("event-1").unwrap()],
        Vec::new(),
    )
    .unwrap();
    assert_eq!(state.phase(), SecretaryAgentPhase::Observe);

    let d = draft(vec![summary_segment("已处理")], vec!["event-1"]);
    state
        .apply_update(SecretaryAgentUpdate::ResponseReady(d.clone()))
        .unwrap();
    assert_eq!(state.phase(), SecretaryAgentPhase::Respond);
    assert_eq!(state.response_draft(), Some(&d));
}

#[test]
fn response_ready_update_rejects_invalid_draft() {
    let mut state =
        SecretaryAgentState::new("处理日程", Vec::new(), Vec::new(), Vec::new()).unwrap();
    // 通过反序列化绕过 `new` 校验，构造一个 segments 为空的非法 draft，
    // 验证 `apply_update` 仍会拒绝并保持 phase 不变。
    let invalid_draft: OwnerResponseDraft = serde_json::from_value(serde_json::json!({
        "segments": [],
        "source_event_ids": [],
        "created_at_unix_secs": 1000,
        "invalidated": false
    }))
    .unwrap();
    let error = state
        .apply_update(SecretaryAgentUpdate::ResponseReady(invalid_draft))
        .unwrap_err();
    assert!(matches!(error, AgentStateError::Business(_)));
    assert_eq!(state.phase(), SecretaryAgentPhase::Observe);
    assert!(state.response_draft().is_none());
}

#[test]
fn agent_state_serialization_remains_backward_compatible() {
    // 反序列化旧状态（无 response_draft 字段）应成功并默认为 None。
    let json = serde_json::json!({
        "goal": "处理日程",
        "phase": "observe",
        "invariants": [],
        "evidence_source_event_ids": [],
        "recent_events": [],
        "pending_proposal": null,
        "last_receipt": null
    });
    let state: SecretaryAgentState = serde_json::from_value(json).unwrap();
    assert!(state.response_draft().is_none());
}
