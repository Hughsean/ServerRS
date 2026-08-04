//! EffectExecutor：根据 Action 类型执行真实查询，再持久化幂等 Receipt。
//!
//! P0-3 修复：显式持有 run_id，避免误用 proposal_id。
//! P0-4 修复：根据 Action 类型调用 Retriever 生成真实查询结果，
//! 再调 `ActionStoreT::apply_effect` 持久化幂等 Receipt。

use std::sync::Arc;

use agent_core::graph::{EffectEnvelope, EffectError, EffectErrorKind, EffectExecutor, RunContext};
use async_trait::async_trait;

use crate::{
    AccountScopedParticipantRef, AgendaApplyRequest, AgendaError, AgendaItemId, AgendaMutation,
    AgendaUseCase, ConversationMemoryModeInput, EventQuery, FollowUpControlEffectRequest,
    FollowUpControlStoreError, FollowUpControlUseCase, MemoryCandidateControlEffectRequest,
    MemoryCandidateControlStoreError, MemoryCandidateControlUseCase, MemoryCandidateUseCase,
    MemoryDeleteInput, MemoryFact, MemoryFactId, MemoryFactStatus, MemoryUseCase,
    NotificationPolicyEffectRequest, NotificationPolicyUseCase, QueryEffectResultV1,
    QueryEffectTypedEvent, ReferenceContext, ResponseExpectationControlEffectRequest,
    ResponseExpectationControlStoreError, ResponseExpectationControlUseCase, RetrieverUseCase,
    SecretaryAction, SecretaryActionEffect, SecretaryActionReceipt, SecretaryToolKind,
    SourceAccountRef, SourceEventId, ThreadControlEffectRequest, ThreadControlStoreError,
    ThreadControlUseCase,
};

use super::port::{ActionLeaseToken, ActionRunId, ActionStoreError, ActionStoreT};

/// Secretary Action Effect 执行器。
/// P0-3 修复：显式持有 run_id，避免误用 proposal_id。
/// P0-4 修复：根据 Action 类型调用 Retriever 生成真实查询结果，
/// 再调 ActionStoreT::apply_effect 持久化幂等 Receipt。
pub struct SecretaryActionEffectExecutor {
    store: Arc<dyn ActionStoreT>,
    run_id: ActionRunId,
    lease_token: ActionLeaseToken,
    retriever: Option<Arc<RetrieverUseCase>>,
    notification_policy: Option<Arc<NotificationPolicyUseCase>>,
    agenda: Option<Arc<AgendaUseCase>>,
    memory: Option<Arc<MemoryUseCase>>,
    thread_control: Option<Arc<ThreadControlUseCase>>,
    follow_up_control: Option<Arc<FollowUpControlUseCase>>,
    response_expectation_control: Option<Arc<ResponseExpectationControlUseCase>>,
    memory_candidate: Option<Arc<MemoryCandidateUseCase>>,
    memory_candidate_control: Option<Arc<MemoryCandidateControlUseCase>>,
    command_source_event_id: Option<SourceEventId>,
    account: SourceAccountRef,
    now_unix_secs: i64,
    is_local_loopback: bool,
}

impl SecretaryActionEffectExecutor {
    pub fn new(
        store: Arc<dyn ActionStoreT>,
        run_id: ActionRunId,
        lease_token: ActionLeaseToken,
        retriever: Option<Arc<RetrieverUseCase>>,
        account: SourceAccountRef,
        now_unix_secs: i64,
    ) -> Self {
        Self {
            store,
            run_id,
            lease_token,
            retriever,
            notification_policy: None,
            agenda: None,
            memory: None,
            thread_control: None,
            follow_up_control: None,
            response_expectation_control: None,
            memory_candidate: None,
            memory_candidate_control: None,
            command_source_event_id: None,
            account,
            now_unix_secs,
            is_local_loopback: false,
        }
    }

    /// 注入经过运行时配置验证的本地模型标记；默认远程路径 fail-closed。
    pub fn with_loopback(mut self, is_local_loopback: bool) -> Self {
        self.is_local_loopback = is_local_loopback;
        self
    }

    pub fn with_notification_policy(
        mut self,
        notification_policy: Arc<NotificationPolicyUseCase>,
        command_source_event_id: SourceEventId,
    ) -> Self {
        self.notification_policy = Some(notification_policy);
        self.command_source_event_id = Some(command_source_event_id);
        self
    }

    pub fn with_agenda(
        mut self,
        agenda: Arc<AgendaUseCase>,
        command_source_event_id: SourceEventId,
    ) -> Self {
        self.agenda = Some(agenda);
        self.command_source_event_id = Some(command_source_event_id);
        self
    }

    pub fn with_memory(
        mut self,
        memory: Arc<MemoryUseCase>,
        command_source_event_id: SourceEventId,
    ) -> Self {
        self.memory = Some(memory);
        self.command_source_event_id = Some(command_source_event_id);
        self
    }

    pub fn with_thread_control(
        mut self,
        thread_control: Arc<ThreadControlUseCase>,
        command_source_event_id: SourceEventId,
    ) -> Self {
        self.thread_control = Some(thread_control);
        self.command_source_event_id = Some(command_source_event_id);
        self
    }

    pub fn with_follow_up_control(
        mut self,
        follow_up_control: Arc<FollowUpControlUseCase>,
        command_source_event_id: SourceEventId,
    ) -> Self {
        self.follow_up_control = Some(follow_up_control);
        self.command_source_event_id = Some(command_source_event_id);
        self
    }

    pub fn with_response_expectation_control(
        mut self,
        response_expectation_control: Arc<ResponseExpectationControlUseCase>,
        command_source_event_id: SourceEventId,
    ) -> Self {
        self.response_expectation_control = Some(response_expectation_control);
        self.command_source_event_id = Some(command_source_event_id);
        self
    }

    pub fn with_memory_candidate(mut self, memory_candidate: Arc<MemoryCandidateUseCase>) -> Self {
        self.memory_candidate = Some(memory_candidate);
        self
    }

    pub fn with_memory_candidate_control(
        mut self,
        memory_candidate_control: Arc<MemoryCandidateControlUseCase>,
        command_source_event_id: SourceEventId,
    ) -> Self {
        self.memory_candidate_control = Some(memory_candidate_control);
        self.command_source_event_id = Some(command_source_event_id);
        self
    }

    async fn execute_follow_up_control(
        &self,
        proposal: &crate::SecretaryActionProposal,
        effect_id: &str,
    ) -> Result<Option<SecretaryActionReceipt>, EffectError> {
        if !is_follow_up_control_action(&proposal.action) {
            return Ok(None);
        }
        let use_case = self.follow_up_control.as_ref().ok_or_else(|| {
            EffectError::new(EffectErrorKind::Permanent, "FollowUpControlUseCase 未注入")
        })?;
        let command_source_event_id = self.command_source_event_id.clone().ok_or_else(|| {
            EffectError::new(
                EffectErrorKind::Permanent,
                "FollowUp 控制需要原始 OwnerCommand 身份",
            )
        })?;
        let proposal_json = serde_json::to_string(proposal)
            .map_err(|error| EffectError::new(EffectErrorKind::Permanent, error.to_string()))?;
        let receipt = use_case
            .apply_effect(&FollowUpControlEffectRequest {
                account: self.account.clone(),
                command_source_event_id,
                run_id: self.run_id.clone(),
                lease_token: self.lease_token.clone(),
                effect_id: effect_id.to_owned(),
                proposal_id: proposal.proposal_id.clone(),
                proposal_json,
                action: proposal.action.clone(),
            })
            .await
            .map_err(follow_up_control_effect_error)?;
        Ok(Some(receipt))
    }

    async fn execute_response_expectation_control(
        &self,
        proposal: &crate::SecretaryActionProposal,
        effect_id: &str,
    ) -> Result<Option<SecretaryActionReceipt>, EffectError> {
        if !is_response_expectation_control_action(&proposal.action) {
            return Ok(None);
        }
        let use_case = self.response_expectation_control.as_ref().ok_or_else(|| {
            EffectError::new(
                EffectErrorKind::Permanent,
                "ResponseExpectationControlUseCase 未注入",
            )
        })?;
        let command_source_event_id = self.command_source_event_id.clone().ok_or_else(|| {
            EffectError::new(
                EffectErrorKind::Permanent,
                "回复期待控制需要原始 OwnerCommand 身份",
            )
        })?;
        let proposal_json = serde_json::to_string(proposal)
            .map_err(|error| EffectError::new(EffectErrorKind::Permanent, error.to_string()))?;
        let receipt = use_case
            .apply_effect(&ResponseExpectationControlEffectRequest {
                account: self.account.clone(),
                command_source_event_id,
                run_id: self.run_id.clone(),
                lease_token: self.lease_token.clone(),
                effect_id: effect_id.to_owned(),
                proposal_id: proposal.proposal_id.clone(),
                proposal_json,
                action: proposal.action.clone(),
            })
            .await
            .map_err(response_expectation_control_effect_error)?;
        Ok(Some(receipt))
    }

