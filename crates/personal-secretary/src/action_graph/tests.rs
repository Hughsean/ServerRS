//! Action Graph 辅助函数、节点与路由的单元测试。

use super::*;
use agent_core::graph::{
    AgentNode, EffectEnvelope, EffectError, EffectErrorKind, EffectExecutor, GraphDefinition,
    GraphId, GraphPolicy, GraphRuntime, RouteKey, Router, RunBudget, TransitionRule,
};
use agent_core::{AgentBusinessState, AgentOutcome, AgentState, AgentUpdate};
use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::Mutex;
use std::time::Duration;

use crate::{
    ActionLeaseToken, ActionPlannerT, ActionRunContext, MessageSource, PlannerError, PlannerInput,
    PlannerOutput, QueryEffectResultV1, QueryEffectTypedEvent, RecentEventRef, SecretaryAction,
    SecretaryActionEffect, SecretaryActionProposal, SecretaryActionReceipt, SecretaryAgentUpdate,
    SecretaryToolKind, SourceAccountRef, SourceEventId, WorkingContextUpdate,
    is_replan_observation_tool,
};

#[test]
fn backoff_first_attempt_is_base() {
    assert_eq!(backoff_ms(1, 500, 10_000), 500);
}

#[test]
fn backoff_doubles_each_attempt() {
    assert_eq!(backoff_ms(2, 500, 10_000), 1000);
    assert_eq!(backoff_ms(3, 500, 10_000), 2000);
}

#[test]
fn backoff_capped_at_max() {
    assert_eq!(backoff_ms(10, 500, 10_000), 10_000);
}

#[test]
fn backoff_saturates_on_huge_attempt() {
    assert_eq!(backoff_ms(u32::MAX, 500, 10_000), 10_000);
}

#[test]
fn l0_readonly_is_direct_execute() {
    assert!(is_l0_direct_execute(SecretaryRiskLevel::L0ReadOnly));
}

#[test]
fn l1_reversible_is_direct_execute() {
    assert!(is_l0_direct_execute(SecretaryRiskLevel::L1Reversible));
}

#[test]
fn l2_impactful_not_direct_execute() {
    assert!(!is_l0_direct_execute(SecretaryRiskLevel::L2Impactful));
}

#[test]
fn l3_external_not_direct_execute() {
    assert!(!is_l0_direct_execute(
        SecretaryRiskLevel::L3ExternalSideEffect
    ));
}

#[test]
fn action_run_id_rejects_empty() {
    assert!(ActionRunId::new("").is_err());
    assert!(ActionRunId::new("  ").is_err());
}

#[test]
fn action_run_id_accepts_non_empty() {
    assert!(ActionRunId::new("run-1").is_ok());
}

#[test]
fn action_ids_reject_database_truncation() {
    assert!(ActionRunId::new("x".repeat(37)).is_err());
    assert!(ActionLeaseToken::new("x".repeat(37)).is_err());
}

#[test]
fn owner_command_run_id_is_stable_uuid_and_version_scoped() {
    let source = SourceEventId::new("550e8400-e29b-41d4-a716-446655440000").unwrap();
    let first = ActionRunId::for_owner_command(&source, "v1");
    let repeated = ActionRunId::for_owner_command(&source, "v1");
    let upgraded = ActionRunId::for_owner_command(&source, "v2");
    assert_eq!(first, repeated);
    assert_ne!(first, upgraded);
    assert_eq!(first.as_str().len(), 36);
    assert!(uuid::Uuid::parse_str(first.as_str()).is_ok());
}

#[test]
fn action_lease_token_generates_uuid() {
    let token = ActionLeaseToken::generate();
    assert!(!token.as_str().is_empty());
}

#[test]
fn invalid_data_maps_to_permanent_effect_error() {
    let error = ActionStoreError::InvalidData("test".into());
    assert_eq!(error.to_effect_error().kind(), EffectErrorKind::Permanent);
}

#[test]
fn lease_lost_maps_to_permanent_effect_error() {
    let error = ActionStoreError::LeaseLost;
    assert_eq!(error.to_effect_error().kind(), EffectErrorKind::Permanent);
}

#[test]
fn database_error_maps_to_unknown_commit() {
    let error = ActionStoreError::Database("connection lost".into());
    assert_eq!(
        error.to_effect_error().kind(),
        EffectErrorKind::UnknownCommit
    );
}

#[test]
fn unavailable_maps_to_unknown_commit() {
    let error = ActionStoreError::Unavailable;
    assert_eq!(
        error.to_effect_error().kind(),
        EffectErrorKind::UnknownCommit
    );
}

