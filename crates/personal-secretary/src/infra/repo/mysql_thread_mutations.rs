use std::collections::HashSet;

use agent_core::graph::{AgentCheckpoint, CheckpointError, CheckpointId, CheckpointStore};
use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};
use tracing::{debug, info};
use uuid::Uuid;

use crate::{
    InboundEventStoreError, ThreadMutationAgentState, ThreadMutationDecision, ThreadMutationEffect,
    ThreadMutationEffectReceipt, ThreadMutationImpact, ThreadMutationKind,
    ThreadMutationProposalStatus, ThreadMutationResumeInput, ThreadMutationRevertInput,
    ThreadMutationRevertReceipt, ThreadMutationStoreT, validate_thread_mutation_impact,
    validate_thread_mutation_revert,
};

use super::mysql_inbound::store_error;

pub(crate) struct MySqlThreadMutationStore {
    db: DatabaseConnection,
}

impl MySqlThreadMutationStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ThreadMutationStoreT for MySqlThreadMutationStore {
    async fn persist_proposal(
        &self,
        impact: &ThreadMutationImpact,
    ) -> Result<(), InboundEventStoreError> {
        validate_thread_mutation_impact(impact)
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
        let transaction = self.db.begin().await.map_err(store_error)?;
        let account = AccountRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT id FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ? FOR UPDATE",
            [impact.account.channel.as_str().into(), impact.account.account_id.clone().into()],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?
        .ok_or_else(|| InboundEventStoreError::InvalidData("thread mutation account was not found".into()))?;

        let thread_ids = impact
            .thread_ids
            .iter()
            .map(|thread_id| thread_id.as_str().to_owned())
            .collect::<HashSet<_>>();
        for thread_id in &impact.thread_ids {
            let row = ThreadAccountRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "SELECT account_id FROM secretary_event_threads WHERE thread_id = ? FOR UPDATE",
                [thread_id.as_str().into()],
            ))
            .one(&transaction)
            .await
            .map_err(store_error)?
            .ok_or_else(|| {
                InboundEventStoreError::InvalidData(format!(
                    "thread {} was not found",
                    thread_id.as_str()
                ))
            })?;
            if row.account_id != account.id {
                return Err(InboundEventStoreError::InvalidData(
                    "thread mutation cannot cross managed accounts".into(),
                ));
            }
        }

        let affected_ids = impact
            .affected_source_event_ids
            .iter()
            .map(|event_id| event_id.as_str().to_owned())
            .collect::<HashSet<_>>();
        let mut conversations = HashSet::new();
        for event_id in &impact.affected_source_event_ids {
            let row = EventMembershipRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"SELECT e.account_id, e.conversation_id, te.thread_id, e.occurred_at_unix_secs
                   FROM secretary_source_events e
                   JOIN secretary_thread_events te ON te.source_event_id = e.source_event_id
                   WHERE e.source_event_id = ? FOR UPDATE"#,
                [event_id.as_str().into()],
            ))
            .one(&transaction)
            .await
            .map_err(store_error)?
            .ok_or_else(|| {
                InboundEventStoreError::InvalidData(format!(
                    "affected event {} was not found in a thread",
                    event_id.as_str()
                ))
            })?;
            if row.account_id != account.id || !thread_ids.contains(&row.thread_id) {
                return Err(InboundEventStoreError::InvalidData(
                    "affected event is outside the proposal account or threads".into(),
                ));
            }
            conversations.insert(row.conversation_id);
        }
        if conversations.len() != impact.affected_conversation_count as usize {
            return Err(InboundEventStoreError::InvalidData(
                "affected_conversation_count does not match stored events".into(),
            ));
        }

        if impact.kind == ThreadMutationKind::Merge {
            let mut stored_ids = HashSet::new();
            for thread_id in &impact.thread_ids {
                let rows = EventIdRow::find_by_statement(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "SELECT source_event_id FROM secretary_thread_events WHERE thread_id = ? FOR UPDATE",
                    [thread_id.as_str().into()],
                ))
                .all(&transaction)
                .await
                .map_err(store_error)?;
                stored_ids.extend(rows.into_iter().map(|row| row.source_event_id));
            }
            if stored_ids != affected_ids {
                return Err(InboundEventStoreError::InvalidData(
                    "merge proposal must enumerate every event in every affected thread".into(),
                ));
            }
        } else {
            let source_thread = impact
                .thread_ids
                .first()
                .expect("validated split has one source thread");
            let total = CountRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "SELECT COUNT(*) AS value FROM secretary_thread_events WHERE thread_id = ? FOR UPDATE",
                [source_thread.as_str().into()],
            ))
            .one(&transaction)
            .await
            .map_err(store_error)?
            .map(|row| row.value)
            .unwrap_or_default();
            if total <= impact.affected_source_event_ids.len() as i64 {
                return Err(InboundEventStoreError::InvalidData(
                    "split proposal must leave at least one event in the source thread".into(),
                ));
            }
        }

        let impact_json = serde_json::to_string(impact).map_err(|error| {
            InboundEventStoreError::InvalidData(format!(
                "cannot serialize thread mutation impact: {error}"
            ))
        })?;
        let now = Utc::now().naive_utc();
        let result = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT IGNORE INTO secretary_thread_mutation_proposals
                   (proposal_id, account_id, mutation_kind, proposal_status, impact_json, created_at, updated_at)
                   VALUES (?, ?, ?, 'awaiting_approval', ?, ?, ?)"#,
                [
                    impact.proposal_id.as_str().into(),
                    account.id.into(),
                    impact.kind.as_str().into(),
                    impact_json.clone().into(),
                    now.into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if result.rows_affected() == 0 {
            let existing = ProposalIdentityRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "SELECT account_id, mutation_kind, CAST(impact_json AS CHAR) AS impact_json FROM secretary_thread_mutation_proposals WHERE proposal_id = ? FOR UPDATE",
                [impact.proposal_id.as_str().into()],
            ))
            .one(&transaction)
            .await
            .map_err(store_error)?
            .ok_or_else(|| InboundEventStoreError::InvalidData("proposal insert lost without an existing row".into()))?;
            let existing_impact: ThreadMutationImpact = serde_json::from_str(&existing.impact_json)
                .map_err(|error| {
                    InboundEventStoreError::InvalidData(format!(
                        "stored proposal impact is invalid: {error}"
                    ))
                })?;
            if existing.account_id != account.id
                || existing.mutation_kind != impact.kind.as_str()
                || existing_impact != *impact
            {
                return Err(InboundEventStoreError::InvalidData(
                    "proposal_id already exists with different immutable impact".into(),
                ));
            }
        }
        transaction.commit().await.map_err(store_error)?;
        debug!(
            proposal_id = impact.proposal_id.as_str(),
            kind = impact.kind.as_str(),
            "thread mutation proposal persisted"
        );
        Ok(())
    }

    async fn authorize_resume(
        &self,
        input: &ThreadMutationResumeInput,
    ) -> Result<ThreadMutationProposalStatus, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let row = ResumeAuthorizationRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT proposal.proposal_status, proposal.decision,
                      proposal.command_source_event_id
               FROM secretary_thread_mutation_proposals proposal
               JOIN secretary_source_events command ON command.source_event_id = ?
               JOIN secretary_accounts command_account ON command_account.id = command.account_id
               JOIN secretary_owner_bindings binding
                 ON binding.managed_account_id = proposal.account_id
                AND binding.command_account_id = command.account_id
                AND binding.owner_actor_id = command.actor_platform_id
                AND binding.status = 'active'
               WHERE proposal.proposal_id = ?
                 AND command.message_role = 'owner_command'
                 AND command_account.source_channel = 'qq_open_platform'
               FOR UPDATE"#,
            [
                input.command_source_event_id.as_str().into(),
                input.proposal_id.as_str().into(),
            ],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?
        .ok_or_else(|| {
            InboundEventStoreError::InvalidData(
                "proposal or authorized OwnerCommand was not found".into(),
            )
        })?;

        let desired = match input.decision {
            ThreadMutationDecision::Approve => ThreadMutationProposalStatus::Approved,
            ThreadMutationDecision::Reject => ThreadMutationProposalStatus::Rejected,
        };
        let existing = parse_status(&row.proposal_status)?;
        if existing == desired
            && row.decision.as_deref() == Some(input.decision.as_str())
            && row.command_source_event_id.as_deref()
                == Some(input.command_source_event_id.as_str())
        {
            transaction.commit().await.map_err(store_error)?;
            return Ok(existing);
        }
        if existing != ThreadMutationProposalStatus::AwaitingApproval {
            return Err(InboundEventStoreError::InvalidData(
                "thread mutation proposal has already consumed a different decision".into(),
            ));
        }

        let now = Utc::now().naive_utc();
        let completed_at = (desired == ThreadMutationProposalStatus::Rejected).then_some(now);
        let result = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_thread_mutation_proposals
                   SET proposal_status = ?, decision = ?, command_source_event_id = ?,
                       updated_at = ?, completed_at = ?
                   WHERE proposal_id = ? AND proposal_status = 'awaiting_approval'"#,
                [
                    desired.as_str().into(),
                    input.decision.as_str().into(),
                    input.command_source_event_id.as_str().into(),
                    now.into(),
                    completed_at.into(),
                    input.proposal_id.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if result.rows_affected() != 1 {
            return Err(InboundEventStoreError::LeaseLost);
        }
        transaction.commit().await.map_err(store_error)?;
        info!(
            proposal_id = input.proposal_id.as_str(),
            decision = input.decision.as_str(),
            "thread mutation owner decision authorized"
        );
        Ok(desired)
    }

    async fn apply_effect(
        &self,
        effect: &ThreadMutationEffect,
        effect_id: &str,
    ) -> Result<ThreadMutationEffectReceipt, InboundEventStoreError> {
        if effect_id.trim().is_empty() || effect_id.len() > 255 {
            return Err(InboundEventStoreError::InvalidData(
                "invalid thread mutation effect id".into(),
            ));
        }
        let transaction = self.db.begin().await.map_err(store_error)?;
        let row = ProposalEffectRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT proposal_status, mutation_kind, CAST(impact_json AS CHAR) AS impact_json, effect_id FROM secretary_thread_mutation_proposals WHERE proposal_id = ? FOR UPDATE",
            [effect.proposal_id.as_str().into()],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?
        .ok_or_else(|| InboundEventStoreError::InvalidData("thread mutation proposal was not found".into()))?;
        let status = parse_status(&row.proposal_status)?;
        if status == ThreadMutationProposalStatus::Applied
            && row.effect_id.as_deref() == Some(effect_id)
        {
            transaction.commit().await.map_err(store_error)?;
            return Ok(ThreadMutationEffectReceipt {
                proposal_id: effect.proposal_id.clone(),
                effect_id: effect_id.into(),
                status,
                changed: false,
            });
        }
        if status != ThreadMutationProposalStatus::Approved || row.effect_id.is_some() {
            return Err(InboundEventStoreError::InvalidData(
                "thread mutation effect is not eligible for execution".into(),
            ));
        }
        if row.mutation_kind != effect.kind.as_str() {
            return Err(InboundEventStoreError::InvalidData(
                "effect kind does not match proposal".into(),
            ));
        }
        let impact: ThreadMutationImpact =
            serde_json::from_str(&row.impact_json).map_err(|error| {
                InboundEventStoreError::InvalidData(format!(
                    "stored proposal impact is invalid: {error}"
                ))
            })?;
        validate_thread_mutation_impact(&impact)
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;

        let now = Utc::now().naive_utc();
        let claimed = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_thread_mutation_proposals SET proposal_status = 'applying', effect_id = ?, updated_at = ? WHERE proposal_id = ? AND proposal_status = 'approved' AND effect_id IS NULL",
                [effect_id.into(), now.into(), effect.proposal_id.as_str().into()],
            ))
            .await
            .map_err(store_error)?;
        if claimed.rows_affected() != 1 {
            return Err(InboundEventStoreError::LeaseLost);
        }

        match effect.kind {
            ThreadMutationKind::Merge => apply_merge(&transaction, &impact).await?,
            ThreadMutationKind::Split => apply_split(&transaction, &impact).await?,
        }
        refresh_link_hints_and_candidates(&transaction, &impact).await?;
        record_semantic_invalidations(&transaction, &impact, "mutation_applied").await?;

        let applied = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_thread_mutation_proposals SET proposal_status = 'applied', completed_at = ?, updated_at = ? WHERE proposal_id = ? AND proposal_status = 'applying' AND effect_id = ?",
                [now.into(), now.into(), effect.proposal_id.as_str().into(), effect_id.into()],
            ))
            .await
            .map_err(store_error)?;
        if applied.rows_affected() != 1 {
            return Err(InboundEventStoreError::LeaseLost);
        }
        transaction.commit().await.map_err(store_error)?;
        info!(
            proposal_id = effect.proposal_id.as_str(),
            effect_id,
            kind = effect.kind.as_str(),
            "thread mutation effect applied"
        );
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
        validate_thread_mutation_revert(input)
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
        let transaction = self.db.begin().await.map_err(store_error)?;
        let row = RevertAuthorizationRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT proposal.proposal_status,
                      CAST(proposal.impact_json AS CHAR) AS impact_json,
                      reversion.command_source_event_id AS reversion_command_source_event_id,
                      reversion.reason AS reversion_reason
               FROM secretary_thread_mutation_proposals proposal
               JOIN secretary_source_events command ON command.source_event_id = ?
               JOIN secretary_accounts command_account ON command_account.id = command.account_id
               JOIN secretary_owner_bindings binding
                 ON binding.managed_account_id = proposal.account_id
                AND binding.command_account_id = command.account_id
                AND binding.owner_actor_id = command.actor_platform_id
                AND binding.status = 'active'
               LEFT JOIN secretary_thread_mutation_reversions reversion
                 ON reversion.proposal_id = proposal.proposal_id
               WHERE proposal.proposal_id = ?
                 AND command.message_role = 'owner_command'
                 AND command_account.source_channel = 'qq_open_platform'
               FOR UPDATE"#,
            [
                input.command_source_event_id.as_str().into(),
                input.proposal_id.as_str().into(),
            ],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?
        .ok_or_else(|| {
            InboundEventStoreError::InvalidData(
                "applied proposal or authorized revert OwnerCommand was not found".into(),
            )
        })?;
        if parse_status(&row.proposal_status)? != ThreadMutationProposalStatus::Applied {
            return Err(InboundEventStoreError::InvalidData(
                "only an applied thread mutation can be reverted".into(),
            ));
        }
        if let Some(existing_command) = row.reversion_command_source_event_id {
            if existing_command == input.command_source_event_id.as_str()
                && row.reversion_reason.as_deref() == Some(input.reason.as_str())
            {
                transaction.commit().await.map_err(store_error)?;
                return Ok(ThreadMutationRevertReceipt {
                    proposal_id: input.proposal_id.clone(),
                    changed: false,
                });
            }
            return Err(InboundEventStoreError::InvalidData(
                "thread mutation was already reverted by a different immutable command".into(),
            ));
        }

        let impact: ThreadMutationImpact =
            serde_json::from_str(&row.impact_json).map_err(|error| {
                InboundEventStoreError::InvalidData(format!(
                    "stored proposal impact is invalid: {error}"
                ))
            })?;
        validate_thread_mutation_impact(&impact)
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT INTO secretary_thread_mutation_reversions
                   (reversion_id, proposal_id, command_source_event_id, reason)
                   VALUES (?, ?, ?, ?)"#,
                [
                    Uuid::new_v4().to_string().into(),
                    input.proposal_id.as_str().into(),
                    input.command_source_event_id.as_str().into(),
                    input.reason.clone().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        let deactivated = match impact.kind {
            ThreadMutationKind::Merge => transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "UPDATE secretary_thread_merge_aliases SET active = FALSE WHERE proposal_id = ? AND active = TRUE",
                    [input.proposal_id.as_str().into()],
                ))
                .await
                .map_err(store_error)?
                .rows_affected(),
            ThreadMutationKind::Split => transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "UPDATE secretary_thread_split_overrides SET active = FALSE WHERE proposal_id = ? AND active = TRUE",
                    [input.proposal_id.as_str().into()],
                ))
                .await
                .map_err(store_error)?
                .rows_affected(),
        };
        if deactivated == 0 {
            return Err(InboundEventStoreError::InvalidData(
                "applied mutation has no active logical overlay to revert".into(),
            ));
        }
        refresh_link_hints_and_candidates(&transaction, &impact).await?;
        record_semantic_invalidations(&transaction, &impact, "mutation_reverted").await?;
        transaction.commit().await.map_err(store_error)?;
        info!(
            proposal_id = input.proposal_id.as_str(),
            "thread mutation reverted and downstream projections invalidated"
        );
        Ok(ThreadMutationRevertReceipt {
            proposal_id: input.proposal_id.clone(),
            changed: true,
        })
    }
}

