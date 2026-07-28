//! EffectExecutor：根据 Action 类型执行真实查询，再持久化幂等 Receipt。
//!
//! P0-3 修复：显式持有 run_id，避免误用 proposal_id。
//! P0-4 修复：根据 Action 类型调用 Retriever 生成真实查询结果，
//! 再调 `ActionStoreT::apply_effect` 持久化幂等 Receipt。

use std::sync::Arc;

use agent_core::graph::{EffectEnvelope, EffectError, EffectErrorKind, EffectExecutor, RunContext};
use async_trait::async_trait;

use crate::{
    AgendaApplyRequest, AgendaItemId, AgendaMutation, AgendaUseCase, EventQuery, ReferenceContext,
    RetrieverUseCase, SecretaryAction, SecretaryActionEffect, SecretaryActionReceipt,
    SourceAccountRef, SourceEventId,
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
    agenda: Option<Arc<AgendaUseCase>>,
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
            agenda: None,
            command_source_event_id: None,
            account,
            now_unix_secs,
        }
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
        }))
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
        if let Some(receipt) = self
            .store
            .load_effect_receipt(&self.run_id, &envelope.id.to_string())
            .await
            .map_err(ActionStoreError::to_effect_error)?
        {
            return Ok(receipt);
        }
        if let Some(receipt) = self
            .execute_agenda(&envelope.effect.proposal, &envelope.id.to_string())
            .await?
        {
            return Ok(receipt);
        }
        // 未命中既有 Receipt 才执行真实只读 Action；Store 提交时再次处理并发竞争。
        let result_ref = self
            .execute_action(&envelope.effect.proposal.action)
            .await?;
        self.store
            .apply_effect(
                &self.run_id,
                &envelope.effect,
                &envelope.id.to_string(),
                &result_ref,
                &self.lease_token,
            )
            .await
            .map_err(|e| e.to_effect_error())
    }
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
