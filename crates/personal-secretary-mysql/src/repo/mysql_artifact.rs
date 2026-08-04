//! MySQL Artifact 仓储：实现 [`crate::ArtifactStoreT`]。
//!
//! Artifact 信封持久化到 MySQL。不自动下载；URL 不写日志。
//! 撤回失效传播：`invalidate_for_recall` 标记某 source_event_id 的所有 Artifact 为 recalled。
//! TTL 过期：`expire_due` 标记已过期的 Artifact 为 expired。

use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};
use tracing::debug;

use crate::{
    ArtifactAvailability, ArtifactEnvelope, ArtifactId, ArtifactKind, ArtifactStoreError,
    ArtifactStoreT, ContentSegment, ContentTrustLevel, ConversationKind, ConversationRef,
    MediaKind, MessageSource, RichContentKind, SourceAccountRef, SourceEventId,
};

use super::mysql_retriever::resolve_account_id;

pub(crate) struct MySqlArtifactStore {
    db: DatabaseConnection,
}

impl MySqlArtifactStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[derive(sea_orm::FromQueryResult)]
#[allow(dead_code)]
struct ArtifactRow {
    artifact_id: String,
    account_id: u64,
    source_event_id: String,
    conversation_id: u64,
    artifact_kind: String,
    platform_reference: String,
    display_name: Option<String>,
    mime_type: Option<String>,
    size_bytes: Option<u64>,
    hash_or_source_key: Option<String>,
    description: Option<String>,
    availability: String,
    content_policy: String,
    created_at_unix_secs: i64,
    ttl_expires_at_unix_secs: Option<i64>,
}

fn db_err(e: sea_orm::DbErr) -> ArtifactStoreError {
    ArtifactStoreError::Database(e.to_string())
}

/// 从行重建 `ArtifactEnvelope`。
async fn map_artifact_row(
    db: &impl ConnectionTrait,
    row: ArtifactRow,
) -> Result<ArtifactEnvelope, ArtifactStoreError> {
    let artifact_id = ArtifactId::new(&row.artifact_id)
        .map_err(|e| ArtifactStoreError::InvalidData(e.to_string()))?;
    let artifact_kind = ArtifactKind::parse_from_str(&row.artifact_kind).ok_or_else(|| {
        ArtifactStoreError::InvalidData(format!("unknown artifact_kind: {}", row.artifact_kind))
    })?;
    let availability =
        ArtifactAvailability::parse_from_str(&row.availability).ok_or_else(|| {
            ArtifactStoreError::InvalidData(format!("unknown availability: {}", row.availability))
        })?;
    let content_policy = match row.content_policy.as_str() {
        "normal" => ContentTrustLevel::Normal,
        "local_only" => ContentTrustLevel::LocalOnly,
        "envelope_only" => ContentTrustLevel::EnvelopeOnly,
        "never_long_term" => ContentTrustLevel::NeverLongTerm,
        _ => {
            return Err(ArtifactStoreError::InvalidData(format!(
                "unknown content_policy: {}",
                row.content_policy
            )));
        }
    };

    // 反查 account_ref 和 conversation。
    #[derive(sea_orm::FromQueryResult)]
    struct AccountRow {
        source_channel: String,
        platform_account_id: String,
    }
    let account_row = AccountRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT source_channel, platform_account_id FROM secretary_accounts WHERE id = ?",
        [row.account_id.into()],
    ))
    .one(db)
    .await
    .map_err(db_err)?
    .ok_or_else(|| {
        ArtifactStoreError::InvalidData(format!("account_id {} not found", row.account_id))
    })?;
    let channel = match account_row.source_channel.as_str() {
        "napcat" => MessageSource::NapCat,
        "qq_open_platform" => MessageSource::QqOpenPlatform,
        _ => {
            return Err(ArtifactStoreError::InvalidData(format!(
                "unknown source_channel: {}",
                account_row.source_channel
            )));
        }
    };
    let account = SourceAccountRef::new(channel, account_row.platform_account_id)
        .map_err(|e| ArtifactStoreError::InvalidData(e.to_string()))?;

    // 反查 conversation。
    #[derive(sea_orm::FromQueryResult)]
    struct ConvRow {
        conversation_kind: String,
        platform_conversation_id: String,
    }
    let conv_row = ConvRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT conversation_kind, platform_conversation_id FROM secretary_conversations WHERE id = ?",
        [row.conversation_id.into()],
    ))
    .one(db)
    .await
    .map_err(db_err)?
    .ok_or_else(|| {
        ArtifactStoreError::InvalidData(format!("conversation_id {} not found", row.conversation_id))
    })?;
    let conv_kind = match conv_row.conversation_kind.as_str() {
        "private" => ConversationKind::Private,
        "group" => ConversationKind::Group,
        "owner_control" => ConversationKind::OwnerControl,
        _ => {
            return Err(ArtifactStoreError::InvalidData(format!(
                "unknown conversation_kind: {}",
                conv_row.conversation_kind
            )));
        }
    };
    let conversation = ConversationRef::new(conv_kind, conv_row.platform_conversation_id)
        .map_err(|e| ArtifactStoreError::InvalidData(e.to_string()))?;

    let source_event_id = SourceEventId::new(&row.source_event_id)
        .map_err(|e| ArtifactStoreError::InvalidData(e.to_string()))?;

    Ok(ArtifactEnvelope {
        artifact_id,
        account,
        source_event_id,
        conversation,
        artifact_kind,
        platform_reference: row.platform_reference,
        display_name: row.display_name,
        mime_type: row.mime_type,
        size_bytes: row.size_bytes,
        hash_or_source_key: row.hash_or_source_key,
        description: row.description,
        availability,
        content_policy,
        created_at_unix_secs: row.created_at_unix_secs,
        ttl_expires_at_unix_secs: row.ttl_expires_at_unix_secs,
    })
}