#[async_trait]
impl CheckpointStore<ThreadMutationAgentState> for MySqlThreadMutationStore {
    async fn save(
        &self,
        checkpoint: AgentCheckpoint<ThreadMutationAgentState>,
    ) -> Result<(), CheckpointError> {
        let checkpoint_id = checkpoint.id();
        let proposal_id = checkpoint.suspend().data.proposal_id.as_str().to_owned();
        if checkpoint.state().business().impact().proposal_id.as_str() != proposal_id {
            return Err(CheckpointError::StoreUnavailable);
        }
        let checkpoint_json = serde_json::to_string(&checkpoint).map_err(|error| {
            tracing::error!(%error, %checkpoint_id, "failed to serialize thread mutation checkpoint");
            CheckpointError::StoreUnavailable
        })?;
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT IGNORE INTO secretary_thread_mutation_checkpoints
                   (checkpoint_id, proposal_id, checkpoint_json, checkpoint_status)
                   VALUES (?, ?, ?, 'active')"#,
                [
                    checkpoint_id.to_string().into(),
                    proposal_id.into(),
                    checkpoint_json.into(),
                ],
            ))
            .await
            .map_err(|error| checkpoint_store_error(error, checkpoint_id))?;
        if result.rows_affected() != 1 {
            return Err(CheckpointError::Duplicate { checkpoint_id });
        }
        debug!(%checkpoint_id, "thread mutation checkpoint persisted");
        Ok(())
    }

    async fn load(
        &self,
        checkpoint_id: CheckpointId,
    ) -> Result<AgentCheckpoint<ThreadMutationAgentState>, CheckpointError> {
        let row = CheckpointRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT CAST(checkpoint_json AS CHAR) AS checkpoint_json FROM secretary_thread_mutation_checkpoints WHERE checkpoint_id = ? AND checkpoint_status = 'active'",
            [checkpoint_id.to_string().into()],
        ))
        .one(&self.db)
        .await
        .map_err(|error| checkpoint_store_error(error, checkpoint_id))?
        .ok_or(CheckpointError::NotFound { checkpoint_id })?;
        deserialize_checkpoint(&row.checkpoint_json, checkpoint_id)
    }

    async fn take(
        &self,
        checkpoint_id: CheckpointId,
    ) -> Result<AgentCheckpoint<ThreadMutationAgentState>, CheckpointError> {
        let transaction = self
            .db
            .begin()
            .await
            .map_err(|error| checkpoint_store_error(error, checkpoint_id))?;
        let row = CheckpointRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT CAST(checkpoint_json AS CHAR) AS checkpoint_json FROM secretary_thread_mutation_checkpoints WHERE checkpoint_id = ? AND checkpoint_status = 'active' FOR UPDATE",
            [checkpoint_id.to_string().into()],
        ))
        .one(&transaction)
        .await
        .map_err(|error| checkpoint_store_error(error, checkpoint_id))?
        .ok_or(CheckpointError::NotFound { checkpoint_id })?;
        let checkpoint = deserialize_checkpoint(&row.checkpoint_json, checkpoint_id)?;
        let result = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_thread_mutation_checkpoints SET checkpoint_status = 'consumed', consumed_at = ? WHERE checkpoint_id = ? AND checkpoint_status = 'active'",
                [Utc::now().naive_utc().into(), checkpoint_id.to_string().into()],
            ))
            .await
            .map_err(|error| checkpoint_store_error(error, checkpoint_id))?;
        if result.rows_affected() != 1 {
            return Err(CheckpointError::NotFound { checkpoint_id });
        }
        transaction
            .commit()
            .await
            .map_err(|error| checkpoint_store_error(error, checkpoint_id))?;
        debug!(%checkpoint_id, "thread mutation checkpoint consumed");
        Ok(checkpoint)
    }
}

