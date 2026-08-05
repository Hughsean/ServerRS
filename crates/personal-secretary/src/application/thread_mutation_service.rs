use std::sync::Arc;

use agent_core::graph::{
    AgentNode, EffectEnvelope, EffectError, EffectErrorKind, EffectExecutor, NodeError,
    NodeErrorKind, NodeId, NodeResult, RunContext, SuspendReason, SuspendRequest, UsageDelta,
};
use agent_core::{AgentOutcome, AgentState, AgentUpdate};
use async_trait::async_trait;
use thiserror::Error;

use crate::{
    EventThreadId, InboundEventStoreError, SecretaryAction, SourceAccountRef, SourceEventId,
    ThreadMutationAgentState, ThreadMutationApprovalRequest, ThreadMutationDecision,
    ThreadMutationEffect, ThreadMutationEffectReceipt, ThreadMutationImpact,
    ThreadMutationProposalStatus, ThreadMutationResumeInput, ThreadMutationRevertInput,
    ThreadMutationRevertReceipt, ThreadMutationUpdate, validate_thread_mutation_impact,
    validate_thread_mutation_revert,
};

/// 从 Owner Action 构造权威影响预览的应用请求。计数和完整成员集合由 Store
/// 重新读取，Planner 不能自行声明影响范围。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadMutationImpactRequest {
    pub proposal_id: crate::ThreadMutationProposalId,
    pub kind: crate::ThreadMutationKind,
    pub account: SourceAccountRef,
    pub thread_ids: Vec<EventThreadId>,
    pub selected_source_event_ids: Vec<SourceEventId>,
    pub reason: String,
}

#[async_trait]
pub trait ThreadMutationStoreT: Send + Sync {
    async fn build_impact(
        &self,
        request: &ThreadMutationImpactRequest,
    ) -> Result<ThreadMutationImpact, InboundEventStoreError>;

    async fn persist_proposal(
        &self,
        impact: &ThreadMutationImpact,
    ) -> Result<(), InboundEventStoreError>;

    async fn authorize_resume(
        &self,
        input: &ThreadMutationResumeInput,
    ) -> Result<ThreadMutationProposalStatus, InboundEventStoreError>;

    async fn apply_effect(
        &self,
        effect: &ThreadMutationEffect,
        effect_id: &str,
    ) -> Result<ThreadMutationEffectReceipt, InboundEventStoreError>;

    async fn revert_applied(
        &self,
        input: &ThreadMutationRevertInput,
    ) -> Result<ThreadMutationRevertReceipt, InboundEventStoreError>;
}

pub struct ThreadMutationUseCase {
    store: Arc<dyn ThreadMutationStoreT>,
}

pub struct ThreadMutationRevertUseCase {
    store: Arc<dyn ThreadMutationStoreT>,
}

impl ThreadMutationRevertUseCase {
    pub fn new(store: Arc<dyn ThreadMutationStoreT>) -> Self {
        Self { store }
    }

    pub async fn revert(
        &self,
        input: &ThreadMutationRevertInput,
    ) -> Result<ThreadMutationRevertReceipt, ThreadMutationUseCaseError> {
        validate_thread_mutation_revert(input)?;
        Ok(self.store.revert_applied(input).await?)
    }
}

impl ThreadMutationUseCase {
    pub fn new(store: Arc<dyn ThreadMutationStoreT>) -> Self {
        Self { store }
    }

    pub async fn prepare(
        &self,
        impact: ThreadMutationImpact,
    ) -> Result<ThreadMutationAgentState, ThreadMutationUseCaseError> {
        validate_thread_mutation_impact(&impact)?;
        self.store.persist_proposal(&impact).await?;
        Ok(ThreadMutationAgentState::new(impact)?)
    }