#[test]
fn unknown_commit_maps_to_unknown_commit() {
    let error = ActionStoreError::UnknownCommit("maybe committed".into());
    assert_eq!(
        error.to_effect_error().kind(),
        EffectErrorKind::UnknownCommit
    );
}

// ===== ReplanDecisionNode / ReplanRouter 测试 =====

fn test_context() -> agent_core::graph::RunContext {
    use std::time::Duration;
    agent_core::graph::RunContext::new(
        RunBudget::new(NonZeroU32::new(8).unwrap(), Duration::from_secs(30)),
        tokio_util::sync::CancellationToken::new(),
        Default::default(),
    )
}

/// 构造带 receipt 的状态（模拟 EffectExecutor 已执行完毕）。
fn state_with_receipt(
    result_ref: &str,
    tool_kind: SecretaryToolKind,
) -> AgentState<SecretaryAgentState> {
    // 根据 tool_kind 构造匹配的 Action，避免 ActionCompleted 校验 tool_kind 不匹配
    let action = match tool_kind {
        SecretaryToolKind::DraftReminder => crate::SecretaryAction::DraftReminder {
            text: "test".into(),
            due_at_unix: 1_800_000_000,
        },
        _ => crate::SecretaryAction::SearchRecentEvents {
            query: "test".into(),
            limit: 20,
            since_unix_secs: None,
            until_unix_secs: None,
            conversation: None,
            thread_id: None,
            actor_id: None,
        },
    };
    let proposal = crate::SecretaryActionProposal::new(
        action,
        "test",
        vec![SourceEventId::new("cmd-event").unwrap()],
        None,
    )
    .unwrap();
    let proposal_id = proposal.proposal_id.clone();
    let receipt = SecretaryActionReceipt {
        proposal_id,
        result_ref: result_ref.into(),
        tool_kind: Some(tool_kind),
    };
    let mut business = SecretaryAgentState::new(
        "test goal",
        Vec::new(),
        vec![SourceEventId::new("cmd-event").unwrap()],
        vec![RecentEventRef {
            source_event_id: SourceEventId::new("cmd-event").unwrap(),
            summary: "cmd".into(),
        }],
    )
    .unwrap();
    business
        .apply_update(SecretaryAgentUpdate::ProposalAccepted(proposal))
        .unwrap();
    business
        .apply_update(SecretaryAgentUpdate::ActionCompleted(receipt))
        .unwrap();
    AgentState::new(business)
}

#[tokio::test]
async fn replan_decision_parses_query_effect_and_returns_observation() {
    let query_result = QueryEffectResultV1 {
        version: 1,
        tool_kind: SecretaryToolKind::SearchRecentEvents,
        summary: "命中 3 条事件".into(),
        source_event_ids: vec![SourceEventId::new("event-1").unwrap()],
        event_count: 3,
        typed_events: vec![],
        ambiguous: false,
    };
    let result_ref = serde_json::to_string(&query_result).unwrap();
    let state = state_with_receipt(&result_ref, SecretaryToolKind::SearchRecentEvents);
    let node = ReplanDecisionNode::new().unwrap();
    let context = test_context();

    let result = node.execute(&state, &context).await.unwrap();
    let updates = result.into_updates();
    // CMD-009 目标 A：观察进入工作上下文（ObservationAppended + WorkingContext 更新）。
    assert_eq!(updates.len(), 2);
    match &updates[0] {
        AgentUpdate::Business(SecretaryAgentUpdate::ObservationAppended(obs)) => {
            assert_eq!(obs.tool_kind, SecretaryToolKind::SearchRecentEvents);
            assert!(obs.success);
            assert!(!obs.proposal_id.is_empty());
            assert!(obs.summary.contains("命中 3 条"));
        }
        other => panic!("expected ObservationAppended, got {other:?}"),
    }
    match &updates[1] {
        AgentUpdate::Business(SecretaryAgentUpdate::WorkingContext(
            WorkingContextUpdate::ReplanEvidence { evidence_refs, .. },
        )) => {
            assert_eq!(evidence_refs.len(), 1);
        }
        other => panic!("expected WorkingContext ReplanEvidence, got {other:?}"),
    }
}

#[tokio::test]
async fn replan_decision_skips_non_query_tool() {
    let result_ref = "已起草提醒：test";
    let state = state_with_receipt(result_ref, SecretaryToolKind::DraftReminder);
    let node = ReplanDecisionNode::new().unwrap();
    let context = test_context();

    let result = node.execute(&state, &context).await.unwrap();
    assert!(result.updates().is_empty());
}