    async fn execute_memory_candidate_control(
        &self,
        proposal: &crate::SecretaryActionProposal,
        effect_id: &str,
    ) -> Result<Option<SecretaryActionReceipt>, EffectError> {
        if !is_memory_candidate_control_action(&proposal.action) {
            return Ok(None);
        }
        let use_case = self.memory_candidate_control.as_ref().ok_or_else(|| {
            EffectError::new(
                EffectErrorKind::Permanent,
                "MemoryCandidateControlUseCase 未注入",
            )
        })?;
        let command_source_event_id = self.command_source_event_id.clone().ok_or_else(|| {
            EffectError::new(
                EffectErrorKind::Permanent,
                "记忆候选控制需要原始 OwnerCommand 身份",
            )
        })?;
        let proposal_json = serde_json::to_string(proposal)
            .map_err(|error| EffectError::new(EffectErrorKind::Permanent, error.to_string()))?;
        let receipt = use_case
            .apply_effect(&MemoryCandidateControlEffectRequest {
                account: self.account.clone(),
                command_source_event_id,
                run_id: self.run_id.clone(),
                lease_token: self.lease_token.clone(),
                effect_id: effect_id.to_owned(),
                proposal_id: proposal.proposal_id.clone(),
                proposal_json,
                action: proposal.action.clone(),
            })
            .await
            .map_err(memory_candidate_control_effect_error)?;
        Ok(Some(receipt))
    }

    async fn execute_thread_control(
        &self,
        proposal: &crate::SecretaryActionProposal,
        effect_id: &str,
    ) -> Result<Option<SecretaryActionReceipt>, EffectError> {
        if !is_thread_control_action(&proposal.action) {
            return Ok(None);
        }
        let use_case = self.thread_control.as_ref().ok_or_else(|| {
            EffectError::new(EffectErrorKind::Permanent, "ThreadControlUseCase 未注入")
        })?;
        let command_source_event_id = self.command_source_event_id.clone().ok_or_else(|| {
            EffectError::new(
                EffectErrorKind::Permanent,
                "线程控制需要原始 OwnerCommand 身份",
            )
        })?;
        let proposal_json = serde_json::to_string(proposal)
            .map_err(|error| EffectError::new(EffectErrorKind::Permanent, error.to_string()))?;
        let receipt = use_case
            .apply_effect(&ThreadControlEffectRequest {
                account: self.account.clone(),
                command_source_event_id,
                run_id: self.run_id.clone(),
                lease_token: self.lease_token.clone(),
                effect_id: effect_id.to_owned(),
                proposal_id: proposal.proposal_id.clone(),
                proposal_json,
                action: proposal.action.clone(),
            })
            .await
            .map_err(thread_control_effect_error)?;
        Ok(Some(receipt))
    }

    async fn execute_memory(
        &self,
        action: &SecretaryAction,
        effect_id: &str,
    ) -> Result<Option<String>, EffectError> {
        let is_memory_action = matches!(
            action,
            SecretaryAction::ListMemoryFacts { .. }
                | SecretaryAction::ReadMemoryFactSources { .. }
                | SecretaryAction::CorrectMemoryFact { .. }
                | SecretaryAction::DeleteMemoryFact { .. }
                | SecretaryAction::SetMemoryFactTtl { .. }
                | SecretaryAction::SetConversationMemoryMode { .. }
        );
        if !is_memory_action {
            return Ok(None);
        }
        let memory = self
            .memory
            .as_ref()
            .ok_or_else(|| EffectError::new(EffectErrorKind::Permanent, "MemoryUseCase 未注入"))?;
        let result = match action {
            SecretaryAction::ListMemoryFacts { limit } => {
                let facts = memory
                    .active(&self.account, u32::from(*limit))
                    .await
                    .map_err(memory_effect_error)?;
                format_memory_facts(&facts)
            }
            SecretaryAction::ReadMemoryFactSources {
                fact_id,
                max_excerpt_chars,
            } => {
                let view = memory
                    .evidence(fact_id, u32::from(*max_excerpt_chars))
                    .await
                    .map_err(memory_effect_error)?;
                match view {
                    Some(view) if view.fact.account == self.account => {
                        format_memory_evidence(&view)
                    }
                    _ => "未找到该账号下的记忆或来源已不可见".into(),
                }
            }
            SecretaryAction::CorrectMemoryFact {
                fact_id,
                replacement,
                confidence_bps,
                source_event_ids,
                valid_until_unix_secs,
            } => {
                if valid_until_unix_secs.is_some_and(|value| value <= self.now_unix_secs) {
                    return Err(EffectError::new(
                        EffectErrorKind::Permanent,
                        "记忆有效期必须晚于当前时间",
                    ));
                }
                let previous = memory
                    .evidence(fact_id, 1)
                    .await
                    .map_err(memory_effect_error)?
                    .ok_or_else(|| {
                        EffectError::new(EffectErrorKind::Permanent, "待修正记忆不存在")
                    })?;
                ensure_active_memory_for_account(&previous.fact, &self.account)?;
                let new_id = deterministic_memory_fact_id(effect_id)?;
                memory
                    .remember(&MemoryFact {
                        fact_id: new_id.clone(),
                        account: self.account.clone(),
                        subject_key: previous.fact.subject_key,
                        payload: replacement.clone(),
                        status: MemoryFactStatus::Confirmed,
                        confidence_bps: *confidence_bps,
                        source_event_ids: source_event_ids.clone(),
                        valid_until_unix_secs: *valid_until_unix_secs,
                        supersedes_fact_id: Some(fact_id.clone()),
                    })
                    .await
                    .map_err(memory_effect_error)?;
                format!("记忆已修正，新版本 {}", new_id.as_str())
            }
            SecretaryAction::DeleteMemoryFact { fact_id, reason } => {
                let command_source_event_id =
                    self.command_source_event_id.clone().ok_or_else(|| {
                        EffectError::new(
                            EffectErrorKind::Permanent,
                            "删除记忆需要原始 OwnerCommand 身份",
                        )
                    })?;
                memory
                    .delete_derived(&MemoryDeleteInput {
                        fact_id: fact_id.clone(),
                        command_source_event_id,
                        reason: reason.clone(),
                    })
                    .await
                    .map_err(memory_effect_error)?;
                format!("记忆 {} 已删除；原始聊天记录未删除", fact_id.as_str())
            }
            SecretaryAction::SetMemoryFactTtl {
                fact_id,
                valid_until_unix_secs,
            } => {
                if valid_until_unix_secs.is_some_and(|value| value <= self.now_unix_secs) {
                    return Err(EffectError::new(
                        EffectErrorKind::Permanent,
                        "记忆有效期必须晚于当前时间",
                    ));
                }
                let previous = memory
                    .evidence(fact_id, 1)
                    .await
                    .map_err(memory_effect_error)?
                    .ok_or_else(|| {
                        EffectError::new(EffectErrorKind::Permanent, "待设置有效期的记忆不存在")
                    })?;
                ensure_active_memory_for_account(&previous.fact, &self.account)?;
                let new_id = deterministic_memory_fact_id(effect_id)?;
                memory
                    .remember(&MemoryFact {
                        fact_id: new_id.clone(),
                        account: previous.fact.account,
                        subject_key: previous.fact.subject_key,
                        payload: previous.fact.payload,
                        status: MemoryFactStatus::Confirmed,
                        confidence_bps: previous.fact.confidence_bps,
                        source_event_ids: previous.fact.source_event_ids,
                        valid_until_unix_secs: *valid_until_unix_secs,
                        supersedes_fact_id: Some(fact_id.clone()),
                    })
                    .await
                    .map_err(memory_effect_error)?;
                match valid_until_unix_secs {
                    Some(value) => {
                        format!("记忆有效期已设置为 {value}，新版本 {}", new_id.as_str())
                    }
                    None => format!("记忆有效期已取消，新版本 {}", new_id.as_str()),
                }
            }
            SecretaryAction::SetConversationMemoryMode { conversation, mode } => {
                let command_source_event_id =
                    self.command_source_event_id.clone().ok_or_else(|| {
                        EffectError::new(
                            EffectErrorKind::Permanent,
                            "设置会话记忆模式需要原始 OwnerCommand 身份",
                        )
                    })?;
                memory
                    .set_conversation_mode(&ConversationMemoryModeInput {
                        account: self.account.clone(),
                        conversation: conversation.clone(),
                        command_source_event_id,
                        mode: *mode,
                    })
                    .await
                    .map_err(memory_effect_error)?;
                format!("会话长期记忆模式已设置为 {}", mode.as_str())
            }
            _ => unreachable!("memory action guard and match must stay exhaustive"),
        };
        Ok(Some(result))
    }

