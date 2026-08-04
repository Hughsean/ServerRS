
#[derive(FromQueryResult)]
struct DerivationRow {
    source_event_id: String,
    account_id: u64,
    conversation_id: u64,
    source_channel: String,
    platform_account_id: String,
    conversation_kind: String,
    platform_conversation_id: String,
    occurred_at_unix_secs: i64,
    segments_json: String,
    recalled: i64,
}

enum DerivationFailure {
    Permanent(&'static str),
    Retryable(String),
}

async fn claim_artifact_derivation(
    db: &DatabaseConnection,
) -> Result<Option<(DerivationRow, String)>, ArtifactStoreError> {
    let txn = db.begin().await.map_err(db_err)?;
    txn.execute_raw(Statement::from_string(
        DatabaseBackend::MySql,
        "UPDATE secretary_artifact_derivations SET status = 'pending', lease_token = NULL, \
         lease_expires_at = NULL, last_error_code = 'lease_expired' \
         WHERE status = 'claimed' AND lease_expires_at < UTC_TIMESTAMP(6)",
    ))
    .await
    .map_err(db_err)?;
    let row = DerivationRow::find_by_statement(Statement::from_string(
        DatabaseBackend::MySql,
        r#"SELECT job.source_event_id, event.account_id, event.conversation_id,
                  account.source_channel, account.platform_account_id,
                  conversation.conversation_kind, conversation.platform_conversation_id,
                  event.occurred_at_unix_secs, CAST(content.segments AS CHAR) AS segments_json,
                  EXISTS(SELECT 1 FROM secretary_message_tombstones tombstone
                         WHERE tombstone.account_id = event.account_id
                           AND tombstone.source_event_id = event.source_event_id
                           AND tombstone.status = 'applied') AS recalled
           FROM secretary_artifact_derivations job
           JOIN secretary_source_events event ON event.source_event_id = job.source_event_id
           JOIN secretary_accounts account ON account.id = event.account_id
           JOIN secretary_conversations conversation ON conversation.id = event.conversation_id
           JOIN secretary_message_contents content ON content.source_event_id = event.source_event_id
           WHERE job.status = 'pending'
           ORDER BY job.created_at, job.source_event_id
           LIMIT 1 FOR UPDATE SKIP LOCKED"#,
    ))
    .one(&txn)
    .await
    .map_err(db_err)?;
    let Some(row) = row else {
        txn.commit().await.map_err(db_err)?;
        return Ok(None);
    };
    let lease = uuid::Uuid::new_v4().to_string();
    let result = txn
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_artifact_derivations SET status = 'claimed', attempts = attempts + 1, \
             lease_token = ?, lease_expires_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 60 SECOND), \
             last_error_code = NULL \
             WHERE source_event_id = ? AND status = 'pending'",
            [lease.clone().into(), row.source_event_id.clone().into()],
        ))
        .await
        .map_err(db_err)?;
    if result.rows_affected() != 1 {
        return Err(ArtifactStoreError::Database("artifact derivation lease lost".into()));
    }
    txn.commit().await.map_err(db_err)?;
    Ok(Some((row, lease)))
}

async fn derive_claimed_artifacts(
    db: &DatabaseConnection,
    row: &DerivationRow,
    default_ttl_secs: u64,
) -> Result<(), DerivationFailure> {
    let source_event_id = SourceEventId::new(&row.source_event_id)
        .map_err(|_| DerivationFailure::Permanent("invalid_source_event"))?;
    let account = SourceAccountRef::new(
        parse_message_source(&row.source_channel).map_err(|_| DerivationFailure::Permanent("invalid_source"))?,
        row.platform_account_id.clone(),
    )
    .map_err(|_| DerivationFailure::Permanent("invalid_account"))?;
    let conversation = ConversationRef::new(
        parse_conversation_kind(&row.conversation_kind).map_err(|_| DerivationFailure::Permanent("invalid_conversation"))?,
        row.platform_conversation_id.clone(),
    )
    .map_err(|_| DerivationFailure::Permanent("invalid_conversation"))?;
    let segments: Vec<ContentSegment> = serde_json::from_str(&row.segments_json)
        .map_err(|_| DerivationFailure::Permanent("invalid_segments_json"))?;
    let ttl = (default_ttl_secs > 0)
        .then(|| row.occurred_at_unix_secs.saturating_add(default_ttl_secs as i64));
    let txn = db.begin().await.map_err(|error| DerivationFailure::Retryable(error.to_string()))?;
    for (ordinal, segment) in segments.iter().enumerate() {
        let Some(mut envelope) = envelope_for_segment(
            &account,
            &conversation,
            &source_event_id,
            row.occurred_at_unix_secs,
            ttl,
            ordinal,
            segment,
        )
        .map_err(|_| DerivationFailure::Permanent("invalid_artifact_envelope"))?
        else {
            continue;
        };
        if row.recalled > 0 {
            envelope.mark_recalled();
        }
        insert_artifact_in_tx(&txn, row.account_id, row.conversation_id, &envelope)
            .await
            .map_err(|error| DerivationFailure::Retryable(error.to_string()))?;
    }
    txn.commit()
        .await
        .map_err(|error| DerivationFailure::Retryable(error.to_string()))
}

async fn update_derivation_status(
    db: &DatabaseConnection,
    source_event_id: &str,
    lease: &str,
    status: &str,
    error_code: Option<&str>,
    retry: bool,
) -> Result<(), ArtifactStoreError> {
    let sql = if retry {
        "UPDATE secretary_artifact_derivations SET status = 'pending', lease_token = NULL, \
         lease_expires_at = NULL, last_error_code = ? WHERE source_event_id = ? \
         AND status = 'claimed' AND lease_token = ?"
    } else {
        "UPDATE secretary_artifact_derivations SET status = ?, lease_token = NULL, \
         lease_expires_at = NULL, last_error_code = ? WHERE source_event_id = ? \
         AND status = 'claimed' AND lease_token = ?"
    };
    let values = if retry {
        vec![
            error_code.map(sea_orm::Value::from).unwrap_or(sea_orm::Value::Bool(None)),
            source_event_id.into(),
            lease.into(),
        ]
    } else {
        vec![
            status.into(),
            error_code.map(sea_orm::Value::from).unwrap_or(sea_orm::Value::Bool(None)),
            source_event_id.into(),
            lease.into(),
        ]
    };
    let result = db
        .execute_raw(Statement::from_sql_and_values(DatabaseBackend::MySql, sql, values))
        .await
        .map_err(db_err)?;
    if result.rows_affected() != 1 {
        return Err(ArtifactStoreError::Database("artifact derivation lease lost".into()));
    }
    Ok(())
}

async fn complete_artifact_derivation(
    db: &DatabaseConnection,
    source_event_id: &str,
    lease: &str,
) -> Result<(), ArtifactStoreError> {
    update_derivation_status(db, source_event_id, lease, "completed", None, false).await
}

async fn fail_artifact_derivation(
    db: &DatabaseConnection,
    source_event_id: &str,
    lease: &str,
    code: &str,
) -> Result<(), ArtifactStoreError> {
    update_derivation_status(db, source_event_id, lease, "failed", Some(code), false).await
}

async fn retry_artifact_derivation(
    db: &DatabaseConnection,
    source_event_id: &str,
    lease: &str,
    _error: &str,
) -> Result<(), ArtifactStoreError> {
    update_derivation_status(db, source_event_id, lease, "pending", Some("store_unavailable"), true).await
}