#[tokio::test]
async fn replan_decision_skips_when_outcome_set() {
    let query_result = QueryEffectResultV1 {
        version: 1,
        tool_kind: SecretaryToolKind::SearchRecentEvents,
        summary: "命中 3 条".into(),
        source_event_ids: vec![],
        event_count: 3,
        typed_events: vec![],
        ambiguous: false,
    };
    let result_ref = serde_json::to_string(&query_result).unwrap();
    let mut state = state_with_receipt(&result_ref, SecretaryToolKind::SearchRecentEvents);
    let _ = state.apply_updates(vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
        "done".into(),
    ))]);
    let node = ReplanDecisionNode::new().unwrap();
    let context = test_context();

    let result = node.execute(&state, &context).await.unwrap();
    assert!(result.updates().is_empty());
}

#[tokio::test]
async fn replan_decision_skips_unparseable_result_ref() {
    let state = state_with_receipt(
        "命中 5 条: event-123 | actor-1 | excerpt...",
        SecretaryToolKind::SearchRecentEvents,
    );
    let node = ReplanDecisionNode::new().unwrap();
    let context = test_context();

    let result = node.execute(&state, &context).await.unwrap();
    assert!(result.updates().is_empty());
}

// ===== ReplanRouter 路由测试 =====

fn route(state: &AgentState<SecretaryAgentState>) -> String {
    ReplanRouter.select(state).unwrap().as_str().to_string()
}

#[test]
fn replan_router_finish_when_outcome_set() {
    let mut state = state_with_receipt("{}", SecretaryToolKind::SearchRecentEvents);
    let _ = state.apply_updates(vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
        "done".into(),
    ))]);
    assert_eq!(route(&state), "finish");
}

#[test]
fn replan_router_finish_when_no_receipt() {
    let business = SecretaryAgentState::new(
        "goal",
        vec![],
        vec![SourceEventId::new("evt").unwrap()],
        vec![],
    )
    .unwrap();
    assert_eq!(route(&AgentState::new(business)), "finish");
}

#[test]
fn replan_router_finish_when_non_query_tool() {
    let state = state_with_receipt("draft", SecretaryToolKind::DraftReminder);
    assert_eq!(route(&state), "finish");
}

#[test]
fn replan_router_continue_when_query_tool_and_has_observation() {
    let query_result = QueryEffectResultV1 {
        version: 1,
        tool_kind: SecretaryToolKind::SearchRecentEvents,
        summary: "命中 3 条".into(),
        source_event_ids: vec![],
        event_count: 3,
        typed_events: vec![],
        ambiguous: false,
    };
    let result_ref = serde_json::to_string(&query_result).unwrap();
    let mut state = state_with_receipt(&result_ref, SecretaryToolKind::SearchRecentEvents);
    // 用 receipt 中实际的 proposal_id 构造观察
    let proposal_id = state.business().last_receipt().unwrap().proposal_id.clone();
    let obs = query_result.to_observation(proposal_id, true);
    let _ = state.apply_updates(vec![AgentUpdate::Business(
        SecretaryAgentUpdate::ObservationAppended(obs),
    )]);
    assert_eq!(route(&state), "continue");
}

#[test]
fn replan_router_finish_when_budget_exhausted() {
    let query_result = QueryEffectResultV1 {
        version: 1,
        tool_kind: SecretaryToolKind::SearchRecentEvents,
        summary: "命中 1 条".into(),
        source_event_ids: vec![],
        event_count: 1,
        typed_events: vec![],
        ambiguous: false,
    };
    let result_ref = serde_json::to_string(&query_result).unwrap();
    let mut state = state_with_receipt(&result_ref, SecretaryToolKind::SearchRecentEvents);
    for i in 0..crate::planner::MAX_REPLAN_ROUNDS {
        let obs = query_result.to_observation(format!("proposal-{i}"), true);
        let _ = state.apply_updates(vec![AgentUpdate::Business(
            SecretaryAgentUpdate::ObservationAppended(obs),
        )]);
    }
    assert_eq!(
        state.business().replan_round(),
        crate::planner::MAX_REPLAN_ROUNDS
    );
    assert_eq!(route(&state), "finish");
}