    async fn execute_agenda(
        &self,
        proposal: &crate::SecretaryActionProposal,
        effect_id: &str,
    ) -> Result<Option<SecretaryActionReceipt>, EffectError> {
        let mutation = match &proposal.action {
            SecretaryAction::CreateSchedule {
                title,
                starts_at_unix,
                timezone,
            } => AgendaMutation::Create {
                kind: crate::AgendaItemKind::Schedule,
                title: title.clone(),
                scheduled_at_unix_secs: Some(*starts_at_unix),
                timezone: timezone.clone(),
            },
            SecretaryAction::CreateTask {
                title,
                due_at_unix,
                timezone,
            } => AgendaMutation::Create {
                kind: crate::AgendaItemKind::Task,
                title: title.clone(),
                scheduled_at_unix_secs: *due_at_unix,
                timezone: timezone.clone(),
            },
            SecretaryAction::CreateReminder {
                text,
                due_at_unix,
                timezone,
            } => AgendaMutation::Create {
                kind: crate::AgendaItemKind::Reminder,
                title: text.clone(),
                scheduled_at_unix_secs: Some(*due_at_unix),
                timezone: timezone.clone(),
            },
            SecretaryAction::RescheduleItem {
                item_id,
                expected_version,
                starts_at_unix,
                timezone,
            } => AgendaMutation::Reschedule {
                item_id: AgendaItemId::new(item_id.clone()).map_err(|error| {
                    EffectError::new(EffectErrorKind::Permanent, error.to_string())
                })?,
                expected_version: *expected_version,
                scheduled_at_unix_secs: *starts_at_unix,
                timezone: timezone.clone(),
            },
            SecretaryAction::CompleteItem {
                item_id,
                expected_version,
            } => AgendaMutation::Complete {
                item_id: AgendaItemId::new(item_id.clone()).map_err(|error| {
                    EffectError::new(EffectErrorKind::Permanent, error.to_string())
                })?,
                expected_version: *expected_version,
            },
            SecretaryAction::CancelItem {
                item_id,
                expected_version,
                ..
            } => AgendaMutation::Cancel {
                item_id: AgendaItemId::new(item_id.clone()).map_err(|error| {
                    EffectError::new(EffectErrorKind::Permanent, error.to_string())
                })?,
                expected_version: *expected_version,
            },
            SecretaryAction::SnoozeItem {
                item_id,
                expected_version,
                due_at_unix,
                timezone,
            } => AgendaMutation::Snooze {
                item_id: AgendaItemId::new(item_id.clone()).map_err(|error| {
                    EffectError::new(EffectErrorKind::Permanent, error.to_string())
                })?,
                expected_version: *expected_version,
                scheduled_at_unix_secs: *due_at_unix,
                timezone: timezone.clone(),
            },
            _ => return Ok(None),
        };
        let agenda = self
            .agenda
            .as_ref()
            .ok_or_else(|| EffectError::new(EffectErrorKind::Permanent, "AgendaUseCase 未注入"))?;
        let proposal_json = serde_json::to_string(proposal)
            .map_err(|error| EffectError::new(EffectErrorKind::Permanent, error.to_string()))?;
        let receipt = agenda
            .apply(&AgendaApplyRequest {
                account: self.account.clone(),
                command_source_event_id: self.command_source_event_id.clone().ok_or_else(|| {
                    EffectError::new(
                        EffectErrorKind::Permanent,
                        "Agenda action requires command source event identity",
                    )
                })?,
                run_id: self.run_id.as_str().to_owned(),
                effect_id: effect_id.to_owned(),
                proposal_id: proposal.proposal_id.clone(),
                proposal_json,
                lease_token: self.lease_token.as_str().to_owned(),
                idempotency_key: proposal.idempotency_key.clone().ok_or_else(|| {
                    EffectError::new(
                        EffectErrorKind::Permanent,
                        "Agenda action missing idempotency key",
                    )
                })?,
                mutation,
            })
            .await
            .map_err(agenda_effect_error)?;
        Ok(Some(SecretaryActionReceipt {
            proposal_id: proposal.proposal_id.clone(),
            result_ref: receipt.result_ref,
            tool_kind: Some(proposal.action.kind()),
        }))
    }
    async fn execute_notification_policy(
        &self,
        proposal: &crate::SecretaryActionProposal,
        effect_id: &str,
    ) -> Result<NotificationPolicyExecution, EffectError> {
        let Some(policy) = &self.notification_policy else {
            return Ok(NotificationPolicyExecution::NotHandled);
        };
        let Some(command_source_event_id) = &self.command_source_event_id else {
            return Err(EffectError::new(
                EffectErrorKind::Permanent,
                "通知策略 Action 缺少原始 OwnerCommand 身份",
            ));
        };
        match &proposal.action {
            SecretaryAction::ListNotificationPolicies { limit } => {
                let authorization = policy
                    .authorization_for_owner_command(&self.account, command_source_event_id)
                    .await
                    .map_err(policy_effect_error)?;
                let artifacts = policy
                    .list_authorized_policy_artifacts(&authorization, *limit)
                    .await
                    .map_err(policy_effect_error)?;
                let result_ref = serde_json::to_string(&artifacts).map_err(|error| {
                    EffectError::new(EffectErrorKind::Permanent, error.to_string())
                })?;
                Ok(NotificationPolicyExecution::ReadOnly(result_ref))
            }
            SecretaryAction::ExplainNotificationDecision { decision_id } => {
                let authorization = policy
                    .authorization_for_owner_command(&self.account, command_source_event_id)
                    .await
                    .map_err(policy_effect_error)?;
                let decision_id =
                    crate::NotificationDecisionId::new(decision_id.clone()).map_err(|error| {
                        EffectError::new(EffectErrorKind::Permanent, error.to_string())
                    })?;
                let artifact = policy
                    .explain_authorized_decision(&authorization, &decision_id)
                    .await
                    .map_err(policy_effect_error)?;
                let result_ref = serde_json::to_string(&artifact).map_err(|error| {
                    EffectError::new(EffectErrorKind::Permanent, error.to_string())
                })?;
                Ok(NotificationPolicyExecution::ReadOnly(result_ref))
            }
            action if is_mutable_notification_policy_action(action) => {
                let proposal_json = serde_json::to_string(proposal).map_err(|error| {
                    EffectError::new(EffectErrorKind::Permanent, error.to_string())
                })?;
                let request = NotificationPolicyEffectRequest {
                    account: self.account.clone(),
                    command_source_event_id: command_source_event_id.clone(),
                    run_id: self.run_id.as_str().to_owned(),
                    effect_id: effect_id.to_owned(),
                    proposal_id: proposal.proposal_id.clone(),
                    proposal_json,
                    lease_token: self.lease_token.as_str().to_owned(),
                    action: action.clone(),
                };
                let receipt = match policy
                    .authorization_for_owner_command(&self.account, command_source_event_id)
                    .await
                {
                    Ok(authorization) => policy.apply_effect(&authorization, &request).await,
                    // 历史回执没有新写入；预检授权漂移后仍必须由仓储的精确身份比较决定
                    // 是否回放，而不是在事务外提前拒绝。
                    Err(_) => policy.apply_effect_with_repository_fencing(&request).await,
                }
                .map_err(policy_effect_error)?;
                Ok(NotificationPolicyExecution::Mutable(receipt))
            }
            _ => Ok(NotificationPolicyExecution::NotHandled),
        }
    }