    /// 统一 Action Graph 已完成 L2 Suspend/Resume 后，复用既有线程变更仓储的
    /// Proposal、OwnerBinding 复验和幂等 Effect 提交，不建立第二套审批状态机。
    pub async fn apply_approved_action(
        &self,
        account: &SourceAccountRef,
        proposal_id: &str,
        action: &SecretaryAction,
        command_source_event_id: &SourceEventId,
        effect_id: &str,
    ) -> Result<ThreadMutationEffectReceipt, ThreadMutationUseCaseError> {
        let proposal_id = crate::ThreadMutationProposalId::new(proposal_id)?;
        let request = match action {
            SecretaryAction::MergeThreads { thread_ids, reason } => ThreadMutationImpactRequest {
                proposal_id: proposal_id.clone(),
                kind: crate::ThreadMutationKind::Merge,
                account: account.clone(),
                thread_ids: thread_ids.clone(),
                selected_source_event_ids: Vec::new(),
                reason: reason.clone(),
            },
            SecretaryAction::SplitThread {
                thread_id,
                source_event_ids,
                reason,
            } => ThreadMutationImpactRequest {
                proposal_id: proposal_id.clone(),
                kind: crate::ThreadMutationKind::Split,
                account: account.clone(),
                thread_ids: vec![thread_id.clone()],
                selected_source_event_ids: source_event_ids.clone(),
                reason: reason.clone(),
            },
            _ => {
                return Err(ThreadMutationUseCaseError::Domain(
                    crate::ThreadMutationError::InvalidImpact(
                        "action is not a thread mutation".into(),
                    ),
                ));
            }
        };
        let impact = self.store.build_impact(&request).await?;
        validate_thread_mutation_impact(&impact)?;
        self.store.persist_proposal(&impact).await?;
        let status = self
            .store
            .authorize_resume(&ThreadMutationResumeInput {
                proposal_id: proposal_id.clone(),
                decision: ThreadMutationDecision::Approve,
                command_source_event_id: command_source_event_id.clone(),
            })
            .await?;
        if status != ThreadMutationProposalStatus::Approved {
            return Err(ThreadMutationUseCaseError::Store(
                InboundEventStoreError::InvalidData(
                    "thread mutation approval did not converge to approved".into(),
                ),
            ));
        }
        Ok(self
            .store
            .apply_effect(
                &ThreadMutationEffect {
                    proposal_id,
                    kind: impact.kind,
                },
                effect_id,
            )
            .await?)
    }
}

pub struct ThreadMutationApprovalNode {
    id: NodeId,
}

impl ThreadMutationApprovalNode {
    pub fn new() -> Result<Self, ThreadMutationUseCaseError> {
        Ok(Self {
            id: NodeId::try_from("thread_mutation_approval")
                .map_err(|error| ThreadMutationUseCaseError::Configuration(error.to_string()))?,
        })
    }
}

#[async_trait]
impl AgentNode<ThreadMutationAgentState> for ThreadMutationApprovalNode {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        state: &AgentState<ThreadMutationAgentState>,
        _context: &RunContext,
    ) -> Result<
        NodeResult<ThreadMutationUpdate, ThreadMutationEffect, ThreadMutationApprovalRequest>,
        NodeError,
    > {
        let impact = state.business().impact();
        validate_thread_mutation_impact(impact)
            .map_err(|error| NodeError::with_source(NodeErrorKind::Invariant, error))?;
        let warning = match impact.kind {
            crate::ThreadMutationKind::Merge => {
                "合并会改变后续检索与记忆投影；确认前不会移动任何事件"
            }
            crate::ThreadMutationKind::Split => {
                "拆分会改变后续因果线程归属；确认前不会移动任何事件"
            }
        };
        Ok(NodeResult::Suspend {
            updates: Vec::new(),
            effects: Vec::new(),
            usage: UsageDelta::default(),
            request: SuspendRequest::new(
                SuspendReason::Approval,
                ThreadMutationApprovalRequest {
                    proposal_id: impact.proposal_id.clone(),
                    kind: impact.kind,
                    account: impact.account.clone(),
                    thread_ids: impact.thread_ids.clone(),
                    affected_event_count: impact.affected_event_count,
                    affected_conversation_count: impact.affected_conversation_count,
                    affected_source_event_ids: impact.affected_source_event_ids.clone(),
                    warning: warning.into(),
                },
            ),
        })
    }
}

pub struct ThreadMutationDecisionNode {
    id: NodeId,
    store: Arc<dyn ThreadMutationStoreT>,
}

impl ThreadMutationDecisionNode {
    pub fn new(store: Arc<dyn ThreadMutationStoreT>) -> Result<Self, ThreadMutationUseCaseError> {
        Ok(Self {
            id: NodeId::try_from("thread_mutation_decision")
                .map_err(|error| ThreadMutationUseCaseError::Configuration(error.to_string()))?,
            store,
        })
    }
}