#[test]
fn replan_router_continues_after_conflict_context_is_recorded() {
    let candidate_id = crate::MemoryCandidateId::generate();
    let fact_id = crate::MemoryFactId::new("conflict-fact-1").unwrap();
    let conflict_result = crate::MemoryCandidateConflictResultV1 {
        version: 1,
        candidate_id: candidate_id.clone(),
        fact_id: fact_id.clone(),
        reason_code: crate::MemoryConflictReasonCode::ActiveFactPayloadDiffers,
        summary: "候选与现行事实冲突".into(),
    };
    let proposal = crate::SecretaryActionProposal::new(
        crate::SecretaryAction::ApproveMemoryCandidate {
            candidate_id: candidate_id.clone(),
            expected_candidate_version: 1,
            reason: "批准候选".into(),
        },
        "批准候选",
        vec![SourceEventId::new("cmd-event").unwrap()],
        Some("approve-conflict-test".into()),
    )
    .unwrap();
    let mut business = SecretaryAgentState::new(
        "test goal",
        Vec::new(),
        vec![SourceEventId::new("cmd-event").unwrap()],
        Vec::new(),
    )
    .unwrap();
    let proposal_id = proposal.proposal_id.clone();
    business
        .apply_update(SecretaryAgentUpdate::ProposalAccepted(proposal))
        .unwrap();
    business
        .apply_update(SecretaryAgentUpdate::ActionCompleted(
            SecretaryActionReceipt {
                proposal_id,
                result_ref: serde_json::to_string(&conflict_result).unwrap(),
                tool_kind: Some(SecretaryToolKind::ApproveMemoryCandidate),
            },
        ))
        .unwrap();
    business
        .apply_update(SecretaryAgentUpdate::WorkingContext(
            WorkingContextUpdate::ConflictReRead(
                crate::MemoryCandidateConflictContext::valid(
                    candidate_id,
                    fact_id,
                    "project",
                    crate::MemoryConflictReasonCode::ActiveFactPayloadDiffers,
                    "候选与现行事实冲突",
                    vec![SourceEventId::new("source-1").unwrap()],
                    "现行项目事实",
                )
                .unwrap(),
            ),
        ))
        .unwrap();

    assert_eq!(route(&AgentState::new(business)), "continue");
}

// ===== is_replan_observation_tool 白名单测试 =====

#[test]
fn replan_allowed_tools_are_all_l0_readonly() {
    for kind in &[
        SecretaryToolKind::SearchRecentEvents,
        SecretaryToolKind::ReadSourceEvent,
        SecretaryToolKind::SearchEventThreads,
        SecretaryToolKind::ResolveReference,
        SecretaryToolKind::ListUpcomingItems,
        SecretaryToolKind::GetSecretaryStatus,
        SecretaryToolKind::ListPendingOwnerWork,
        SecretaryToolKind::ListMemoryCandidates,
    ] {
        assert!(
            is_replan_observation_tool(*kind),
            "{kind:?} should be replan-eligible"
        );
    }
    // GetThreadContext 的摘要仍含稳定 thread/actor/evidence ID，且当前没有
    // 类型化事件投影，不得进入 Replan 白名单。
    assert!(!is_replan_observation_tool(
        SecretaryToolKind::GetThreadContext
    ));
}

#[test]
fn non_query_tools_not_replan_eligible() {
    assert!(!is_replan_observation_tool(
        SecretaryToolKind::DraftReminder
    ));
    assert!(!is_replan_observation_tool(
        SecretaryToolKind::CreateReminder
    ));
    assert!(!is_replan_observation_tool(
        SecretaryToolKind::AskOwnerClarification
    ));
    assert!(!is_replan_observation_tool(
        SecretaryToolKind::DismissFollowUp
    ));
    assert!(!is_replan_observation_tool(
        SecretaryToolKind::ApproveMemoryCandidate
    ));
}

// ===== CTX-004-VERIFY：全 Graph Replan 闭环集成测试 =====

/// 记录每次 Planner 调用的输入，并按预设顺序返回响应。
struct RecordingPlanner {
    calls: Mutex<Vec<PlannerInput>>,
    outputs: Mutex<std::vec::IntoIter<PlannerOutput>>,
}

impl RecordingPlanner {
    fn new(outputs: Vec<PlannerOutput>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            outputs: Mutex::new(outputs.into_iter()),
        }
    }
}

#[async_trait::async_trait]
impl ActionPlannerT for RecordingPlanner {
    async fn plan(&self, input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        self.calls.lock().unwrap().push(input.clone());
        self.outputs
            .lock()
            .unwrap()
            .next()
            .ok_or_else(|| PlannerError::UnparseableOutput("no more outputs".into()))
    }
}