    /// 根据 Action 类型执行真实查询，返回结果摘要作为 result_ref。
    /// 查询工具返回结构化 JSON（`QueryEffectResultV1`），供 ReplanDecision 解析。
    /// 非查询工具保持纯文本 result_ref。
    async fn execute_action(&self, action: &SecretaryAction) -> Result<String, EffectError> {
        if let SecretaryAction::ListUpcomingItems { horizon_secs } = action {
            let agenda = self.agenda.as_ref().ok_or_else(|| {
                EffectError::new(EffectErrorKind::Permanent, "AgendaUseCase 未注入")
            })?;
            let items = agenda
                .list_upcoming(&self.account, *horizon_secs)
                .await
                .map_err(|error| EffectError::new(EffectErrorKind::Transient, error.to_string()))?;
            let summary = format!("查到 {} 个即将到期事项", items.len());
            return query_effect_json(
                SecretaryToolKind::ListUpcomingItems,
                &summary,
                &[],
                Vec::new(),
            );
        }
        let retriever = self.retriever.as_ref().ok_or_else(|| {
            EffectError::new(
                EffectErrorKind::Permanent,
                "Retriever 未注入，无法执行查询型 Action",
            )
        })?;
        match action {
            SecretaryAction::ListMemoryCandidates {
                status,
                kind,
                limit,
            } => {
                let memory_candidate = self.memory_candidate.as_ref().ok_or_else(|| {
                    EffectError::new(
                        EffectErrorKind::Permanent,
                        "MemoryCandidateUseCase 未注入，无法列出记忆候选",
                    )
                })?;
                let views = memory_candidate
                    .list(&self.account, *status, *kind, u32::from(*limit))
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                let summary = format_memory_candidates(&views);
                query_effect_json(
                    SecretaryToolKind::ListMemoryCandidates,
                    &summary,
                    &[],
                    Vec::new(),
                )
            }
            SecretaryAction::ListProjects { limit } => {
                let summaries = retriever
                    .list_projects(&self.account, *limit)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                let summary = format_project_list(&summaries);
                query_effect_json(SecretaryToolKind::ListProjects, &summary, &[], Vec::new())
            }
            SecretaryAction::QueryProject { project_key } => {
                let view = retriever
                    .query_project(&self.account, project_key)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                let (summary, source_ids) = match view {
                    Some(ref v) => (format_project_detail(v), v.source_event_ids.clone()),
                    None => (
                        format!("未找到项目「{project_key}」或来源已失效").to_owned(),
                        Vec::new(),
                    ),
                };
                query_effect_json(
                    SecretaryToolKind::QueryProject,
                    &summary,
                    &source_ids,
                    Vec::new(),
                )
            }
            SecretaryAction::ListCommitments {
                status,
                due_since_unix_secs,
                due_until_unix_secs,
                promisor,
                beneficiary,
                limit,
            } => {
                let query = crate::CommitmentQuery {
                    account: self.account.clone(),
                    status: *status,
                    due_since_unix_secs: *due_since_unix_secs,
                    due_until_unix_secs: *due_until_unix_secs,
                    promisor: promisor.clone(),
                    beneficiary: beneficiary.clone(),
                    limit: *limit,
                };
                let items = retriever
                    .list_commitments(&query)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                let summary = format_commitment_list(&items);
                // 收集所有承诺的精确来源引用（去重、有界），供后续证据回读。
                const MAX_SOURCE_IDS: usize = 20;
                let mut source_ids: Vec<SourceEventId> = Vec::new();
                for item in &items {
                    for sid in &item.source_event_ids {
                        if source_ids.len() >= MAX_SOURCE_IDS {
                            break;
                        }
                        if !source_ids.contains(sid) {
                            source_ids.push(sid.clone());
                        }
                    }
                }
                query_effect_json(
                    SecretaryToolKind::ListCommitments,
                    &summary,
                    &source_ids,
                    Vec::new(),
                )
            }
            // CMD-009 目标 B：SearchRecentEvents 名称保留以兼容旧序列化，语义已扩展为
            // 有界事件搜索。未指定 since 时允许检索 24 小时以前的长期事件（不暗中补
            // 24h 下限）；显式 conversation/thread/actor 是硬过滤；until 不得无理由
            // 越过可信当前时间（允许 60 秒时钟偏差）。
            SecretaryAction::SearchRecentEvents {
                query,
                limit,
                since_unix_secs,
                until_unix_secs,
                conversation,
                thread_id,
                actor_id,
            } => {
                if until_unix_secs.is_some_and(|until| until > self.now_unix_secs + 60) {
                    return Err(EffectError::new(
                        EffectErrorKind::Permanent,
                        "search until_unix_secs must not exceed the trusted current time",
                    ));
                }
                let event_query = EventQuery {
                    account: self.account.clone(),
                    conversation: conversation.clone(),
                    actor_id: actor_id.clone(),
                    thread_id: thread_id.clone(),
                    since_unix_secs: *since_unix_secs,
                    until_unix_secs: *until_unix_secs,
                    query_text: Some(query.clone()),
                    limit: *limit,
                };
                let results = retriever
                    .search_events(&event_query, self.is_local_loopback)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                let event_ids: Vec<SourceEventId> =
                    results.iter().map(|r| r.source_event_id.clone()).collect();
                let summary = format_event_results(&results);
                let typed_events: Vec<QueryEffectTypedEvent> = results
                    .iter()
                    .map(|r| QueryEffectTypedEvent {
                        source_event_id: r.source_event_id.clone(),
                        actor_id: r.actor.id.clone(),
                        actor_kind: crate::PlatformIdentityKind::from_verified_actor_kind(
                            r.actor.kind,
                        ),
                        occurred_at_unix_secs: r.occurred_at_unix_secs,
                        excerpt: r.excerpt.chars().take(120).collect(),
                    })
                    .collect();
                query_effect_json(
                    SecretaryToolKind::SearchRecentEvents,
                    &summary,
                    &event_ids,
                    typed_events,
                )
            }
            SecretaryAction::ReadSourceEvent { source_event_id } => {
                let detail = retriever
                    .read_source_event(source_event_id, &self.account)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                let (summary, event_ids, typed_events) = match detail {
                    Some(ref d) => (
                        format!(
                            "事件 {} | {} | {} | 摘录: {}",
                            d.source_event_id.as_str(),
                            d.actor.id,
                            d.occurred_at_unix_secs,
                            d.normalized_text.chars().take(120).collect::<String>(),
                        ),
                        vec![d.source_event_id.clone()],
                        vec![QueryEffectTypedEvent {
                            source_event_id: d.source_event_id.clone(),
                            actor_id: d.actor.id.clone(),
                            actor_kind: crate::PlatformIdentityKind::from_verified_actor_kind(
                                d.actor.kind,
                            ),
                            occurred_at_unix_secs: d.occurred_at_unix_secs,
                            excerpt: d.normalized_text.chars().take(120).collect(),
                        }],
                    ),
                    None => (
                        format!("未找到事件 {}", source_event_id.as_str()),
                        vec![],
                        vec![],
                    ),
                };
                query_effect_json(
                    SecretaryToolKind::ReadSourceEvent,
                    &summary,
                    &event_ids,
                    typed_events,
                )
            }
            SecretaryAction::SearchEventThreads { query, limit } => {
                let results = retriever
                    .search_threads(&self.account, query, *limit)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                let summary = format!("搜索到 {} 个线程", results.len());
                query_effect_json(
                    SecretaryToolKind::SearchEventThreads,
                    &summary,
                    &[],
                    Vec::new(),
                )
            }
            SecretaryAction::ResolveReference {
                expression,
                conversation_ref,
                thread_id,
            } => {
                // CMD-010 防线 C：解析作用域只来自 Owner 显式提供的已登记
                // conversation_ref / thread_ref；无作用域时用例层不返回候选，
                // 回执携带歧义标记，供 ReplanDecision 登记 OpenReference。
                let context = ReferenceContext {
                    account: self.account.clone(),
                    current_conversation: conversation_ref.clone(),
                    current_thread_id: thread_id.clone(),
                    recent_events: Vec::new(),
                    now_unix_secs: self.now_unix_secs,
                    timezone: "UTC".into(),
                };
                let resolution = retriever
                    .resolve_reference(&self.account, expression, &context)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                let summary = if resolution.ambiguous {
                    format!("指代歧义：{}", resolution.evidence)
                } else {
                    format!("指代已解析：{}", resolution.evidence)
                };
                // 唯一解析只把一条权威来源投影为 typed_event；稳定事件/Actor ID
                // 由下一轮 LLM 适配层替换为 evt_N/actor_N。歧义时不暴露候选集合，
                // 仅登记 OpenReference 并强制 Owner 澄清。
                let mut source_event_ids = Vec::new();
                let mut typed_events = Vec::new();
                if !resolution.ambiguous
                    && let Some(event_id) = resolution.resolved_event_ids.first()
                    && let Some(detail) = retriever
                        .read_source_event(event_id, &self.account)
                        .await
                        .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?
                {
                    source_event_ids.push(detail.source_event_id.clone());
                    typed_events.push(QueryEffectTypedEvent {
                        source_event_id: detail.source_event_id,
                        actor_id: detail.actor.id,
                        actor_kind: crate::PlatformIdentityKind::from_verified_actor_kind(
                            detail.actor.kind,
                        ),
                        occurred_at_unix_secs: detail.occurred_at_unix_secs,
                        // 指代解析只需要身份与来源，不把正文再次送入模型。
                        excerpt: String::new(),
                    });
                }
                // 歧义标记进入回执，供 ReplanDecision 登记未解决指代（CMD-009 目标 A）。
                query_effect_json_ambiguous(
                    SecretaryToolKind::ResolveReference,
                    &summary,
                    &source_event_ids,
                    typed_events,
                    resolution.ambiguous,
                )
            }
            SecretaryAction::GetSecretaryStatus => {
                let status = retriever
                    .secretary_status(&self.account)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                let summary = format_secretary_status(&status);
                query_effect_json(
                    SecretaryToolKind::GetSecretaryStatus,
                    &summary,
                    &[],
                    Vec::new(),
                )
            }
            SecretaryAction::ListPendingOwnerWork { limit } => {
                let items = retriever
                    .list_pending_owner_work(&self.account, *limit)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                let summary = format_pending_owner_work(&items);
                query_effect_json(
                    SecretaryToolKind::ListPendingOwnerWork,
                    &summary,
                    &[],
                    Vec::new(),
                )
            }
            SecretaryAction::GetThreadContext { thread_id } => {
                let context = retriever
                    .thread_context(&self.account, thread_id)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                let summary = match context {
                    Some(ref ctx) => format_thread_context(ctx),
                    None => format!("未找到当前账号下的线程 {}", thread_id.as_str()),
                };
                query_effect_json(
                    SecretaryToolKind::GetThreadContext,
                    &summary,
                    &[],
                    Vec::new(),
                )
            }
            SecretaryAction::GetEventCausalContext { source_event_id } => {
                let view = retriever
                    .event_causal_context(&self.account, source_event_id)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                let Some(view) = view else {
                    return query_effect_json(
                        SecretaryToolKind::GetEventCausalContext,
                        &format!("未找到当前账号下的事件 {}", source_event_id.as_str()),
                        &[],
                        Vec::new(),
                    );
                };
                let (summary, event_ids, typed_events) = format_causal_context(&view);
                query_effect_json(
                    SecretaryToolKind::GetEventCausalContext,
                    &summary,
                    &event_ids,
                    typed_events,
                )
            }
            SecretaryAction::GetParticipantContext {
                actor_kind,
                actor_id,
                conversation_ref,
                thread_id,
            } => {
                // 完整三元组身份（账号 + 身份种类 + 稳定 ID）从 actor_ref 恢复；
                // 上下文按三元组精确读取，不触发宽松查询的跨命名空间歧义拒绝。
                let participant_ref = AccountScopedParticipantRef::new(
                    self.account.clone(),
                    *actor_kind,
                    actor_id.clone(),
                    crate::IdentityTrust::Observed,
                )
                .map_err(|e| EffectError::new(EffectErrorKind::Permanent, format!("{e}")))?;
                let view = retriever
                    .participant_context_by_ref(
                        &participant_ref,
                        conversation_ref.as_ref(),
                        thread_id.as_ref(),
                    )
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                let Some(view) = view else {
                    return query_effect_json(
                        SecretaryToolKind::GetParticipantContext,
                        &format!("未找到当前账号内参与者 {actor_id} 的证据"),
                        &[],
                        Vec::new(),
                    );
                };
                let (summary, event_ids, typed_events) = format_participant_context(&view);
                query_effect_json(
                    SecretaryToolKind::GetParticipantContext,
                    &summary,
                    &event_ids,
                    typed_events,
                )
            }
            SecretaryAction::GetParticipantContextByName {
                name,
                conversation_ref,
                thread_id,
            } => {
                // 复合查询：先按显示名/别名/群名片有界解析（账号作用域），唯一候选再
                // 读取完整上下文；零候选/多候选返回有界摘要，不投影稳定 ID。
                let candidates = retriever
                    .participants_by_display_name(
                        &self.account,
                        name,
                        conversation_ref.as_ref(),
                        thread_id.as_ref(),
                        5,
                    )
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                if candidates.is_empty() {
                    return query_effect_json(
                        SecretaryToolKind::GetParticipantContextByName,
                        &format!("未找到显示名或别名匹配「{name}」的参与者"),
                        &[],
                        Vec::new(),
                    );
                }
                if candidates.len() == 1 {
                    // 候选是完整账号作用域参与者引用（含身份种类）：上下文按三元组
                    // 精确读取，同 ID 不同身份命名空间并存时不会触发歧义拒绝。
                    let actor_id = candidates[0].stable_id().to_owned();
                    let view = retriever
                        .participant_context_by_ref(
                            &candidates[0],
                            conversation_ref.as_ref(),
                            thread_id.as_ref(),
                        )
                        .await
                        .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                    let Some(view) = view else {
                        return query_effect_json(
                            SecretaryToolKind::GetParticipantContextByName,
                            &format!("已解析到参与者 {actor_id}，但当前账号内无上下文证据"),
                            &[],
                            Vec::new(),
                        );
                    };
                    let (summary, event_ids, typed_events) = format_participant_context(&view);
                    return query_effect_json(
                        SecretaryToolKind::GetParticipantContextByName,
                        &summary,
                        &event_ids,
                        typed_events,
                    );
                }
                let summary = format_participant_candidates(&candidates);
                query_effect_json(
                    SecretaryToolKind::GetParticipantContextByName,
                    &summary,
                    &[],
                    Vec::new(),
                )
            }
            SecretaryAction::ListUpcomingItems { .. } => Err(EffectError::new(
                EffectErrorKind::Permanent,
                "ListUpcomingItems 应由 AgendaUseCase 执行",
            )),
            SecretaryAction::DraftReminder { text, .. } => Ok(format!(
                "已起草提醒：{}",
                text.chars().take(50).collect::<String>()
            )),
            SecretaryAction::AskOwnerClarification { question } => Ok(format!(
                "已向 Owner 提问：{}",
                question.chars().take(50).collect::<String>()
            )),
            other => Err(EffectError::new(
                EffectErrorKind::Permanent,
                format!("本批不支持执行 Action: {:?}", other.kind()),
            )),
        }
    }
}