#[async_trait]
impl AgentNode<ThreadMutationAgentState> for ThreadMutationDecisionNode {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        state: &AgentState<ThreadMutationAgentState>,
        _context: &RunContext,
    ) -> Result<
        NodeResult<ThreadMutationUpdate, ThreadMutationEffect, ThreadMutationApprovalRequest>,
        NodeError,
    > {
        let input = state.business().resume_input().ok_or_else(|| {
            NodeError::new(NodeErrorKind::Invariant, "线程变更决策节点缺少 ResumeInput")
        })?;
        let status = self
            .store
            .authorize_resume(input)
            .await
            .map_err(node_error)?;
        match (input.decision, status) {
            (ThreadMutationDecision::Reject, ThreadMutationProposalStatus::Rejected) => {
                Ok(NodeResult::Continue {
                    updates: vec![
                        AgentUpdate::Business(ThreadMutationUpdate::Rejected),
                        AgentUpdate::SetOutcome(AgentOutcome::Respond(
                            "线程变更已拒绝，未修改线程投影".into(),
                        )),
                    ],
                    effects: Vec::new(),
                    usage: UsageDelta::default(),
                })
            }
            (ThreadMutationDecision::Approve, ThreadMutationProposalStatus::Approved) => {
                Ok(NodeResult::Continue {
                    updates: vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
                        "线程变更已批准并应用".into(),
                    ))],
                    effects: vec![ThreadMutationEffect {
                        proposal_id: input.proposal_id.clone(),
                        kind: state.business().impact().kind,
                    }],
                    usage: UsageDelta::default(),
                })
            }
            _ => Err(NodeError::new(
                NodeErrorKind::Invariant,
                "线程变更 Resume 决策与持久化状态不一致",
            )),
        }
    }
}

pub struct ThreadMutationEffectExecutor {
    store: Arc<dyn ThreadMutationStoreT>,
}

