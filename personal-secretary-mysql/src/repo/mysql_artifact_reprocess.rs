use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};

use crate::{
    ArtifactReprocessEffectRequest, ArtifactReprocessStoreError, ArtifactReprocessStoreT,
    SecretaryAction, SecretaryActionProposal, SecretaryActionReceipt,
};

pub(crate) struct MySqlArtifactReprocessStore {
    db: DatabaseConnection,
}

impl MySqlArtifactReprocessStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ArtifactReprocessStoreT for MySqlArtifactReprocessStore {
    async fn apply_effect(
        &self,
        request: &ArtifactReprocessEffectRequest,
    ) -> Result<SecretaryActionReceipt, ArtifactReprocessStoreError> {
        let (limit, reason) = match &request.action {
            SecretaryAction::RetryFailedArtifactDerivations { limit, reason } => {
                (*limit, reason.as_str())
            }
            _ => {
                return Err(ArtifactReprocessStoreError::InvalidData(
                    "action is not an artifact reprocess control".into(),
                ));
            }
        };
        let transaction = self.db.begin().await.map_err(database_error)?;
        if let Some(receipt) = load_receipt(&transaction, request).await? {
            transaction.commit().await.map_err(database_error)?;
            return Ok(receipt);
        }
        let account_id = lock_account(&transaction, request).await?;
        verify_action_lease(&transaction, request, account_id).await?;
        verify_owner_command(&transaction, request, account_id).await?;
        if let Some(receipt) = load_receipt(&transaction, request).await? {
            transaction.commit().await.map_err(database_error)?;
            return Ok(receipt);
        }

        let rows = SourceEventRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT job.source_event_id FROM secretary_artifact_derivations job \
             INNER JOIN secretary_source_events event \
               ON event.source_event_id = job.source_event_id \
             WHERE event.account_id = ? AND job.status = 'failed' \
               AND job.lease_token IS NULL AND job.lease_expires_at IS NULL \
             ORDER BY job.updated_at, job.source_event_id LIMIT ? FOR UPDATE SKIP LOCKED",
            [account_id.into(), u64::from(limit).into()],
        ))
        .all(&transaction)
        .await
        .map_err(database_error)?;

        let mut requeued = 0_u64;
        for row in &rows {
            let result = transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "UPDATE secretary_artifact_derivations \
                     SET status = 'pending', last_error_code = NULL, updated_at = UTC_TIMESTAMP(6) \
                     WHERE source_event_id = ? AND status = 'failed' \
                       AND lease_token IS NULL AND lease_expires_at IS NULL",
                    [row.source_event_id.clone().into()],
                ))
                .await
                .map_err(database_error)?;
            if result.rows_affected() != 1 {
                return Err(ArtifactReprocessStoreError::LeaseLost);
            }
            requeued += 1;
        }

        let result_ref = serde_json::json!({
            "scope": "artifact_derivation_reprocess",
            "requested": limit,
            "requeued": requeued,
        })
        .to_string();
        let requeued_source_event_ids = serde_json::to_string(
            &rows
                .iter()
                .map(|row| row.source_event_id.as_str())
                .collect::<Vec<_>>(),
        )
        .map_err(|_| {
            ArtifactReprocessStoreError::InvalidData(
                "artifact reprocess audit target serialization failed".into(),
            )
        })?;
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT INTO secretary_artifact_reprocess_audit \
                 (audit_id, effect_id, run_id, proposal_id, account_id, \
                  command_source_event_id, requested_limit, requeued_count, \
                  requeued_source_event_ids, reason) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                [
                    uuid::Uuid::new_v4().to_string().into(),
                    request.effect_id.clone().into(),
                    request.run_id.as_str().into(),
                    request.proposal_id.clone().into(),
                    account_id.into(),
                    request.command_source_event_id.as_str().into(),
                    u64::from(limit).into(),
                    requeued.into(),
                    requeued_source_event_ids.into(),
                    reason.into(),
                ],
            ))
            .await
            .map_err(database_error)?;
        let inserted = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT IGNORE INTO secretary_action_effect_receipts \
                 (effect_id, run_id, proposal_json, result_ref) VALUES (?, ?, ?, ?)",
                [
                    request.effect_id.clone().into(),
                    request.run_id.as_str().into(),
                    request.proposal_json.clone().into(),
                    result_ref.clone().into(),
                ],
            ))
            .await
            .map_err(database_error)?;
        if inserted.rows_affected() != 1 {
            return Err(ArtifactReprocessStoreError::Database);
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(SecretaryActionReceipt {
            proposal_id: request.proposal_id.clone(),
            result_ref,
            tool_kind: Some(request.action.kind()),
        })
    }
}