#[async_trait]
impl EffectExecutor<SecretaryActionEffect> for SecretaryActionEffectExecutor {
    async fn execute(
        &self,
        envelope: &EffectEnvelope<SecretaryActionEffect>,
        _context: &RunContext,
    ) -> Result<SecretaryActionReceipt, EffectError> {
        let tool_kind = envelope.effect.proposal.action.kind();
        // 可变策略 Action 的 receipt 由策略仓储校验 run、proposal 与完整 Action，不能用
        // 通用 store 仅按 (run_id, effect_id) 的快速读取绕过碰撞检查。
        let is_mutable_policy =
            is_mutable_notification_policy_action(&envelope.effect.proposal.action);
        let is_thread_control = is_thread_control_action(&envelope.effect.proposal.action);
        let is_owner_work_control = is_owner_work_control_action(&envelope.effect.proposal.action);
        let is_memory_candidate_control =
            is_memory_candidate_control_action(&envelope.effect.proposal.action);
        if !is_mutable_policy
            && !is_thread_control
            && !is_owner_work_control
            && !is_memory_candidate_control
            && let Some(mut receipt) = self
                .store
                .load_effect_receipt(&self.run_id, &envelope.id.to_string())
                .await
                .map_err(ActionStoreError::to_effect_error)?
        {
            receipt.tool_kind = Some(tool_kind);
            return Ok(receipt);
        }
        match self
            .execute_notification_policy(&envelope.effect.proposal, &envelope.id.to_string())
            .await?
        {
            NotificationPolicyExecution::ReadOnly(result_ref) => {
                let mut receipt = self
                    .store
                    .apply_effect(
                        &self.run_id,
                        &envelope.effect,
                        &envelope.id.to_string(),
                        &result_ref,
                        &self.lease_token,
                    )
                    .await
                    .map_err(ActionStoreError::to_effect_error)?;
                receipt.tool_kind = Some(tool_kind);
                return Ok(receipt);
            }
            NotificationPolicyExecution::Mutable(mut receipt) => {
                receipt.tool_kind = Some(tool_kind);
                return Ok(receipt);
            }
            NotificationPolicyExecution::NotHandled => {}
        }
        if let Some(mut receipt) = self
            .execute_thread_control(&envelope.effect.proposal, &envelope.id.to_string())
            .await?
        {
            receipt.tool_kind = Some(tool_kind);
            return Ok(receipt);
        }
        if let Some(mut receipt) = self
            .execute_follow_up_control(&envelope.effect.proposal, &envelope.id.to_string())
            .await?
        {
            receipt.tool_kind = Some(tool_kind);
            return Ok(receipt);
        }
        if let Some(mut receipt) = self
            .execute_response_expectation_control(
                &envelope.effect.proposal,
                &envelope.id.to_string(),
            )
            .await?
        {
            receipt.tool_kind = Some(tool_kind);
            return Ok(receipt);
        }
        if let Some(mut receipt) = self
            .execute_memory_candidate_control(&envelope.effect.proposal, &envelope.id.to_string())
            .await?
        {
            receipt.tool_kind = Some(tool_kind);
            return Ok(receipt);
        }
        if let Some(mut receipt) = self
            .execute_agenda(&envelope.effect.proposal, &envelope.id.to_string())
            .await?
        {
            receipt.tool_kind = Some(tool_kind);
            return Ok(receipt);
        }
        if let Some(result_ref) = self
            .execute_memory(&envelope.effect.proposal.action, &envelope.id.to_string())
            .await?
        {
            let mut receipt = self
                .store
                .apply_effect(
                    &self.run_id,
                    &envelope.effect,
                    &envelope.id.to_string(),
                    &result_ref,
                    &self.lease_token,
                )
                .await
                .map_err(ActionStoreError::to_effect_error)?;
            receipt.tool_kind = Some(tool_kind);
            return Ok(receipt);
        }
        // 未命中既有 Receipt 才执行真实只读 Action；Store 提交时再次处理并发竞争。
        let result_ref = self
            .execute_action(&envelope.effect.proposal.action)
            .await?;
        let mut receipt = self
            .store
            .apply_effect(
                &self.run_id,
                &envelope.effect,
                &envelope.id.to_string(),
                &result_ref,
                &self.lease_token,
            )
            .await
            .map_err(|e| e.to_effect_error())?;
        receipt.tool_kind = Some(tool_kind);
        Ok(receipt)
    }
}