impl ThreadMutationEffectExecutor {
    pub fn new(store: Arc<dyn ThreadMutationStoreT>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl EffectExecutor<ThreadMutationEffect> for ThreadMutationEffectExecutor {
    async fn execute(
        &self,
        envelope: &EffectEnvelope<ThreadMutationEffect>,
        _context: &RunContext,
    ) -> Result<ThreadMutationEffectReceipt, EffectError> {
        self.store
            .apply_effect(&envelope.effect, &envelope.id.to_string())
            .await
            .map_err(|error| match error {
                InboundEventStoreError::InvalidData(_) | InboundEventStoreError::LeaseLost => {
                    EffectError::with_source(EffectErrorKind::Permanent, error)
                }
                InboundEventStoreError::Unavailable | InboundEventStoreError::Database(_) => {
                    // 数据库提交是否成功无法从传输错误判断；Runtime 不得自动重放。
                    EffectError::with_source(EffectErrorKind::UnknownCommit, error)
                }
            })
    }
}

fn node_error(error: InboundEventStoreError) -> NodeError {
    let kind = match &error {
        InboundEventStoreError::Unavailable | InboundEventStoreError::Database(_) => {
            NodeErrorKind::Transient
        }
        InboundEventStoreError::InvalidData(_) | InboundEventStoreError::LeaseLost => {
            NodeErrorKind::Permanent
        }
    };
    NodeError::with_source(kind, error)
}

#[derive(Debug, Error)]
pub enum ThreadMutationUseCaseError {
    #[error("invalid thread mutation configuration: {0}")]
    Configuration(String),
    #[error(transparent)]
    Domain(#[from] crate::ThreadMutationError),
    #[error(transparent)]
    Store(#[from] InboundEventStoreError),
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::sync::Mutex;
    use std::time::Duration;

    use agent_core::graph::{
        GraphDefinition, GraphExecutionResult, GraphId, GraphPolicy, GraphRuntime,
        InMemoryCheckpointStore, RunBudget, TransitionRule,
    };

    use super::*;
    use crate::{
        EventThreadId, MessageSource, SourceAccountRef, SourceEventId, ThreadMutationKind,
        ThreadMutationProposalId,
    };

    #[derive(Default)]
    struct FakeStore {
        proposals: Mutex<Vec<ThreadMutationProposalId>>,
        effects: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ThreadMutationStoreT for FakeStore {
        async fn build_impact(
            &self,
            request: &ThreadMutationImpactRequest,
        ) -> Result<ThreadMutationImpact, InboundEventStoreError> {
            let source_event_ids = if request.selected_source_event_ids.is_empty() {
                vec![SourceEventId::new("event").unwrap()]
            } else {
                request.selected_source_event_ids.clone()
            };
            Ok(ThreadMutationImpact {
                proposal_id: request.proposal_id.clone(),
                kind: request.kind,
                account: request.account.clone(),
                thread_ids: request.thread_ids.clone(),
                affected_event_count: source_event_ids.len() as u32,
                affected_conversation_count: 1,
                affected_source_event_ids: source_event_ids,
                reason: request.reason.clone(),
            })
        }

        async fn persist_proposal(
            &self,
            impact: &ThreadMutationImpact,
        ) -> Result<(), InboundEventStoreError> {
            self.proposals
                .lock()
                .unwrap()
                .push(impact.proposal_id.clone());
            Ok(())
        }

        async fn authorize_resume(
            &self,
            input: &ThreadMutationResumeInput,
        ) -> Result<ThreadMutationProposalStatus, InboundEventStoreError> {
            Ok(match input.decision {
                ThreadMutationDecision::Approve => ThreadMutationProposalStatus::Approved,
                ThreadMutationDecision::Reject => ThreadMutationProposalStatus::Rejected,
            })
        }

        async fn apply_effect(
            &self,
            effect: &ThreadMutationEffect,
            effect_id: &str,
        ) -> Result<ThreadMutationEffectReceipt, InboundEventStoreError> {
            self.effects.lock().unwrap().push(effect_id.into());
            Ok(ThreadMutationEffectReceipt {
                proposal_id: effect.proposal_id.clone(),
                effect_id: effect_id.into(),
                status: ThreadMutationProposalStatus::Applied,
                changed: true,
            })
        }

        async fn revert_applied(
            &self,
            input: &ThreadMutationRevertInput,
        ) -> Result<ThreadMutationRevertReceipt, InboundEventStoreError> {
            Ok(ThreadMutationRevertReceipt {
                proposal_id: input.proposal_id.clone(),
                changed: true,
            })
        }
    }

    fn impact() -> ThreadMutationImpact {
        ThreadMutationImpact {
            proposal_id: ThreadMutationProposalId::new("00000000-0000-0000-0000-000000000091")
                .unwrap(),
            kind: ThreadMutationKind::Merge,
            account: SourceAccountRef::new(MessageSource::NapCat, "managed-account").unwrap(),
            thread_ids: vec![
                EventThreadId::new("thread-a").unwrap(),
                EventThreadId::new("thread-b").unwrap(),
            ],
            affected_event_count: 2,
            affected_conversation_count: 2,
            affected_source_event_ids: vec![
                SourceEventId::new("event-a").unwrap(),
                SourceEventId::new("event-b").unwrap(),
            ],
            reason: "Owner 明确要求合并同一事项的两个线程".into(),
        }
    }

    fn runtime(store: Arc<dyn ThreadMutationStoreT>) -> GraphRuntime<ThreadMutationAgentState> {
        let approval_id = NodeId::try_from("thread_mutation_approval").unwrap();
        let decision_id = NodeId::try_from("thread_mutation_decision").unwrap();
        let mut definition = GraphDefinition::new(GraphId::try_from("thread-mutation").unwrap());
        definition
            .add_node(Arc::new(ThreadMutationApprovalNode::new().unwrap()))
            .unwrap();
        definition
            .add_node(Arc::new(
                ThreadMutationDecisionNode::new(store.clone()).unwrap(),
            ))
            .unwrap();
        definition.set_entry(approval_id.clone());
        definition
            .set_transition(approval_id, TransitionRule::Goto(decision_id.clone()))
            .unwrap();
        definition
            .set_transition(decision_id, TransitionRule::End)
            .unwrap();
        let compiled = definition
            .compile(GraphPolicy::new(NonZeroU32::new(4).unwrap()))
            .unwrap();
        GraphRuntime::with_effect_executor(
            compiled,
            Arc::new(ThreadMutationEffectExecutor::new(store)),
        )
        .with_checkpoint_store(Arc::new(InMemoryCheckpointStore::new()))
    }

    #[tokio::test]
    async fn proposal_suspends_then_owner_resume_applies_one_typed_effect() {
        let fake = Arc::new(FakeStore::default());
        let store: Arc<dyn ThreadMutationStoreT> = fake.clone();
        let state = ThreadMutationUseCase::new(store.clone())
            .prepare(impact())
            .await
            .unwrap();
        let runtime = runtime(store);
        let suspended = match runtime
            .run_checkpointed(
                AgentState::new(state),
                RunBudget::new(NonZeroU32::new(4).unwrap(), Duration::from_secs(5)),
            )
            .await
            .unwrap()
        {
            GraphExecutionResult::Suspended(value) => value,
            GraphExecutionResult::Completed(_) => panic!("approval node must suspend"),
        };
        assert_eq!(
            suspended.checkpoint().suspend().reason,
            SuspendReason::Approval
        );
        assert!(fake.effects.lock().unwrap().is_empty());

        let completed = match runtime
            .resume(
                suspended.checkpoint().id(),
                ThreadMutationResumeInput {
                    proposal_id: impact().proposal_id,
                    decision: ThreadMutationDecision::Approve,
                    command_source_event_id: SourceEventId::new("owner-command").unwrap(),
                },
            )
            .await
            .unwrap()
        {
            GraphExecutionResult::Completed(value) => value,
            GraphExecutionResult::Suspended(_) => panic!("decision node must complete"),
        };
        assert_eq!(
            completed.state.business().status(),
            ThreadMutationProposalStatus::Applied
        );
        assert!(completed.state.business().receipt().is_some());
        assert_eq!(fake.effects.lock().unwrap().len(), 1);
        assert_eq!(completed.effect_receipts.len(), 1);
    }

    #[tokio::test]
    async fn rejected_resume_completes_without_effect() {
        let fake = Arc::new(FakeStore::default());
        let store: Arc<dyn ThreadMutationStoreT> = fake.clone();
        let state = ThreadMutationUseCase::new(store.clone())
            .prepare(impact())
            .await
            .unwrap();
        let runtime = runtime(store);
        let suspended = match runtime
            .run_checkpointed(
                AgentState::new(state),
                RunBudget::new(NonZeroU32::new(4).unwrap(), Duration::from_secs(5)),
            )
            .await
            .unwrap()
        {
            GraphExecutionResult::Suspended(value) => value,
            GraphExecutionResult::Completed(_) => panic!("approval node must suspend"),
        };
        let completed = match runtime
            .resume(
                suspended.checkpoint().id(),
                ThreadMutationResumeInput {
                    proposal_id: impact().proposal_id,
                    decision: ThreadMutationDecision::Reject,
                    command_source_event_id: SourceEventId::new("owner-command").unwrap(),
                },
            )
            .await
            .unwrap()
        {
            GraphExecutionResult::Completed(value) => value,
            GraphExecutionResult::Suspended(_) => panic!("decision node must complete"),
        };
        assert_eq!(
            completed.state.business().status(),
            ThreadMutationProposalStatus::Rejected
        );
        assert!(fake.effects.lock().unwrap().is_empty());
        assert!(completed.effect_receipts.is_empty());
    }

    #[tokio::test]
    async fn approved_owner_action_reuses_proposal_authorization_and_effect_store() {
        let fake = Arc::new(FakeStore::default());
        let use_case = ThreadMutationUseCase::new(fake.clone());
        let action = SecretaryAction::MergeThreads {
            thread_ids: vec![
                EventThreadId::new("thread-a").unwrap(),
                EventThreadId::new("thread-b").unwrap(),
            ],
            reason: "Owner 确认两个线程属于同一事项".into(),
        };
        let receipt = use_case
            .apply_approved_action(
                &SourceAccountRef::new(MessageSource::NapCat, "managed-account").unwrap(),
                "00000000-0000-0000-0000-000000000099",
                &action,
                &SourceEventId::new("owner-command").unwrap(),
                "effect-1",
            )
            .await
            .unwrap();
        assert_eq!(receipt.status, ThreadMutationProposalStatus::Applied);
        assert_eq!(fake.proposals.lock().unwrap().len(), 1);
        assert_eq!(fake.effects.lock().unwrap().as_slice(), &["effect-1"]);
    }
}