fn deserialize_checkpoint(
    checkpoint_json: &str,
    checkpoint_id: CheckpointId,
) -> Result<AgentCheckpoint<ThreadMutationAgentState>, CheckpointError> {
    let checkpoint = serde_json::from_str::<AgentCheckpoint<ThreadMutationAgentState>>(
        checkpoint_json,
    )
    .map_err(|error| {
        tracing::error!(%error, %checkpoint_id, "failed to deserialize thread mutation checkpoint");
        CheckpointError::StoreUnavailable
    })?;
    if checkpoint.id() != checkpoint_id {
        tracing::error!(%checkpoint_id, stored_checkpoint_id = %checkpoint.id(), "thread mutation checkpoint identity mismatch");
        return Err(CheckpointError::StoreUnavailable);
    }
    Ok(checkpoint)
}

fn checkpoint_store_error(error: sea_orm::DbErr, checkpoint_id: CheckpointId) -> CheckpointError {
    tracing::error!(%error, %checkpoint_id, "thread mutation checkpoint store operation failed");
    CheckpointError::StoreUnavailable
}

async fn refresh_link_hints_and_candidates<C: ConnectionTrait>(
    connection: &C,
    impact: &ThreadMutationImpact,
) -> Result<(), InboundEventStoreError> {
    for event_id in &impact.affected_source_event_ids {
        connection
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_thread_link_hints hint
                   JOIN secretary_effective_thread_events effective
                     ON effective.source_event_id = hint.source_event_id
                   SET hint.thread_id = effective.thread_id
                   WHERE hint.source_event_id = ?"#,
                [event_id.as_str().into()],
            ))
            .await
            .map_err(store_error)?;
        connection
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_thread_link_candidates candidate
                   JOIN secretary_thread_link_candidate_sources source
                     ON source.candidate_id = candidate.candidate_id
                   SET candidate.status = 'expired'
                   WHERE source.source_event_id = ? AND candidate.status = 'proposed'"#,
                [event_id.as_str().into()],
            ))
            .await
            .map_err(store_error)?;
    }
    Ok(())
}

