//! EffectExecutor：根据 Action 类型执行真实查询，再持久化幂等 Receipt。
//!
//! P0-3 修复：显式持有 run_id，避免误用 proposal_id。
//! P0-4 修复：根据 Action 类型调用 Retriever 生成真实查询结果，
//! 再调 `ActionStoreT::apply_effect` 持久化幂等 Receipt。

use std::sync::Arc;

use agent_core::graph::{EffectEnvelope, EffectError, EffectErrorKind, EffectExecutor, RunContext};
use async_trait::async_trait;

use crate::{
    AgendaApplyRequest, AgendaItemId, AgendaMutation, AgendaUseCase, ConversationMemoryModeInput,
    EventQuery, FollowUpControlEffectRequest, FollowUpControlStoreError, FollowUpControlUseCase,
    MemoryDeleteInput, MemoryFact, MemoryFactId, MemoryFactStatus, MemoryUseCase,
    NotificationPolicyEffectRequest, NotificationPolicyUseCase, ReferenceContext, RetrieverUseCase,
    SecretaryAction, SecretaryActionEffect, SecretaryActionReceipt, SourceAccountRef,
    SourceEventId, ThreadControlEffectRequest, ThreadControlStoreError, ThreadControlUseCase,
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
    command_source_event_id: Option<SourceEventId>,
    account: SourceAccountRef,
    now_unix_secs: i64,
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
            command_source_event_id: None,
            account,
            now_unix_secs,
        }
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
            .map_err(|error| EffectError::new(EffectErrorKind::Permanent, error.to_string()))?;
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
    /// Effect 不再只写 executed:{effect_id}，而是调用 Retriever 生成真实结果。
    async fn execute_action(&self, action: &SecretaryAction) -> Result<String, EffectError> {
        if let SecretaryAction::ListUpcomingItems { horizon_secs } = action {
            let agenda = self.agenda.as_ref().ok_or_else(|| {
                EffectError::new(EffectErrorKind::Permanent, "AgendaUseCase 未注入")
            })?;
            let items = agenda
                .list_upcoming(&self.account, *horizon_secs)
                .await
                .map_err(|error| EffectError::new(EffectErrorKind::Transient, error.to_string()))?;
            return Ok(format!("查到 {} 个即将到期事项", items.len()));
        }
        let retriever = self.retriever.as_ref().ok_or_else(|| {
            EffectError::new(
                EffectErrorKind::Permanent,
                "Retriever 未注入，无法执行查询型 Action",
            )
        })?;
        match action {
            SecretaryAction::SearchRecentEvents { query, limit } => {
                let event_query = EventQuery {
                    account: self.account.clone(),
                    conversation: None,
                    actor_id: None,
                    thread_id: None,
                    since_unix_secs: Some(self.now_unix_secs - 86_400),
                    until_unix_secs: Some(self.now_unix_secs),
                    query_text: Some(query.clone()),
                    limit: *limit,
                };
                let results = retriever
                    .search_events(&event_query, false)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                Ok(format_event_results(&results))
            }
            SecretaryAction::ReadSourceEvent { source_event_id } => {
                let detail = retriever
                    .read_source_event(source_event_id, &self.account)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                match detail {
                    Some(d) => Ok(format!(
                        "事件 {} | {} | {} | 摘录: {}",
                        d.source_event_id.as_str(),
                        d.actor.id,
                        d.occurred_at_unix_secs,
                        d.normalized_text.chars().take(120).collect::<String>(),
                    )),
                    None => Ok(format!("未找到事件 {}", source_event_id.as_str())),
                }
            }
            SecretaryAction::SearchEventThreads { query, limit } => {
                let results = retriever
                    .search_threads(&self.account, query, *limit)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                Ok(format!("搜索到 {} 个线程", results.len()))
            }
            SecretaryAction::ResolveReference { expression } => {
                let context = ReferenceContext {
                    account: self.account.clone(),
                    current_conversation: None,
                    current_thread_id: None,
                    recent_events: Vec::new(),
                    now_unix_secs: self.now_unix_secs,
                    timezone: "UTC".into(),
                };
                let resolution = retriever
                    .resolve_reference(&self.account, expression, &context)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                Ok(if resolution.ambiguous {
                    format!("指代歧义：{}", resolution.evidence)
                } else {
                    format!("指代已解析：{}", resolution.evidence)
                })
            }
            SecretaryAction::GetSecretaryStatus => {
                let status = retriever
                    .secretary_status(&self.account)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                Ok(format_secretary_status(&status))
            }
            SecretaryAction::ListPendingOwnerWork { limit } => {
                let items = retriever
                    .list_pending_owner_work(&self.account, *limit)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                Ok(format_pending_owner_work(&items))
            }
            SecretaryAction::GetThreadContext { thread_id } => {
                let context = retriever
                    .thread_context(&self.account, thread_id)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                Ok(match context {
                    Some(context) => format_thread_context(&context),
                    None => format!("未找到当前账号下的线程 {}", thread_id.as_str()),
                })
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
        let is_follow_up_control = is_follow_up_control_action(&envelope.effect.proposal.action);
        if !is_mutable_policy
            && !is_thread_control
            && !is_follow_up_control
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
    )
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

fn append_bounded_line(output: &mut String, line: &str) {
    if output.chars().count() + line.chars().count() <= 900 {
        output.push_str(line);
    }
}