fn plan_node_test_context(command_event: &SourceEventId) -> std::sync::Arc<ActionRunContext> {
    std::sync::Arc::new(ActionRunContext {
        account: SourceAccountRef::new(MessageSource::NapCat, "test-account").unwrap(),
        command_source_event_id: command_event.clone(),
        command_text: "测试 OwnerCommand".into(),
        conversation_id: "conv-test".into(),
        occurred_at_unix_secs: 1_800_000_000,
        timezone_offset_secs: 28_800,
        timezone: "Asia/Shanghai".into(),
        now_unix_secs: 1_800_000_000,
        lease_token: ActionLeaseToken::generate(),
        is_local_loopback: false,
    })
}

fn plan_node_test_state(command_event: &SourceEventId) -> AgentState<SecretaryAgentState> {
    AgentState::new(
        SecretaryAgentState::new(
            "test goal",
            Vec::new(),
            vec![command_event.clone()],
            vec![RecentEventRef {
                source_event_id: command_event.clone(),
                summary: "OwnerCommand".into(),
            }],
        )
        .unwrap(),
    )
}

#[tokio::test]
async fn plan_node_rejects_write_without_authoritative_command_evidence() {
    let command_event = SourceEventId::new("cmd-domain-evidence").unwrap();
    let unrelated_event = SourceEventId::new("untrusted-chat-event").unwrap();
    let proposal = SecretaryActionProposal::new(
        SecretaryAction::DraftReminder {
            text: "提醒内容".into(),
            due_at_unix: 1_900_000_000,
        },
        "来自不可信聊天正文",
        vec![unrelated_event],
        None,
    )
    .unwrap();
    let planner = std::sync::Arc::new(RecordingPlanner::new(vec![PlannerOutput::Proposal(
        proposal,
    )]));
    let node = PlanNode::new(planner, None, plan_node_test_context(&command_event)).unwrap();

    let error = match node
        .execute(&plan_node_test_state(&command_event), &test_context())
        .await
    {
        Ok(_) => panic!("domain PlanNode must reject missing OwnerCommand evidence"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("缺少本轮 OwnerCommand 证据"));
}

#[tokio::test]
async fn plan_node_with_open_reference_allows_only_owner_clarification() {
    let command_event = SourceEventId::new("cmd-open-reference").unwrap();
    let proposal = SecretaryActionProposal::new(
        SecretaryAction::SearchRecentEvents {
            query: "继续猜那个人".into(),
            limit: 20,
            since_unix_secs: None,
            until_unix_secs: None,
            conversation: None,
            thread_id: None,
            actor_id: None,
        },
        "绕过澄清继续检索",
        vec![command_event.clone()],
        None,
    )
    .unwrap();
    let planner = std::sync::Arc::new(RecordingPlanner::new(vec![PlannerOutput::Proposal(
        proposal,
    )]));
    let node = PlanNode::new(planner, None, plan_node_test_context(&command_event)).unwrap();
    let mut business = SecretaryAgentState::new(
        "test goal",
        Vec::new(),
        vec![command_event.clone()],
        vec![RecentEventRef {
            source_event_id: command_event.clone(),
            summary: "OwnerCommand".into(),
        }],
    )
    .unwrap();
    business
        .apply_update(SecretaryAgentUpdate::WorkingContext(
            WorkingContextUpdate::ReplanEvidence {
                evidence_refs: Vec::new(),
                resolved_thread_refs: Vec::new(),
                resolved_participant_refs: Vec::new(),
                resolved_fact_refs: Vec::new(),
                open_references: vec![crate::OpenReference {
                    kind: crate::OpenReferenceKind::AmbiguousReference,
                    label: "未能唯一确定所指对象".into(),
                    source_event_ids: Vec::new(),
                    reason: "作用域内存在多个候选".into(),
                }],
            },
        ))
        .unwrap();
    let state = AgentState::new(business);

    let error = match node.execute(&state, &test_context()).await {
        Ok(_) => panic!("open reference must force owner clarification"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("只能请求 Owner 澄清"));
}

/// 返回预设 QueryEffectResultV1 receipt 的 Fake EffectExecutor。
/// 根据 envelope 中的 proposal_id 动态构造 receipt，保证 proposal_id 一致性。
struct FakeEffectExecutor;

#[async_trait::async_trait]
impl EffectExecutor<SecretaryActionEffect> for FakeEffectExecutor {
    async fn execute(
        &self,
        envelope: &EffectEnvelope<SecretaryActionEffect>,
        _context: &agent_core::graph::RunContext,
    ) -> Result<SecretaryActionReceipt, EffectError> {
        let tool_kind = envelope.effect.proposal.action.kind();
        let query_result = QueryEffectResultV1 {
            version: 1,
            tool_kind,
            summary: "命中 3 条事件".into(),
            source_event_ids: vec![SourceEventId::new("event-1").unwrap()],
            event_count: 3,
            typed_events: vec![QueryEffectTypedEvent {
                source_event_id: SourceEventId::new("event-1").unwrap(),
                actor_id: "actor-1".into(),
                actor_kind: crate::PlatformIdentityKind::External,
                occurred_at_unix_secs: 1_000,
                excerpt: "关于报价单的讨论".into(),
            }],
            ambiguous: false,
        };
        let result_ref = serde_json::to_string(&query_result).unwrap();
        Ok(SecretaryActionReceipt {
            proposal_id: envelope.effect.proposal.proposal_id.clone(),
            result_ref,
            tool_kind: Some(tool_kind),
        })
    }
}

/// CTX-004-VERIFY：全 Graph 主路径闭环测试。
///
/// 验证完整 Replan 循环：
/// - Round 1：Planner 返回 SearchRecentEvents → Effect 执行 → 观察注入
/// - Round 2：Planner 收到观察 → 返回 NoAction → BuildResponse
/// - 断言：Planner 调用 2 次、Effect 执行 1 次、响应 1 份。
#[tokio::test]
async fn replan_full_graph_two_rounds_search_then_no_action() {
    // 构造 ActionRunContext
    let account = SourceAccountRef::new(MessageSource::NapCat, "test-account").unwrap();
    let cmd_event_id = SourceEventId::new("cmd-event-001").unwrap();
    let context = std::sync::Arc::new(ActionRunContext {
        account: account.clone(),
        command_source_event_id: cmd_event_id.clone(),
        command_text: "帮我查报价单".into(),
        conversation_id: "conv-test".into(),
        occurred_at_unix_secs: 1_800_000_000,
        timezone_offset_secs: 28_800,
        timezone: "Asia/Shanghai".into(),
        now_unix_secs: 1_800_000_000,
        lease_token: ActionLeaseToken::generate(),
        is_local_loopback: false,
    });

    // Round 1：Planner 返回 SearchRecentEvents Proposal
    let round1 = PlannerOutput::Proposal(
        SecretaryActionProposal::new(
            SecretaryAction::SearchRecentEvents {
                query: "报价单".into(),
                limit: 20,
                since_unix_secs: None,
                until_unix_secs: None,
                conversation: None,
                thread_id: None,
                actor_id: None,
            },
            "Owner 要查报价单",
            vec![cmd_event_id.clone()],
            None,
        )
        .unwrap(),
    );

    // Round 2：Planner 返回 NoAction（收到观察后判定无需进一步操作）
    let round2 = PlannerOutput::NoAction {
        reason: "已查到报价单相关信息".into(),
    };

    let planner = std::sync::Arc::new(RecordingPlanner::new(vec![round1, round2]));
    let effect_executor = std::sync::Arc::new(FakeEffectExecutor);

    // 手动装配图（等价于 build_action_graph，但使用 Fake EffectExecutor）
    let mut graph = GraphDefinition::new(GraphId::try_from("test_replan").unwrap());
    let plan_node =
        std::sync::Arc::new(PlanNode::new(planner.clone(), None, context.clone()).unwrap());
    let l0_node = std::sync::Arc::new(L0ExecuteNode::new().unwrap());
    let replan_node = std::sync::Arc::new(ReplanDecisionNode::new().unwrap());
    let build_node = std::sync::Arc::new(BuildResponseNode::new(context.clone()).unwrap());

    graph.add_node(plan_node.clone()).unwrap();
    graph.add_node(l0_node.clone()).unwrap();
    graph.add_node(replan_node.clone()).unwrap();
    graph.add_node(build_node.clone()).unwrap();

    let plan_id = NodeId::try_from("plan").unwrap();
    let l0_id = NodeId::try_from("l0_execute").unwrap();
    let replan_id = NodeId::try_from("replan_decision").unwrap();
    let build_id = NodeId::try_from("build_response").unwrap();

    graph.set_entry(plan_id.clone());
    graph
        .set_transition(plan_id.clone(), TransitionRule::Goto(l0_id.clone()))
        .unwrap();
    graph
        .set_transition(l0_id.clone(), TransitionRule::Goto(replan_id.clone()))
        .unwrap();

    let mut replan_targets = BTreeMap::new();
    replan_targets.insert(RouteKey::try_from("continue").unwrap(), plan_id.clone());
    replan_targets.insert(RouteKey::try_from("finish").unwrap(), build_id.clone());
    graph
        .set_transition(
            replan_id.clone(),
            TransitionRule::Branch {
                router: std::sync::Arc::new(ReplanRouter),
                targets: replan_targets,
            },
        )
        .unwrap();
    graph
        .set_transition(build_id.clone(), TransitionRule::End)
        .unwrap();

    let compiled = graph
        .compile(GraphPolicy::new(NonZeroU32::new(16).unwrap()))
        .unwrap();
    let runtime = GraphRuntime::with_effect_executor(compiled, effect_executor);

    // 构建初始状态
    let business = SecretaryAgentState::new(
        "test goal",
        Vec::new(),
        vec![cmd_event_id.clone()],
        vec![RecentEventRef {
            source_event_id: cmd_event_id.clone(),
            summary: "cmd".into(),
        }],
    )
    .unwrap();
    let state = AgentState::new(business);

    // 运行图至完成
    let budget = RunBudget::new(NonZeroU32::new(16).unwrap(), Duration::from_secs(30));
    let result = runtime.run(state, budget).await.unwrap();

    // === 断言 ===

    // 1. Planner 恰好调用 2 次
    let calls = planner.calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "Planner should be called exactly twice");

    // 2. 恰好 1 条 Effect Receipt
    assert_eq!(
        result.effect_receipts.len(),
        1,
        "Exactly one effect receipt"
    );

    // 3. 状态包含 Outcome 和 ResponseReady
    let outcome = result.state.outcome().expect("Should have outcome");
    let outcome_text = outcome
        .response_text()
        .expect("Outcome should have response text");
    assert!(
        outcome_text.contains("已查到报价单"),
        "Outcome text should contain the reason, got: {outcome_text}"
    );

    let business = result.state.business();
    let response_draft = business
        .response_draft()
        .expect("Should have ResponseReady");
    assert!(
        !response_draft.segments().is_empty(),
        "Response should have segments"
    );

    // 4. Round 1 输入：replan_round=0，无观察
    let round1_input = &calls[0];
    assert_eq!(round1_input.replan_round, 0);
    assert!(
        round1_input.observations.is_empty(),
        "Round 1 should have no observations"
    );
    assert_eq!(
        round1_input.command.normalized_text, "帮我查报价单",
        "Command text preserved"
    );

    // 5. Round 2 输入：replan_round=1，包含 round 1 的工具观察
    let round2_input = &calls[1];
    assert_eq!(
        round2_input.replan_round, 1,
        "Round 2 replan_round should be 1"
    );
    assert_eq!(
        round2_input.observations.len(),
        1,
        "Round 2 should have 1 observation"
    );
    let obs = &round2_input.observations[0];
    assert_eq!(obs.tool_kind, SecretaryToolKind::SearchRecentEvents);
    assert!(obs.success);
    assert_eq!(obs.typed_events.len(), 1);
    assert_eq!(obs.typed_events[0].source_event_id.as_str(), "event-1");
    assert_eq!(obs.typed_events[0].actor_id, "actor-1");
    // 来源事件 ID 集合包含 typed event 的 event
    assert!(
        obs.source_event_ids
            .contains(&obs.typed_events[0].source_event_id)
    );

    // 6. 访问节点顺序校验：
    // plan → l0_execute → replan_decision → plan → l0_execute → replan_decision → build_response
    assert_eq!(
        result.visited.len(),
        7,
        "Should visit exactly 7 nodes, got {:?}",
        result.visited
    );

    // 7. 最终状态 replan_round 为 1（非 2，因为 NoAction 不推进轮次）
    assert_eq!(business.replan_round(), 1);
}

/// CTX-004-VERIFY：预算耗尽时 Router 正确终止 Replan。
///
/// 两轮查询后 Planner 返回 Proposal（而非 NoAction），但 MAX_REPLAN_ROUNDS=2，
/// ReplanRouter 必须在达到上限后拒绝 continue，路由到 finish 进入 BuildResponse。
#[tokio::test]
async fn replan_full_graph_budget_exhausted_finishes() {
    let account = SourceAccountRef::new(MessageSource::NapCat, "test-acct").unwrap();
    let cmd_event_id = SourceEventId::new("cmd-ev-002").unwrap();
    let context = std::sync::Arc::new(ActionRunContext {
        account: account.clone(),
        command_source_event_id: cmd_event_id.clone(),
        command_text: "帮我查一下".into(),
        conversation_id: "conv-2".into(),
        occurred_at_unix_secs: 1_800_000_000,
        timezone_offset_secs: 28_800,
        timezone: "Asia/Shanghai".into(),
        now_unix_secs: 1_800_000_000,
        lease_token: ActionLeaseToken::generate(),
        is_local_loopback: false,
    });

    // 所有 Planner 调用都返回 SearchRecentEvents（永不返回 NoAction）。
    // 每个调用必须返回具有不同 proposal_id 的新 Proposal；否则 ObservationAppended
    // 的去重逻辑会拒绝同一 proposal_id 的重复观察，导致 replan_round 不推进。
    fn search_proposal(evidence: Vec<SourceEventId>) -> PlannerOutput {
        PlannerOutput::Proposal(
            SecretaryActionProposal::new(
                SecretaryAction::SearchRecentEvents {
                    query: "x".into(),
                    limit: 10,
                    since_unix_secs: None,
                    until_unix_secs: None,
                    conversation: None,
                    thread_id: None,
                    actor_id: None,
                },
                "继续查",
                evidence,
                None,
            )
            .unwrap(),
        )
    }

    let planner = std::sync::Arc::new(RecordingPlanner::new(vec![
        search_proposal(vec![cmd_event_id.clone()]),
        search_proposal(vec![cmd_event_id.clone()]),
    ]));
    let effect_executor = std::sync::Arc::new(FakeEffectExecutor);

    let mut graph = GraphDefinition::new(GraphId::try_from("test_budget").unwrap());
    let plan_node =
        std::sync::Arc::new(PlanNode::new(planner.clone(), None, context.clone()).unwrap());
    let l0_node = std::sync::Arc::new(L0ExecuteNode::new().unwrap());
    let replan_node = std::sync::Arc::new(ReplanDecisionNode::new().unwrap());
    let build_node = std::sync::Arc::new(BuildResponseNode::new(context).unwrap());

    graph.add_node(plan_node.clone()).unwrap();
    graph.add_node(l0_node.clone()).unwrap();
    graph.add_node(replan_node.clone()).unwrap();
    graph.add_node(build_node.clone()).unwrap();

    let plan_id = NodeId::try_from("plan").unwrap();
    let l0_id = NodeId::try_from("l0_execute").unwrap();
    let replan_id = NodeId::try_from("replan_decision").unwrap();
    let build_id = NodeId::try_from("build_response").unwrap();

    graph.set_entry(plan_id.clone());
    graph
        .set_transition(plan_id.clone(), TransitionRule::Goto(l0_id.clone()))
        .unwrap();
    graph
        .set_transition(l0_id.clone(), TransitionRule::Goto(replan_id.clone()))
        .unwrap();

    let mut replan_targets = BTreeMap::new();
    replan_targets.insert(RouteKey::try_from("continue").unwrap(), plan_id);
    replan_targets.insert(RouteKey::try_from("finish").unwrap(), build_id.clone());
    graph
        .set_transition(
            replan_id,
            TransitionRule::Branch {
                router: std::sync::Arc::new(ReplanRouter),
                targets: replan_targets,
            },
        )
        .unwrap();
    graph.set_transition(build_id, TransitionRule::End).unwrap();

    let compiled = graph
        .compile(GraphPolicy::new(NonZeroU32::new(32).unwrap()))
        .unwrap();
    let runtime = GraphRuntime::with_effect_executor(compiled, effect_executor);

    let business = SecretaryAgentState::new(
        "goal",
        Vec::new(),
        vec![cmd_event_id.clone()],
        vec![RecentEventRef {
            source_event_id: cmd_event_id,
            summary: "cmd".into(),
        }],
    )
    .unwrap();
    let state = AgentState::new(business);

    let budget = RunBudget::new(NonZeroU32::new(32).unwrap(), Duration::from_secs(30));
    let result = runtime.run(state, budget).await.unwrap();

    // Planner 调用次数 = MAX_REPLAN_ROUNDS（每轮一次）
    let calls = planner.calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        usize::from(crate::planner::MAX_REPLAN_ROUNDS),
        "Planner should be called MAX_REPLAN_ROUNDS times"
    );

    // Effect 执行次数 = MAX_REPLAN_ROUNDS（每轮一次）
    assert_eq!(
        result.effect_receipts.len(),
        usize::from(crate::planner::MAX_REPLAN_ROUNDS),
        "Effect receipts should equal MAX_REPLAN_ROUNDS"
    );

    // 最终状态：outcome 必须设置（BuildResponseNode 从 last_receipt 构造响应并设置 Outcome）
    assert!(
        result.state.outcome().is_some(),
        "Should have outcome even when budget exhausted"
    );

    // replan_round 达到上限
    assert_eq!(
        result.state.business().replan_round(),
        crate::planner::MAX_REPLAN_ROUNDS
    );
}