async fn record_semantic_invalidations<C: ConnectionTrait>(
    connection: &C,
    impact: &ThreadMutationImpact,
    kind: &str,
) -> Result<(), InboundEventStoreError> {
    let mut thread_ids = impact
        .thread_ids
        .iter()
        .map(|thread_id| thread_id.as_str().to_owned())
        .collect::<HashSet<_>>();
    if impact.kind == ThreadMutationKind::Split {
        thread_ids.insert(impact.proposal_id.as_str().to_owned());
    }
    for thread_id in thread_ids {
        connection
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT IGNORE INTO secretary_thread_semantic_invalidations
                   (invalidation_id, proposal_id, thread_id, invalidation_kind)
                   VALUES (?, ?, ?, ?)"#,
                [
                    Uuid::new_v4().to_string().into(),
                    impact.proposal_id.as_str().into(),
                    thread_id.clone().into(),
                    kind.into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        connection
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "DELETE FROM secretary_thread_semantic_state WHERE thread_id = ?",
                [thread_id.into()],
            ))
            .await
            .map_err(store_error)?;
    }
    Ok(())
}

async fn apply_merge<C: ConnectionTrait>(
    connection: &C,
    impact: &ThreadMutationImpact,
) -> Result<(), InboundEventStoreError> {
    let canonical = impact
        .thread_ids
        .first()
        .expect("validated merge has threads");
    for merged in impact.thread_ids.iter().skip(1) {
        let conflict = CountRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT COUNT(*) AS value FROM secretary_thread_merge_aliases
               WHERE active = TRUE AND
                     (merged_thread_id IN (?, ?) OR canonical_thread_id = ?)"#,
            [
                canonical.as_str().into(),
                merged.as_str().into(),
                merged.as_str().into(),
            ],
        ))
        .one(connection)
        .await
        .map_err(store_error)?
        .map(|row| row.value)
        .unwrap_or_default();
        if conflict != 0 {
            return Err(InboundEventStoreError::InvalidData(
                "merge would create an alias chain or overwrite an active alias".into(),
            ));
        }
        connection
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT INTO secretary_thread_merge_aliases
                   (merged_thread_id, canonical_thread_id, proposal_id, active)
                   VALUES (?, ?, ?, TRUE)"#,
                [
                    merged.as_str().into(),
                    canonical.as_str().into(),
                    impact.proposal_id.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
    }
    Ok(())
}