fn policy_effect_error(error: crate::NotificationPolicyUseCaseError) -> EffectError {
    let kind = match error {
        crate::NotificationPolicyUseCaseError::Store(
            crate::NotificationPolicyStoreError::Unavailable
            | crate::NotificationPolicyStoreError::Database,
        ) => EffectErrorKind::Transient,
        _ => EffectErrorKind::Permanent,
    };
    EffectError::new(kind, error.to_string())
}

fn agenda_effect_error(error: AgendaError) -> EffectError {
    let kind = match error {
        AgendaError::Database(_) => EffectErrorKind::UnknownCommit,
        AgendaError::Invalid(_)
        | AgendaError::NotFound
        | AgendaError::VersionConflict
        | AgendaError::Unauthorized
        | AgendaError::LeaseLost
        | AgendaError::Store(_) => EffectErrorKind::Permanent,
    };
    EffectError::new(kind, error.to_string())
}

fn memory_effect_error(error: crate::MemoryUseCaseError) -> EffectError {
    let kind = match error {
        crate::MemoryUseCaseError::Store(crate::InboundEventStoreError::Database(_)) => {
            EffectErrorKind::Transient
        }
        _ => EffectErrorKind::Permanent,
    };
    EffectError::new(kind, error.to_string())
}

fn thread_control_effect_error(error: ThreadControlStoreError) -> EffectError {
    let kind = match error {
        ThreadControlStoreError::Database => EffectErrorKind::UnknownCommit,
        ThreadControlStoreError::LeaseLost
        | ThreadControlStoreError::Unauthorized
        | ThreadControlStoreError::InvalidData(_) => EffectErrorKind::Permanent,
    };
    EffectError::new(kind, error.to_string())
}

fn follow_up_control_effect_error(error: FollowUpControlStoreError) -> EffectError {
    let kind = match error {
        FollowUpControlStoreError::Database => EffectErrorKind::UnknownCommit,
        FollowUpControlStoreError::LeaseLost
        | FollowUpControlStoreError::Unauthorized
        | FollowUpControlStoreError::InvalidData(_) => EffectErrorKind::Permanent,
    };
    EffectError::new(kind, error.to_string())
}

fn response_expectation_control_effect_error(
    error: ResponseExpectationControlStoreError,
) -> EffectError {
    let kind = match error {
        ResponseExpectationControlStoreError::Database => EffectErrorKind::UnknownCommit,
        ResponseExpectationControlStoreError::LeaseLost
        | ResponseExpectationControlStoreError::Unauthorized
        | ResponseExpectationControlStoreError::InvalidData(_) => EffectErrorKind::Permanent,
    };
    EffectError::new(kind, error.to_string())
}

/// 记忆候选控制错误映射：数据库失败可能已提交，必须按 UnknownCommit 回放复验；
/// 授权/租约/数据均确定性失败，不重试不重放。内容冲突不是错误——它作为
/// `ApproveConflict` 业务结果写入审计与 Receipt，Owner 会收到含旧 Fact ID 与
/// Candidate ID 的冲突响应，而不是 Run 失败。
fn memory_candidate_control_effect_error(error: MemoryCandidateControlStoreError) -> EffectError {
    let kind = match error {
        MemoryCandidateControlStoreError::Database => EffectErrorKind::UnknownCommit,
        MemoryCandidateControlStoreError::Unauthorized
        | MemoryCandidateControlStoreError::LeaseLost
        | MemoryCandidateControlStoreError::InvalidData(_) => EffectErrorKind::Permanent,
    };
    EffectError::new(kind, error.to_string())
}

fn deterministic_memory_fact_id(effect_id: &str) -> Result<MemoryFactId, EffectError> {
    MemoryFactId::new(
        uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_OID,
            format!("memory-effect:{effect_id}").as_bytes(),
        )
        .to_string(),
    )
    .map_err(|error| EffectError::new(EffectErrorKind::Permanent, error.to_string()))
}

fn ensure_active_memory_for_account(
    fact: &MemoryFact,
    account: &SourceAccountRef,
) -> Result<(), EffectError> {
    if &fact.account != account {
        return Err(EffectError::new(
            EffectErrorKind::Permanent,
            "不能跨账号修改记忆",
        ));
    }
    if !matches!(
        fact.status,
        MemoryFactStatus::Proposed | MemoryFactStatus::Confirmed
    ) {
        return Err(EffectError::new(
            EffectErrorKind::Permanent,
            "只能修改当前有效的记忆版本",
        ));
    }
    Ok(())
}

fn format_memory_facts(facts: &[MemoryFact]) -> String {
    if facts.is_empty() {
        return "当前没有有效的长期记忆".into();
    }
    let mut output = format!("当前有 {} 条有效记忆：", facts.len());
    for fact in facts.iter().take(10) {
        let item = format!(
            "\n{} | {} | {}",
            fact.fact_id.as_str(),
            fact.payload.kind(),
            fact.subject_key
        );
        if output.chars().count() + item.chars().count() > 900 {
            output.push_str("\n其余记忆已省略");
            break;
        }
        output.push_str(&item);
    }
    output
}

fn format_memory_evidence(view: &crate::MemoryFactView) -> String {
    let mut output = format!(
        "记忆 {}（{} / {}）来源：",
        view.fact.fact_id.as_str(),
        view.fact.payload.kind(),
        view.fact.subject_key
    );
    for source in view.sources.iter().take(5) {
        let excerpt = source.excerpt.chars().take(100).collect::<String>();
        let item = format!(
            "\n{} | {} | {}",
            source.source_event_id.as_str(),
            source.actor_id,
            excerpt
        );
        if output.chars().count() + item.chars().count() > 900 {
            output.push_str("\n其余来源已省略");
            break;
        }
        output.push_str(&item);
    }
    output
}

/// 格式化记忆候选列表为有界中文摘要：ID/kind/status/version/subject/
/// 精简 payload/来源条数/冲突标记；不回显完整聊天正文。
fn format_memory_candidates(views: &[crate::MemoryCandidateView]) -> String {
    if views.is_empty() {
        return "当前没有可展示的记忆候选".into();
    }
    let mut output = format!("当前有 {} 条记忆候选：", views.len());
    for view in views.iter().take(8) {
        let conflict = if view.conflicts_with_active_fact {
            "有冲突"
        } else {
            "无冲突"
        };
        let payload_excerpt = serde_json::to_string(&view.payload)
            .unwrap_or_default()
            .chars()
            .take(80)
            .collect::<String>();
        let item = format!(
            "\n{} | {} | {} | v{} | {} | payload {} | {conflict} | 来源 {}",
            view.candidate_id.as_str(),
            view.kind.as_str(),
            view.status.as_str(),
            view.version.as_u64(),
            view.subject_key.chars().take(60).collect::<String>(),
            payload_excerpt,
            view.source_excerpts.len(),
        );
        if output.chars().count() + item.chars().count() > 900 {
            output.push_str("\n其余候选已省略");
            break;
        }
        output.push_str(&item);
    }
    output
}

/// 格式化项目列表摘要（MEM-003）。
fn format_project_list(summaries: &[crate::ProjectMemorySummary]) -> String {
    if summaries.is_empty() {
        return "当前没有活跃的项目记忆".into();
    }
    let mut output = format!("当前有 {} 个项目：", summaries.len());
    for (i, s) in summaries.iter().enumerate().take(10) {
        let progress = s.progress.as_deref().unwrap_or("无");
        let item = format!(
            "\n{}. {} | 目标: {} | 成员 {} | 风险 {} | 阻塞 {} | {}",
            i + 1,
            s.project_key,
            s.goal,
            s.member_count,
            s.risk_count,
            s.blocker_count,
            progress,
        );
        if output.chars().count() + item.chars().count() > 1500 {
            output.push_str("\n其余项目已省略");
            break;
        }
        output.push_str(&item);
    }
    output
}