async fn lock_account<C: ConnectionTrait>(
    db: &C,
    request: &ArtifactReprocessEffectRequest,
) -> Result<u64, ArtifactReprocessStoreError> {
    IdRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT id FROM secretary_accounts WHERE source_channel = ? \
         AND platform_account_id = ? AND status = 'active' FOR UPDATE",
        [
            request.account.channel.as_str().into(),
            request.account.account_id.clone().into(),
        ],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .map(|row| row.id)
    .ok_or(ArtifactReprocessStoreError::Unauthorized)
}

async fn verify_action_lease<C: ConnectionTrait>(
    db: &C,
    request: &ArtifactReprocessEffectRequest,
    account_id: u64,
) -> Result<(), ArtifactReprocessStoreError> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_action_runs SET updated_at = UTC_TIMESTAMP(6) \
             WHERE run_id = ? AND lease_token = ? AND status = 'running' AND account_id = ? \
               AND command_source_event_id = ? AND lease_expires_at >= UTC_TIMESTAMP(6)",
            [
                request.run_id.as_str().into(),
                request.lease_token.as_str().into(),
                account_id.into(),
                request.command_source_event_id.as_str().into(),
            ],
        ))
        .await
        .map_err(database_error)?;
    if result.rows_affected() != 1 {
        return Err(ArtifactReprocessStoreError::LeaseLost);
    }
    Ok(())
}

async fn verify_owner_command<C: ConnectionTrait>(
    db: &C,
    request: &ArtifactReprocessEffectRequest,
    account_id: u64,
) -> Result<(), ArtifactReprocessStoreError> {
    super::owner_authorization::verify_owner_command(
        db,
        &request.command_source_event_id,
        account_id,
    )
    .await
    .map_err(|error| match error {
        super::owner_authorization::OwnerAuthError::Unauthorized => {
            ArtifactReprocessStoreError::Unauthorized
        }
        super::owner_authorization::OwnerAuthError::Database => {
            ArtifactReprocessStoreError::Database
        }
    })
}

async fn load_receipt<C: ConnectionTrait>(
    db: &C,
    request: &ArtifactReprocessEffectRequest,
) -> Result<Option<SecretaryActionReceipt>, ArtifactReprocessStoreError> {
    let row = ReceiptRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT run_id, CAST(proposal_json AS CHAR) AS proposal_json, result_ref \
         FROM secretary_action_effect_receipts WHERE effect_id = ?",
        [request.effect_id.clone().into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?;
    row.map(|row| {
        let proposal: SecretaryActionProposal = serde_json::from_str(&row.proposal_json)
            .map_err(|_| ArtifactReprocessStoreError::Database)?;
        if row.run_id != request.run_id.as_str()
            || proposal.proposal_id != request.proposal_id
            || proposal.action != request.action
        {
            return Err(ArtifactReprocessStoreError::InvalidData(
                "effect receipt belongs to a different action".into(),
            ));
        }
        Ok(SecretaryActionReceipt {
            proposal_id: proposal.proposal_id,
            result_ref: row.result_ref,
            tool_kind: Some(request.action.kind()),
        })
    })
    .transpose()
}

fn database_error(_: sea_orm::DbErr) -> ArtifactReprocessStoreError {
    ArtifactReprocessStoreError::Database
}

#[derive(FromQueryResult)]
struct IdRow {
    id: u64,
}

#[derive(FromQueryResult)]
struct SourceEventRow {
    source_event_id: String,
}

#[derive(FromQueryResult)]
struct ReceiptRow {
    run_id: String,
    proposal_json: String,
    result_ref: String,
}