async fn apply_split<C: ConnectionTrait>(
    connection: &C,
    impact: &ThreadMutationImpact,
) -> Result<(), InboundEventStoreError> {
    let source_thread = impact
        .thread_ids
        .first()
        .expect("validated split has a thread");
    let alias_count = CountRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT COUNT(*) AS value FROM secretary_thread_merge_aliases WHERE active = TRUE AND merged_thread_id = ?",
        [source_thread.as_str().into()],
    ))
    .one(connection)
    .await
    .map_err(store_error)?
    .map(|row| row.value)
    .unwrap_or_default();
    if alias_count != 0 {
        return Err(InboundEventStoreError::InvalidData(
            "split source is an active merge alias; resolve the merge first".into(),
        ));
    }

    let mut events = Vec::with_capacity(impact.affected_source_event_ids.len());
    for event_id in &impact.affected_source_event_ids {
        let row = EventMembershipRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT e.account_id, e.conversation_id, te.thread_id, e.occurred_at_unix_secs
               FROM secretary_source_events e JOIN secretary_thread_events te
                 ON te.source_event_id = e.source_event_id
               WHERE e.source_event_id = ? FOR UPDATE"#,
            [event_id.as_str().into()],
        ))
        .one(connection)
        .await
        .map_err(store_error)?
        .ok_or_else(|| InboundEventStoreError::InvalidData("split event was not found".into()))?;
        if row.thread_id != source_thread.as_str() {
            return Err(InboundEventStoreError::InvalidData(
                "split event no longer belongs to the proposal source thread".into(),
            ));
        }
        events.push((event_id.as_str().to_owned(), row));
    }
    events.sort_by_key(|(_, row)| (row.occurred_at_unix_secs, row.conversation_id));
    let (root_event_id, root) = events.first().expect("validated split has events");
    let (latest_event_id, latest) = events.last().expect("validated split has events");
    let effective_thread_id = impact.proposal_id.as_str();
    connection
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"INSERT INTO secretary_event_threads
               (thread_id, account_id, status, root_event_id, latest_event_id,
                opened_at_unix_secs, latest_occurred_at_unix_secs)
               VALUES (?, ?, 'open', ?, ?, ?, ?)"#,
            [
                effective_thread_id.into(),
                root.account_id.into(),
                root_event_id.clone().into(),
                latest_event_id.clone().into(),
                root.occurred_at_unix_secs.into(),
                latest.occurred_at_unix_secs.into(),
            ],
        ))
        .await
        .map_err(store_error)?;
    for (event_id, _) in events {
        connection
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT INTO secretary_thread_split_overrides
                   (source_event_id, original_thread_id, effective_thread_id, proposal_id, active)
                   VALUES (?, ?, ?, ?, TRUE)"#,
                [
                    event_id.into(),
                    source_thread.as_str().into(),
                    effective_thread_id.into(),
                    impact.proposal_id.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
    }
    Ok(())
}