/// 格式化单个项目详情（MEM-003）。
fn format_project_detail(view: &crate::ProjectContextView) -> String {
    let mut output = format!("项目「{}」\n目标: {}\n", view.project_key, view.goal);
    if !view.members.is_empty() {
        output.push_str("成员: ");
        let member_strs: Vec<String> = view
            .members
            .iter()
            .map(|m| {
                let kind_label = match m.platform_identity_kind {
                    Some(kind) => kind.as_str(),
                    None => "未知身份",
                };
                format!("{}({kind_label})", m.actor_id)
            })
            .collect();
        output.push_str(&member_strs.join(", "));
        output.push('\n');
        if view.legacy_member_ids {
            output.push_str("（成员身份类型来自旧数据，显示为未知）\n");
        }
    }
    if let Some(progress) = &view.progress {
        output.push_str(&format!("进展: {progress}\n"));
    }
    if !view.risks.is_empty() {
        output.push_str(&format!("风险: {}\n", view.risks.join("; ")));
    }
    if !view.blockers.is_empty() {
        output.push_str(&format!("阻塞: {}\n", view.blockers.join("; ")));
    }
    if !view.decision_ids.is_empty() {
        output.push_str(&format!("决策数: {}\n", view.decision_ids.len()));
    }
    if !view.artifact_refs.is_empty() {
        output.push_str(&format!("产出物: {}\n", view.artifact_refs.join(", ")));
    }
    output.push_str(&format!("记忆版本: {}", view.fact_id.as_str()));
    output
}

/// 格式化承诺列表摘要（MEM-004 B2）。
fn format_commitment_list(items: &[crate::CommitmentSummary]) -> String {
    if items.is_empty() {
        return "未找到匹配的承诺".into();
    }
    let mut output = format!("找到 {} 条承诺：", items.len());
    for (i, c) in items.iter().enumerate().take(10) {
        let due = c
            .due_at_unix_secs
            .map(|ts| format!("{}", ts))
            .unwrap_or_else(|| "无截止".into());
        let fu = c.follow_up_id.as_deref().unwrap_or("无跟进");
        let item = format!(
            "\n{}. {} → {} @ {due} | {} | 跟进: {fu}",
            i + 1,
            c.promisor.actor_id,
            c.beneficiary.actor_id,
            c.status.as_str(),
        );
        if output.chars().count() + item.chars().count() > 1500 {
            output.push_str("\n其余承诺已省略");
            break;
        }
        output.push_str(&item);
    }
    output
}

enum NotificationPolicyExecution {
    NotHandled,
    ReadOnly(String),
    Mutable(SecretaryActionReceipt),
}

fn is_mutable_notification_policy_action(action: &SecretaryAction) -> bool {
    matches!(
        action,
        SecretaryAction::SetAccountDefaultNotificationMode { .. }
            | SecretaryAction::SetConversationNotificationMode { .. }
            | SecretaryAction::SetQuietHours { .. }
            | SecretaryAction::SetImportantContact { .. }
            | SecretaryAction::SetNotificationCategoryImportance { .. }
            | SecretaryAction::RecordNotificationFeedback { .. }
            | SecretaryAction::CreateSimilarNotificationRule { .. }
            | SecretaryAction::DisableNotificationPolicy { .. }
            | SecretaryAction::SetAutomaticReplyDeniedForContact { .. }
    )
}

fn is_thread_control_action(action: &SecretaryAction) -> bool {
    matches!(
        action,
        SecretaryAction::ConfirmThreadDecision { .. }
            | SecretaryAction::RevokeThreadDecision { .. }
            | SecretaryAction::DismissThreadQuestion { .. }
            | SecretaryAction::SetThreadLifecycle { .. }
    )
}

fn is_follow_up_control_action(action: &SecretaryAction) -> bool {
    matches!(
        action,
        SecretaryAction::DismissFollowUp { .. }
            | SecretaryAction::SnoozeFollowUp { .. }
            | SecretaryAction::DismissFollowUps { .. }
            | SecretaryAction::SnoozeFollowUps { .. }
            | SecretaryAction::CompleteFollowUp { .. }
            | SecretaryAction::CompleteFollowUps { .. }
    )
}

fn is_response_expectation_control_action(action: &SecretaryAction) -> bool {
    matches!(
        action,
        SecretaryAction::DismissResponseExpectation { .. }
            | SecretaryAction::DismissResponseExpectations { .. }
    )
}

fn is_memory_candidate_control_action(action: &SecretaryAction) -> bool {
    matches!(
        action,
        SecretaryAction::ApproveMemoryCandidate { .. }
            | SecretaryAction::RejectMemoryCandidate { .. }
    )
}

/// 所有 Owner 工作控制动作；命中时跳过通用 store 的快速回执读取，
/// 改由控制仓储做 run/proposal/完整 Action 碰撞校验。
fn is_owner_work_control_action(action: &SecretaryAction) -> bool {
    is_follow_up_control_action(action)
        || is_response_expectation_control_action(action)
        || is_memory_candidate_control_action(action)
}

/// 格式化事件检索结果为有界摘要（含来源、时间、Actor、摘录、命中数）。
/// 最多展示前 3 条事件详情，超过时标记截断。
fn format_event_results(results: &[crate::EventSearchResult]) -> String {
    let total = results.len();
    if total == 0 {
        return "未找到匹配事件".into();
    }
    let show = total.min(3);
    let parts: Vec<String> = results[..show]
        .iter()
        .map(|r| {
            format!(
                "{} | {} | {}",
                r.source_event_id.as_str(),
                r.actor.id,
                r.excerpt.chars().take(80).collect::<String>()
            )
        })
        .collect();
    let truncation = if total > show {
        format!("，仅展示前 {show} 条")
    } else {
        String::new()
    };
    format!("命中 {total} 条{truncation}: {}", parts.join("; "))
}

fn format_secretary_status(status: &crate::SecretaryStatusView) -> String {
    let continuity = if status.unresolved_gap_count == 0 {
        "无未闭合空窗".to_owned()
    } else {
        format!(
            "{} 个未闭合空窗（其中 {} 个仍开放，最早起点 {:?}）",
            status.unresolved_gap_count,
            status.open_gap_count,
            status.earliest_gap_started_at_unix_secs
        )
    };
    format!(
        "连续性：{continuity}；线程：开放 {}、等待 {}；待办：回复期待 {}、跟进 {}；通知：待求值 {}、待投递 {}、异常 {}",
        status.open_thread_count,
        status.waiting_thread_count,
        status.active_response_expectation_count,
        status.scheduled_follow_up_count,
        status.pending_evaluation_count,
        status.pending_outbox_count,
        status.failed_outbox_count,
    )
}

fn format_pending_owner_work(items: &[crate::PendingOwnerWorkItem]) -> String {
    if items.is_empty() {
        return "当前没有需要 Owner 处理的事项".into();
    }
    let mut output = format!("当前有 {} 项待处理：", items.len());
    for item in items.iter().take(8) {
        // 仅在存在来源版本时展示，无版本事项不伪造版本（如 outbox）。
        let version_prefix = item
            .source_version
            .map(|version| format!("version {version} | "))
            .unwrap_or_default();
        let line = format!(
            "\n{}:{} | {}{} | 到期 {:?} | {}",
            item.source_kind,
            item.source_id,
            version_prefix,
            item.status,
            item.due_at_unix_secs,
            item.summary.chars().take(80).collect::<String>()
        );
        if output.chars().count() + line.chars().count() > 900 {
            output.push_str("\n其余事项已省略");
            break;
        }
        output.push_str(&line);
    }
    output
}