fn parse_message_source(value: &str) -> Result<MessageSource, ArtifactStoreError> {
    match value {
        "napcat" => Ok(MessageSource::NapCat),
        "qq_open_platform" => Ok(MessageSource::QqOpenPlatform),
        _ => Err(ArtifactStoreError::InvalidData(format!(
            "unknown source_channel: {value}"
        ))),
    }
}

fn parse_conversation_kind(value: &str) -> Result<ConversationKind, ArtifactStoreError> {
    match value {
        "private" => Ok(ConversationKind::Private),
        "group" => Ok(ConversationKind::Group),
        "owner_control" => Ok(ConversationKind::OwnerControl),
        _ => Err(ArtifactStoreError::InvalidData(format!(
            "unknown conversation_kind: {value}"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
fn envelope_for_segment(
    account: &SourceAccountRef,
    conversation: &ConversationRef,
    source_event_id: &SourceEventId,
    occurred_at: i64,
    ttl: Option<i64>,
    ordinal: usize,
    segment: &ContentSegment,
) -> Result<Option<ArtifactEnvelope>, ArtifactStoreError> {
    let (kind, reference, display_name, description) = match segment {
        ContentSegment::Media {
            kind,
            source_key,
            display_name,
            ..
        } => (
            match kind {
                MediaKind::Image => ArtifactKind::Image,
                MediaKind::Audio => ArtifactKind::Record,
                MediaKind::Video => ArtifactKind::Video,
                MediaKind::File => ArtifactKind::File,
            },
            source_key,
            display_name.clone(),
            None,
        ),
        ContentSegment::Forward { source_key } => (ArtifactKind::Forward, source_key, None, None),
        ContentSegment::Rich {
            kind,
            source_key,
            summary,
        } => (
            match kind {
                RichContentKind::Json => ArtifactKind::RichJson,
                RichContentKind::Xml => ArtifactKind::RichXml,
                RichContentKind::Card => ArtifactKind::RichCard,
            },
            source_key,
            None,
            summary.clone(),
        ),
        _ => return Ok(None),
    };
    let mut envelope = ArtifactEnvelope::new(
        ArtifactId::for_source_segment(source_event_id, ordinal, kind),
        account.clone(),
        source_event_id.clone(),
        conversation.clone(),
        kind,
        reference.clone(),
        ContentTrustLevel::Normal,
        occurred_at,
        ttl,
    )
    .map_err(|error| ArtifactStoreError::InvalidData(error.to_string()))?;
    if let Some(name) = display_name {
        envelope = envelope.with_display_name(Some(name));
    }
    if let Some(description) = description {
        envelope = envelope.with_description(Some(description));
    }
    Ok(Some(envelope))
}

async fn insert_artifact_in_tx(
    db: &impl ConnectionTrait,
    account_id: u64,
    conversation_id: u64,
    envelope: &ArtifactEnvelope,
) -> Result<(), ArtifactStoreError> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"INSERT IGNORE INTO secretary_artifacts
           (artifact_id, account_id, source_event_id, conversation_id, artifact_kind,
            platform_reference, display_name, mime_type, size_bytes, hash_or_source_key,
            description, availability, content_policy, created_at_unix_secs,
            ttl_expires_at_unix_secs)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        [
            envelope.artifact_id.as_str().into(),
            account_id.into(),
            envelope.source_event_id.as_str().into(),
            conversation_id.into(),
            envelope.artifact_kind.as_str().into(),
            envelope.platform_reference.clone().into(),
            envelope
                .display_name
                .clone()
                .map(sea_orm::Value::from)
                .unwrap_or(sea_orm::Value::Bool(None)),
            envelope
                .mime_type
                .clone()
                .map(sea_orm::Value::from)
                .unwrap_or(sea_orm::Value::Bool(None)),
            envelope
                .size_bytes
                .map(sea_orm::Value::from)
                .unwrap_or(sea_orm::Value::Bool(None)),
            envelope
                .hash_or_source_key
                .clone()
                .map(sea_orm::Value::from)
                .unwrap_or(sea_orm::Value::Bool(None)),
            envelope
                .description
                .clone()
                .map(sea_orm::Value::from)
                .unwrap_or(sea_orm::Value::Bool(None)),
            envelope.availability.as_str().into(),
            envelope.content_policy.as_str().into(),
            envelope.created_at_unix_secs.into(),
            envelope
                .ttl_expires_at_unix_secs
                .map(sea_orm::Value::from)
                .unwrap_or(sea_orm::Value::Bool(None)),
        ],
    ))
    .await
    .map_err(db_err)?;
    Ok(())
}

include!("mysql_artifact_derivation_helpers.inc.rs");

#[async_trait]
impl ArtifactStoreT for MySqlArtifactStore {
    async fn create_artifact(&self, envelope: &ArtifactEnvelope) -> Result<(), ArtifactStoreError> {
        let account_id = resolve_account_id(&self.db, &envelope.account)
            .await
            .map_err(|e| ArtifactStoreError::Database(e.to_string()))?;

        let transaction = self.db.begin().await.map_err(db_err)?;
        #[derive(FromQueryResult)]
        struct SourceLockRow {
            _source_event_id: String,
        }
        let source = SourceLockRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT source_event_id AS _source_event_id FROM secretary_source_events \
             WHERE source_event_id = ? AND account_id = ? FOR UPDATE",
            [envelope.source_event_id.as_str().into(), account_id.into()],
        ))
        .one(&transaction)
        .await
        .map_err(db_err)?;
        if source.is_none() {
            return Err(ArtifactStoreError::InvalidData(
                "artifact source event is missing or belongs to another account".into(),
            ));
        }

        #[derive(FromQueryResult)]
        struct RecallCountRow {
            recalled: i64,
        }
        let recalled = RecallCountRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT COUNT(*) AS recalled FROM secretary_message_tombstones \
             WHERE account_id = ? AND source_event_id = ? AND status = 'applied'",
            [account_id.into(), envelope.source_event_id.as_str().into()],
        ))
        .one(&transaction)
        .await
        .map_err(db_err)?
        .is_some_and(|row| row.recalled > 0);
        let availability = if recalled {
            ArtifactAvailability::Recalled
        } else {
            envelope.availability
        };

        // 查找 conversation_id。
        #[derive(sea_orm::FromQueryResult)]
        struct ConvIdRow {
            id: u64,
        }
        let conv_row = ConvIdRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT id FROM secretary_conversations WHERE account_id = ? AND conversation_kind = ? AND platform_conversation_id = ?",
            [
                account_id.into(),
                envelope.conversation.kind.as_str().into(),
                envelope.conversation.id.clone().into(),
            ],
        ))
        .one(&transaction)
        .await
        .map_err(db_err)?;
        let conversation_id = conv_row
            .ok_or_else(|| {
                ArtifactStoreError::InvalidData(format!(
                    "conversation not found for account {account_id}"
                ))
            })?
            .id;

        // 幂等：INSERT IGNORE。
        let result = ConnectionTrait::execute_raw(
            &transaction,
            Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT IGNORE INTO secretary_artifacts
                   (artifact_id, account_id, source_event_id, conversation_id, artifact_kind,
                    platform_reference, display_name, mime_type, size_bytes, hash_or_source_key,
                    description, availability, content_policy, created_at_unix_secs,
                    ttl_expires_at_unix_secs)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
                [
                    envelope.artifact_id.as_str().into(),
                    account_id.into(),
                    envelope.source_event_id.as_str().into(),
                    conversation_id.into(),
                    envelope.artifact_kind.as_str().into(),
                    envelope.platform_reference.clone().into(),
                    envelope
                        .display_name
                        .clone()
                        .map(sea_orm::Value::from)
                        .unwrap_or(sea_orm::Value::Bool(None)),
                    envelope
                        .mime_type
                        .clone()
                        .map(sea_orm::Value::from)
                        .unwrap_or(sea_orm::Value::Bool(None)),
                    envelope
                        .size_bytes
                        .map(sea_orm::Value::from)
                        .unwrap_or(sea_orm::Value::Bool(None)),
                    envelope
                        .hash_or_source_key
                        .clone()
                        .map(sea_orm::Value::from)
                        .unwrap_or(sea_orm::Value::Bool(None)),
                    envelope
                        .description
                        .clone()
                        .map(sea_orm::Value::from)
                        .unwrap_or(sea_orm::Value::Bool(None)),
                    availability.as_str().into(),
                    envelope.content_policy.as_str().into(),
                    envelope.created_at_unix_secs.into(),
                    envelope
                        .ttl_expires_at_unix_secs
                        .map(sea_orm::Value::from)
                        .unwrap_or(sea_orm::Value::Bool(None)),
                ],
            ),
        )
        .await
        .map_err(db_err)?;
        transaction.commit().await.map_err(db_err)?;

        debug!(
            artifact_id = envelope.artifact_id.as_str(),
            rows_affected = result.rows_affected(),
            "Artifact 信封已持久化（幂等）"
        );
        Ok(())
    }

    async fn load_artifact(
        &self,
        artifact_id: &ArtifactId,
    ) -> Result<Option<ArtifactEnvelope>, ArtifactStoreError> {
        let row = ArtifactRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT artifact_id, account_id, source_event_id, conversation_id, artifact_kind,
                      platform_reference, display_name, mime_type, size_bytes, hash_or_source_key,
                      description, availability, content_policy, created_at_unix_secs,
                      ttl_expires_at_unix_secs
               FROM secretary_artifacts
               WHERE artifact_id = ?"#,
            [artifact_id.as_str().into()],
        ))
        .one(&self.db)
        .await
        .map_err(db_err)?;

        match row {
            Some(row) => Ok(Some(map_artifact_row(&self.db, row).await?)),
            None => Ok(None),
        }
    }

    async fn list_for_event(
        &self,
        account: &SourceAccountRef,
        source_event_id: &SourceEventId,
    ) -> Result<Vec<ArtifactEnvelope>, ArtifactStoreError> {
        let account_id = resolve_account_id(&self.db, account)
            .await
            .map_err(|e| ArtifactStoreError::Database(e.to_string()))?;

        let rows = ArtifactRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT artifact_id, account_id, source_event_id, conversation_id, artifact_kind,
                      platform_reference, display_name, mime_type, size_bytes, hash_or_source_key,
                      description, availability, content_policy, created_at_unix_secs,
                      ttl_expires_at_unix_secs
               FROM secretary_artifacts
               WHERE account_id = ? AND source_event_id = ? AND availability = 'available'
               ORDER BY created_at_unix_secs DESC"#,
            [account_id.into(), source_event_id.as_str().into()],
        ))
        .all(&self.db)
        .await
        .map_err(db_err)?;

        let mut envelopes = Vec::new();
        for row in rows {
            envelopes.push(map_artifact_row(&self.db, row).await?);
        }
        Ok(envelopes)
    }

    async fn invalidate_for_recall(
        &self,
        source_event_id: &SourceEventId,
    ) -> Result<u64, ArtifactStoreError> {
        let result = ConnectionTrait::execute_raw(
            &self.db,
            Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_artifacts
                   SET availability = 'recalled'
                   WHERE source_event_id = ? AND availability = 'available'"#,
                [source_event_id.as_str().into()],
            ),
        )
        .await
        .map_err(db_err)?;

        debug!(
            source_event_id = source_event_id.as_str(),
            rows_affected = result.rows_affected(),
            "Artifact 撤回失效传播完成"
        );
        Ok(result.rows_affected())
    }

    async fn derive_pending(
        &self,
        default_ttl_secs: u64,
        batch_size: u64,
    ) -> Result<u64, ArtifactStoreError> {
        let mut completed = 0_u64;
        for _ in 0..batch_size.clamp(1, 100) {
            let Some((row, lease)) = claim_artifact_derivation(&self.db).await? else {
                break;
            };
            let outcome = derive_claimed_artifacts(&self.db, &row, default_ttl_secs).await;
            match outcome {
                Ok(()) => {
                    complete_artifact_derivation(&self.db, &row.source_event_id, &lease).await?;
                    completed += 1;
                }
                Err(DerivationFailure::Permanent(code)) => {
                    fail_artifact_derivation(&self.db, &row.source_event_id, &lease, code).await?;
                }
                Err(DerivationFailure::Retryable(error)) => {
                    retry_artifact_derivation(&self.db, &row.source_event_id, &lease, &error)
                        .await?;
                    break;
                }
            }
        }
        Ok(completed)
    }

    async fn expire_due(&self, now_unix_secs: i64) -> Result<u64, ArtifactStoreError> {
        let result = ConnectionTrait::execute_raw(
            &self.db,
            Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_artifacts
                   SET availability = 'expired'
                   WHERE ttl_expires_at_unix_secs IS NOT NULL
                     AND ttl_expires_at_unix_secs <= ?
                     AND availability = 'available'"#,
                [now_unix_secs.into()],
            ),
        )
        .await
        .map_err(db_err)?;

        debug!(
            now_unix_secs,
            rows_affected = result.rows_affected(),
            "Artifact TTL 过期清理完成"
        );
        Ok(result.rows_affected())
    }
}