fn parse_status(value: &str) -> Result<ThreadMutationProposalStatus, InboundEventStoreError> {
    match value {
        "awaiting_approval" => Ok(ThreadMutationProposalStatus::AwaitingApproval),
        "approved" => Ok(ThreadMutationProposalStatus::Approved),
        "rejected" => Ok(ThreadMutationProposalStatus::Rejected),
        "applying" => Ok(ThreadMutationProposalStatus::Applying),
        "applied" => Ok(ThreadMutationProposalStatus::Applied),
        "failed" => Ok(ThreadMutationProposalStatus::Failed),
        "unknown_commit" => Ok(ThreadMutationProposalStatus::UnknownCommit),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown thread mutation status {value}"
        ))),
    }
}

#[derive(Debug, FromQueryResult)]
struct AccountRow {
    id: u64,
}

#[derive(Debug, FromQueryResult)]
struct ThreadAccountRow {
    account_id: u64,
}

#[derive(Debug, FromQueryResult)]
struct EventIdRow {
    source_event_id: String,
}

#[derive(Debug, FromQueryResult)]
struct EventMembershipRow {
    account_id: u64,
    conversation_id: u64,
    thread_id: String,
    occurred_at_unix_secs: i64,
}

#[derive(Debug, FromQueryResult)]
struct ProposalIdentityRow {
    account_id: u64,
    mutation_kind: String,
    impact_json: String,
}

#[derive(Debug, FromQueryResult)]
struct ResumeAuthorizationRow {
    proposal_status: String,
    decision: Option<String>,
    command_source_event_id: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct ProposalEffectRow {
    proposal_status: String,
    mutation_kind: String,
    impact_json: String,
    effect_id: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct RevertAuthorizationRow {
    proposal_status: String,
    impact_json: String,
    reversion_command_source_event_id: Option<String>,
    reversion_reason: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    value: i64,
}

#[derive(Debug, FromQueryResult)]
struct CheckpointRow {
    checkpoint_json: String,
}