fn format_thread_context(context: &crate::ThreadContextView) -> String {
    let actors = context
        .actors
        .iter()
        .take(5)
        .map(|actor| {
            format!(
                "{}:{}({})",
                actor.actor_kind, actor.actor_id, actor.event_count
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let mut output = format!(
        "线程 {} | {:?} | {} 条事件 | 参与者 [{}]",
        context.thread_id.as_str(),
        context.status,
        context.event_count,
        actors
    );
    for claim in context.claims.iter().take(3) {
        let line = format!(
            "\n要求/意见 {}:{}:{} | {}",
            claim.claim_kind,
            claim.claimant_actor_id,
            claim.status,
            claim.statement.chars().take(80).collect::<String>()
        );
        append_bounded_line(&mut output, &line);
    }
    for decision in context.decisions.iter().take(3) {
        let line = format!(
            "\n结论 {}:{} | {}",
            decision.decision_id,
            decision.status,
            decision.statement.chars().take(80).collect::<String>()
        );
        append_bounded_line(&mut output, &line);
    }
    for question in context.open_questions.iter().take(3) {
        let line = format!(
            "\n未决 {}:{} | {}",
            question.question_id,
            question.raised_by_actor_id,
            question.question.chars().take(80).collect::<String>()
        );
        append_bounded_line(&mut output, &line);
    }
    output
}

/// 格式化事件因果上下文（THR-011/THR-012）。summary 为安全中文（显示名优先），
/// 稳定 ID 只通过 typed_events 交给投影层映射为临时引用，绝不直接发给 LLM。
fn format_causal_context(
    view: &crate::EventCausalContextView,
) -> (String, Vec<SourceEventId>, Vec<QueryEffectTypedEvent>) {
    let mut output = String::new();
    let mut typed_events: Vec<QueryEffectTypedEvent> = Vec::new();
    let mut push_typed = |actor_id: String,
                          actor_kind: crate::PlatformIdentityKind,
                          source_event_id: SourceEventId,
                          excerpt: &str| {
        if typed_events.len() < 20 {
            typed_events.push(QueryEffectTypedEvent {
                source_event_id,
                actor_id,
                actor_kind,
                occurred_at_unix_secs: 0,
                excerpt: excerpt.chars().take(120).collect(),
            });
        }
    };

    if let Some(ref sender) = view.sender {
        let name = sender
            .display_name
            .clone()
            .unwrap_or_else(|| "（无显示名）".into());
        let line = format!(
            "\n发送者: {}（{}）",
            name.chars().take(40).collect::<String>(),
            sender.platform_kind.as_str()
        );
        append_bounded_line(&mut output, &line);
        push_typed(
            sender.stable_id.clone(),
            sender.platform_kind,
            view.source_event_id.clone(),
            "事件发送者",
        );
    }
    if let Some(ref parent) = view.reply_parent {
        let name = parent
            .sender
            .as_ref()
            .and_then(|s| s.display_name.clone())
            .unwrap_or_else(|| "（无显示名）".into());
        let line = format!("\n回复对象: {}", name.chars().take(40).collect::<String>());
        append_bounded_line(&mut output, &line);
        if let Some(ref sender) = parent.sender {
            push_typed(
                sender.stable_id.clone(),
                sender.platform_kind,
                parent.source_event_id.clone(),
                "被回复的父事件发送者",
            );
        }
    }
    if let Some(ref thread) = view.thread {
        let line = format!(
            "\n所属线程: {}（{:?}，{} 条事件）",
            thread.thread_id.as_str(),
            thread.status,
            view.participants.len()
        );
        append_bounded_line(&mut output, &line);
        if let Some(ref root) = thread.root_sender {
            let name = root
                .display_name
                .clone()
                .unwrap_or_else(|| "（无显示名）".into());
            let root_line = format!(
                "\n线程发起人: {}",
                name.chars().take(40).collect::<String>()
            );
            append_bounded_line(&mut output, &root_line);
            push_typed(
                root.stable_id.clone(),
                root.platform_kind,
                thread.root_event_id.clone(),
                "线程发起人（根事件发送者）",
            );
        }
    }
    for participant in &view.mentioned {
        let line = format!(
            "\n被@参与者: {}",
            participant
                .identity
                .display_name
                .clone()
                .unwrap_or_else(|| participant.stable_id().chars().take(20).collect())
                .chars()
                .take(40)
                .collect::<String>()
        );
        append_bounded_line(&mut output, &line);
        push_typed(
            participant.stable_id().to_owned(),
            participant.identity.platform_kind,
            view.source_event_id.clone(),
            "被@到的参与者（提及不等于指派）",
        );
    }
    for (label, list) in [
        ("已确认要求者", &view.requesters),
        ("已确认负责人", &view.assignees),
        ("已确认承诺人", &view.promisors),
        ("已确认受益方", &view.beneficiaries),
    ] {
        for participant in list {
            let line = format!(
                "\n{label}: {}",
                participant
                    .identity
                    .display_name
                    .clone()
                    .unwrap_or_else(|| participant.stable_id().chars().take(20).collect())
                    .chars()
                    .take(40)
                    .collect::<String>()
            );
            append_bounded_line(&mut output, &line);
            push_typed(
                participant.stable_id().to_owned(),
                participant.identity.platform_kind,
                view.source_event_id.clone(),
                label,
            );
        }
    }
    if view.ambiguous {
        append_bounded_line(&mut output, "\n语义存在歧义，需要 Owner 澄清");
    }
    append_bounded_line(
        &mut output,
        &format!("\n来源事件 {} 条", view.source_refs.len()),
    );

    let event_ids = view.source_refs.clone();
    (output, event_ids, typed_events)
}

/// 格式化参与者上下文（ID-004/ID-005/MEM-002）。summary 为安全中文；
/// 属性来源事件在 typed_events 中投影，LLM 只看到临时引用。
fn format_participant_context(
    view: &crate::ParticipantContextView,
) -> (String, Vec<SourceEventId>, Vec<QueryEffectTypedEvent>) {
    let mut output = format!(
        "参与者: {}（{}）| 群角色: {}",
        view.display_name
            .clone()
            .unwrap_or_else(|| view.participant.stable_id().chars().take(20).collect())
            .chars()
            .take(40)
            .collect::<String>(),
        view.participant.identity.platform_kind.as_str(),
        view.group_role.as_str()
    );
    if let Some(ref card) = view.group_card {
        let line = format!("\n群名片: {}", card.chars().take(40).collect::<String>());
        append_bounded_line(&mut output, &line);
    }
    if !view.aliases.is_empty() {
        let line = format!(
            "\n历史别名: {}",
            view.aliases
                .iter()
                .take(3)
                .map(|alias| alias.chars().take(30).collect::<String>())
                .collect::<Vec<_>>()
                .join("、")
        );
        append_bounded_line(&mut output, &line);
    }
    for attribute in view.attributes.iter().take(5) {
        let line = format!(
            "\n{}: {}",
            attribute.kind.as_str(),
            attribute.value.chars().take(60).collect::<String>()
        );
        append_bounded_line(&mut output, &line);
    }
    if view.expired_or_invalidated {
        append_bounded_line(&mut output, "\n该参与者的档案已失效或来源已被召回");
    }

    // typed_events：属性携带来源时投影；无来源的属性只出现在摘要（不对 LLM 投影）。
    let mut event_ids: Vec<SourceEventId> = view.related_event_ids.clone();
    let mut typed_events: Vec<QueryEffectTypedEvent> = Vec::new();
    for attribute in view.attributes.iter() {
        let Some(source) = attribute.source_event_ids.first() else {
            continue;
        };
        if !event_ids.iter().any(|existing| existing == source) {
            event_ids.push(source.clone());
        }
        event_ids.truncate(10);
        if typed_events.len() < 10 {
            typed_events.push(QueryEffectTypedEvent {
                source_event_id: source.clone(),
                actor_id: view.participant.stable_id().to_owned(),
                actor_kind: view.participant.identity.platform_kind,
                occurred_at_unix_secs: 0,
                excerpt: format!(
                    "{}: {}",
                    attribute.kind.as_str(),
                    attribute.value.chars().take(80).collect::<String>()
                ),
            });
        }
    }
    (output, event_ids, typed_events)
}

/// 格式化多候选解析结果（有界安全中文；不投影稳定 ID，由 Owner 澄清）。
fn format_participant_candidates(candidates: &[crate::AccountScopedParticipantRef]) -> String {
    let mut output = format!(
        "找到 {} 个候选参与者（显示名可能重复），请提供群名片等更多信息以澄清：",
        candidates.len()
    );
    for (index, candidate) in candidates.iter().take(5).enumerate() {
        let display = candidate
            .identity
            .display_name
            .clone()
            .unwrap_or_else(|| candidate.stable_id().chars().take(20).collect())
            .chars()
            .take(40)
            .collect::<String>();
        let line = format!("\n{}. {display}", index + 1);
        append_bounded_line(&mut output, &line);
    }
    output
}

fn append_bounded_line(output: &mut String, line: &str) {
    if output.chars().count() + line.chars().count() <= 900 {
        output.push_str(line);
    }
}

/// 将查询工具结果序列化为结构化 JSON（`QueryEffectResultV1`）。
/// 供 ReplanDecisionNode 解析为 PlannerToolObservation。
/// `typed_events` 用于 LLM 投影，不包含稳定 ID 之外的正文文本。
fn query_effect_json(
    tool_kind: SecretaryToolKind,
    summary: &str,
    source_event_ids: &[SourceEventId],
    typed_events: Vec<QueryEffectTypedEvent>,
) -> Result<String, EffectError> {
    build_query_effect_json(tool_kind, summary, source_event_ids, typed_events, false)
}

/// 带歧义标记的查询回执（如 ResolveReference 多候选）。歧义是确定性业务结果，
/// 供 ReplanDecision 节点登记未解决指代。
fn query_effect_json_ambiguous(
    tool_kind: SecretaryToolKind,
    summary: &str,
    source_event_ids: &[SourceEventId],
    typed_events: Vec<QueryEffectTypedEvent>,
    ambiguous: bool,
) -> Result<String, EffectError> {
    build_query_effect_json(
        tool_kind,
        summary,
        source_event_ids,
        typed_events,
        ambiguous,
    )
}

fn build_query_effect_json(
    tool_kind: SecretaryToolKind,
    summary: &str,
    source_event_ids: &[SourceEventId],
    typed_events: Vec<QueryEffectTypedEvent>,
    ambiguous: bool,
) -> Result<String, EffectError> {
    let bounded_summary: String = summary.chars().take(2_000).collect();
    let result = QueryEffectResultV1 {
        version: 1,
        tool_kind,
        summary: bounded_summary,
        source_event_ids: source_event_ids.to_vec(),
        event_count: source_event_ids.len(),
        typed_events,
        ambiguous,
    };
    serde_json::to_string(&result)
        .map_err(|e| EffectError::new(EffectErrorKind::Permanent, e.to_string()))
}
