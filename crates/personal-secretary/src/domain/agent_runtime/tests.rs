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
            since_unix_secs: None,
            until_unix_secs: None,
            conversation: None,
            thread_id: None,
            actor_id: None,
        },
        None,
    ))
    .unwrap();
    assert!(matches!(result, NodeResult::Continue { .. }));
    assert_eq!(result.effects().len(), 1);
}

#[test]
fn pending_thread_link_candidates_are_read_only_and_bounded() {
    let result = gate_secretary_action(proposal(
        SecretaryAction::ListThreadLinkCandidates { limit: 20 },
        None,
    ))
    .unwrap();
    assert!(matches!(result, NodeResult::Continue { .. }));
    assert_eq!(result.effects().len(), 1);

    let error = SecretaryActionProposal::new(
        SecretaryAction::ListThreadLinkCandidates { limit: 21 },
        "列出候选",
        Vec::new(),
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("limit"));
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
fn owner_work_close_actions_require_confirmation_and_are_not_advertised_as_reversible() {
    for kind in [
        SecretaryToolKind::CompleteFollowUp,
        SecretaryToolKind::CompleteFollowUps,
        SecretaryToolKind::DismissResponseExpectation,
        SecretaryToolKind::DismissResponseExpectations,
    ] {
        let policy = kind.policy();
        assert_eq!(policy.risk, SecretaryRiskLevel::L2Impactful);
        assert!(policy.requires_confirmation);
        assert!(!policy.reversible);
    }
}

#[test]
fn semantic_reconfirmation_is_bounded_l2_owner_control() {
    let action = SecretaryAction::ReconfirmThreadSemantics {
        thread_id: crate::EventThreadId::new("thread-1").unwrap(),
        reason: "Owner 已复核迁移后的线程语义".into(),
    };
    let policy = action.kind().policy();
    assert_eq!(policy.risk, SecretaryRiskLevel::L2Impactful);
    assert!(policy.requires_confirmation);
    assert!(!policy.reversible);
    assert_eq!(
        proposal(action, Some("reconfirm:thread-1")).action.kind(),
        SecretaryToolKind::ReconfirmThreadSemantics
    );

    let error = SecretaryActionProposal::new(
        SecretaryAction::ReconfirmThreadSemantics {
            thread_id: crate::EventThreadId::new("thread-1").unwrap(),
            reason: " ".into(),
        },
        "重新确认线程语义",
        vec![SourceEventId::new("event-1").unwrap()],
        Some("reconfirm:thread-1".into()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("thread control reason"));
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

// ===== 策略响应工件收口表驱动测试 =====

fn policy_receipt(kind: SecretaryToolKind, result_ref: &str) -> SecretaryActionReceipt {
    SecretaryActionReceipt {
        proposal_id: "test-proposal".into(),
        result_ref: result_ref.into(),
        tool_kind: Some(kind),
    }
}

fn list_artifact_json(scope: &str, status: &str, typed_reason: &str) -> String {
    serde_json::to_string(&vec![crate::NotificationPolicyResponseArtifact {
        scope: scope.into(),
        policy_family_id: None,
        policy_revision_id: None,
        decision_id: None,
        status: status.into(),
        priority: "conversation".into(),
        typed_reason: typed_reason.into(),
        audit_reference: "policy_family_head".into(),
    }])
    .unwrap()
}

fn single_artifact_json(
    scope: &str,
    status: &str,
    typed_reason: &str,
    audit_reference: &str,
) -> String {
    serde_json::to_string(&crate::NotificationPolicyResponseArtifact {
        scope: scope.into(),
        policy_family_id: None,
        policy_revision_id: None,
        decision_id: None,
        status: status.into(),
        priority: "conversation".into(),
        typed_reason: typed_reason.into(),
        audit_reference: audit_reference.into(),
    })
    .unwrap()
}

fn draft_text(receipt: &SecretaryActionReceipt) -> String {
    let draft = crate::build_action_response_draft(Some(receipt), vec![], 1_000).unwrap();
    draft
        .segments()
        .iter()
        .map(|s| s.text().to_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn build_action_response_draft_policy_table() {
    // Case 1: List JSON → 中文规则列表
    let result_ref = list_artifact_json("conversation:conv-1", "rule", "generation:3");
    let receipt = policy_receipt(SecretaryToolKind::ListNotificationPolicies, &result_ref);
    let text = draft_text(&receipt);
    assert!(
        text.contains("当前提醒规则"),
        "List 应展示规则列表标题，实际：{text}"
    );
    assert!(
        text.contains("会话范围"),
        "List 应展示 scope 中文标签，实际：{text}"
    );
    assert!(text.contains("规则"), "List 应翻译 status，实际：{text}");
    assert!(!text.contains('{'), "List 不应包含 JSON，实际：{text}");
    assert!(
        !text.contains("typed_reason"),
        "List 不应泄漏内部 JSON 字段名，实际：{text}"
    );

    // Case 2: Explain schedule_time_ambiguous → 正确中文翻译
    let result_ref = single_artifact_json(
        "conversation:conv-1",
        "suppress",
        "schedule_time_ambiguous",
        "audit-ref-1",
    );
    let receipt = policy_receipt(SecretaryToolKind::ExplainNotificationDecision, &result_ref);
    let text = draft_text(&receipt);
    assert!(
        text.contains("时区歧义"),
        "schedule_time_ambiguous 应正确翻译，实际：{text}"
    );
    assert!(
        !text.contains("数据库失败") && !text.contains("系统异常"),
        "schedule_time_ambiguous 不得翻译为系统故障，实际：{text}"
    );
    assert!(
        !text.contains("schedule_time_ambiguous"),
        "不应泄漏原始字段值，实际：{text}"
    );

    // Case 3: Mutation → 成功中文文案
    let result_ref = single_artifact_json(
        "conversation:conv-1",
        "rule",
        "policy_written",
        "generation:1",
    );
    let receipt = policy_receipt(
        SecretaryToolKind::SetAccountDefaultNotificationMode,
        &result_ref,
    );
    let text = draft_text(&receipt);
    assert!(
        text.contains("提醒规则已更新"),
        "Mutation 应返回确定文案，实际：{text}"
    );
    assert!(!text.contains('{'), "Mutation 不应泄漏 JSON，实际：{text}");

    // Case 4: 损坏 JSON → 安全降级，不回显原始数据
    let receipt = policy_receipt(
        SecretaryToolKind::ListNotificationPolicies,
        "{{{{broken json not parseable",
    );
    let text = draft_text(&receipt);
    assert!(
        text.contains("无法安全展示"),
        "损坏 JSON 应降级，实际：{text}"
    );
    assert!(
        !text.contains("{{{{"),
        "损坏 JSON 的原始内容不得回显，实际：{text}"
    );

    // Case 5: 长列表 → 安全截断
    let many: Vec<crate::NotificationPolicyResponseArtifact> = (0..50)
        .map(|i| crate::NotificationPolicyResponseArtifact {
            scope: format!("conversation:conv-{i}"),
            policy_family_id: None,
            policy_revision_id: None,
            decision_id: None,
            status: "rule".into(),
            priority: "conversation".into(),
            typed_reason: format!("generation:{}", i + 1),
            audit_reference: "policy_family_head".into(),
        })
        .collect();
    let result_ref = serde_json::to_string(&many).unwrap();
    let receipt = policy_receipt(SecretaryToolKind::ListNotificationPolicies, &result_ref);
    let text = draft_text(&receipt);
    assert!(
        text.contains("其余规则已省略"),
        "长列表应截断，实际：{text}"
    );
    let draft = crate::build_action_response_draft(Some(&receipt), vec![], 1_000).unwrap();
    let text: String = draft
        .segments()
        .iter()
        .map(|s| s.text().to_owned())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.chars().count() <= 1_000,
        "截断后单段不应超过 1000 字符，实际 {} 字符",
        text.chars().count()
    );
    let serialized = serde_json::to_vec(&draft).unwrap();
    assert!(
        serialized.len() <= 8192,
        "序列化草稿不应超过 8 KiB，实际 {} bytes",
        serialized.len()
    );

    // Case 6: 敏感哨兵不得泄漏到响应中（注入在无法解析的原始 JSON 中）
    // 验证：即使 result_ref 原始字符串包含哨兵，解析失败后也走降级，不回显
    let bad_json = r#"{"scope":"合法范围","SECRET_MARKER":"OPENID_MARKER","status":"rule","MESSAGE_BODY_MARKER":true}"#;
    for kind in &[
        SecretaryToolKind::ListNotificationPolicies,
        SecretaryToolKind::ExplainNotificationDecision,
        SecretaryToolKind::SetAccountDefaultNotificationMode,
    ] {
        let receipt = policy_receipt(*kind, bad_json);
        let text = draft_text(&receipt);
        for sentinel in ["SECRET_MARKER", "OPENID_MARKER", "MESSAGE_BODY_MARKER"] {
            assert!(
                !text.contains(sentinel),
                "敏感哨兵 {sentinel} 不得出现在 {kind:?} 的降级响应中，实际：{text}"
            );
        }
        assert!(
            text.contains("无法安全展示"),
            "{kind:?} 损坏 JSON 应走降级路径，实际：{text}"
        );
    }

    // Case 7: 非策略 Action 保持兼容（原有 result_ref 行为）
    let receipt = SecretaryActionReceipt {
        proposal_id: "test-proposal".into(),
        result_ref: "命中 3 条事件".into(),
        tool_kind: Some(SecretaryToolKind::SearchRecentEvents),
    };
    let text = draft_text(&receipt);
    assert!(
        text.contains("命中 3 条事件"),
        "非策略 Action 应保持原有 result_ref 语义，实际：{text}"
    );
    assert!(
        text.contains("动作已执行"),
        "非策略 Action 应使用传统前缀，实际：{text}"
    );
}
