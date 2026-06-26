use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, QueryFilter, QueryOrder, Set, Statement, TransactionTrait,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::domain::memory::ALLOWED_MEMORY_TYPES;
use crate::domain::user::user_context_control::{
    ForgetResult, PersonaRebuildResult, PersonaResetResult, PersonaSnapshotSummary, PersonaView,
    TranscriptClearResult, UserContextControlRepoT,
};
use crate::shared::error::AppError;

use super::super::entities::{
    conversation_messages, conversation_summaries, conversations, post_conversation_risk_audits,
    user_memories, user_persona_snapshots, user_profiles,
};

pub struct UserContextControlRepo {
    db: DatabaseConnection,
    memory_collection: String,
    summary_collection: String,
}

impl UserContextControlRepo {
    pub fn new(
        db: DatabaseConnection,
        memory_collection: String,
        summary_collection: String,
    ) -> Self {
        Self {
            db,
            memory_collection,
            summary_collection,
        }
    }
}

fn map_err(context: &str, error: sea_orm::DbErr) -> AppError {
    AppError::internal(format!("{context}: {error}"))
}

async fn bump_context_version<C>(db: &C, user_id: u64) -> Result<(), AppError>
where
    C: ConnectionTrait,
{
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO user_context_versions (user_id, version, updated_at) \
         VALUES (?, 2, UTC_TIMESTAMP(6)) \
         ON DUPLICATE KEY UPDATE version = version + 1, updated_at = UTC_TIMESTAMP(6)",
        [user_id.into()],
    ))
    .await
    .map_err(|error| map_err("bump context version", error))?;
    Ok(())
}

async fn enqueue_delete_job<C>(
    db: &C,
    object_type: &str,
    object_id: u64,
    collection_name: &str,
    vector_id: String,
) -> Result<(), AppError>
where
    C: ConnectionTrait,
{
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO vector_index_jobs \
         (action, object_type, object_id, collection_name, vector_id, priority, status, \
          attempts, max_attempts, next_run_at, created_at, updated_at) \
         VALUES ('delete', ?, ?, ?, ?, 200, 'pending', 0, 5, \
                 UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
        [
            object_type.into(),
            object_id.into(),
            collection_name.into(),
            vector_id.into(),
        ],
    ))
    .await
    .map_err(|error| map_err("enqueue vector delete job", error))?;
    Ok(())
}

async fn set_personalization<C>(
    db: &C,
    user_id: u64,
    enabled: bool,
    reset_now: bool,
) -> Result<(), AppError>
where
    C: ConnectionTrait,
{
    let reset_insert = if reset_now {
        "UTC_TIMESTAMP(6)"
    } else {
        "NULL"
    };
    let reset_update = if reset_now {
        ", personalization_reset_at = UTC_TIMESTAMP(6)"
    } else {
        ""
    };
    let sql = format!(
        "INSERT INTO user_profiles \
         (user_id, personalization_enabled, personalization_reset_at, created_at, updated_at) \
         VALUES (?, ?, {reset_insert}, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)) \
         ON DUPLICATE KEY UPDATE personalization_enabled = VALUES(personalization_enabled), \
         updated_at = UTC_TIMESTAMP(6){reset_update}"
    );
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        sql,
        [user_id.into(), i8::from(enabled).into()],
    ))
    .await
    .map_err(|error| map_err("update personalization state", error))?;
    Ok(())
}

async fn expire_active_persona<C>(db: &C, user_id: u64) -> Result<u64, AppError>
where
    C: ConnectionTrait,
{
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE user_persona_snapshots \
             SET status = 'expired', expires_at = UTC_TIMESTAMP(6) \
             WHERE user_id = ? AND status = 'active'",
            [user_id.into()],
        ))
        .await
        .map_err(|error| map_err("expire persona snapshots", error))?;
    Ok(result.rows_affected())
}

struct ClearData {
    messages_deleted: u64,
    summaries_deleted: u64,
    audits_deleted: u64,
    summary_ids: Vec<u64>,
}

async fn clear_transcript_in_transaction<C>(
    db: &C,
    user_id: u64,
    summary_collection: &str,
) -> Result<ClearData, AppError>
where
    C: ConnectionTrait,
{
    let Some(conversation) = conversations::Entity::find()
        .filter(conversations::Column::UserId.eq(user_id))
        .one(db)
        .await
        .map_err(|error| map_err("find user conversation", error))?
    else {
        return Ok(ClearData {
            messages_deleted: 0,
            summaries_deleted: 0,
            audits_deleted: 0,
            summary_ids: Vec::new(),
        });
    };

    let summaries = conversation_summaries::Entity::find()
        .filter(conversation_summaries::Column::ConversationId.eq(conversation.id))
        .all(db)
        .await
        .map_err(|error| map_err("load summaries for clear", error))?;
    let summary_ids = summaries
        .iter()
        .map(|summary| summary.summary_id)
        .collect::<Vec<_>>();
    for summary_id in &summary_ids {
        enqueue_delete_job(
            db,
            "summary",
            *summary_id,
            summary_collection,
            format!("conversation_summary:{summary_id}"),
        )
        .await?;
    }

    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE user_memory_evidence evidence \
         JOIN conversation_messages message ON evidence.message_id = message.id \
         SET evidence.message_id = NULL, evidence.source_deleted = 1 \
         WHERE message.conversation_id = ?",
        [conversation.id.into()],
    ))
    .await
    .map_err(|error| map_err("detach message evidence", error))?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE user_memory_evidence evidence \
         JOIN conversation_summaries summary ON evidence.summary_id = summary.summary_id \
         SET evidence.summary_id = NULL, evidence.source_deleted = 1 \
         WHERE summary.conversation_id = ?",
        [conversation.id.into()],
    ))
    .await
    .map_err(|error| map_err("detach summary evidence", error))?;

    let audits_deleted = post_conversation_risk_audits::Entity::delete_many()
        .filter(post_conversation_risk_audits::Column::ConversationId.eq(conversation.id))
        .exec(db)
        .await
        .map_err(|error| map_err("delete transcript risk audits", error))?
        .rows_affected;
    let summaries_deleted = conversation_summaries::Entity::delete_many()
        .filter(conversation_summaries::Column::ConversationId.eq(conversation.id))
        .exec(db)
        .await
        .map_err(|error| map_err("delete conversation summaries", error))?
        .rows_affected;
    let messages_deleted = conversation_messages::Entity::delete_many()
        .filter(conversation_messages::Column::ConversationId.eq(conversation.id))
        .exec(db)
        .await
        .map_err(|error| map_err("delete conversation messages", error))?
        .rows_affected;

    let mut active: conversations::ActiveModel = conversation.into();
    active.message_count = Set(0);
    active.title = Set(None);
    active.last_message_at = Set(None);
    active.updated_at = Set(Utc::now().naive_utc());
    active
        .update(db)
        .await
        .map_err(|error| map_err("reset conversation metadata", error))?;

    Ok(ClearData {
        messages_deleted,
        summaries_deleted,
        audits_deleted,
        summary_ids,
    })
}

fn array_len(snapshot: &Value, key: &str) -> usize {
    snapshot
        .get(key)
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

fn snapshot_summary(snapshot: &Value) -> PersonaSnapshotSummary {
    PersonaSnapshotSummary {
        communication_preferences_count: array_len(snapshot, "communication_preferences"),
        stable_facts_count: array_len(snapshot, "stable_facts"),
        recurring_topics_count: array_len(snapshot, "recurring_topics"),
        goals_count: array_len(snapshot, "goals"),
        sensitive_context_count: array_len(snapshot, "sensitive_context"),
    }
}

#[async_trait]
impl UserContextControlRepoT for UserContextControlRepo {
    async fn persona_view(&self, user_id: u64) -> Result<PersonaView, AppError> {
        let profile = user_profiles::Entity::find()
            .filter(user_profiles::Column::UserId.eq(user_id))
            .one(&self.db)
            .await
            .map_err(|error| map_err("load personalization profile", error))?;
        let snapshot = user_persona_snapshots::Entity::find()
            .filter(user_persona_snapshots::Column::UserId.eq(user_id))
            .filter(user_persona_snapshots::Column::Status.eq("active"))
            .order_by_desc(user_persona_snapshots::Column::CreatedAt)
            .one(&self.db)
            .await
            .map_err(|error| map_err("load active persona", error))?;

        Ok(PersonaView {
            has_active_persona: snapshot.is_some(),
            generated_at: snapshot.as_ref().map(|value| value.created_at.and_utc()),
            snapshot_summary: snapshot
                .as_ref()
                .map(|value| snapshot_summary(&value.snapshot_data))
                .unwrap_or_default(),
            personalization_enabled: profile
                .map(|value| value.personalization_enabled != 0)
                .unwrap_or(true),
        })
    }

    async fn reset_persona(&self, user_id: u64) -> Result<PersonaResetResult, AppError> {
        let txn = self
            .db
            .begin()
            .await
            .map_err(|error| map_err("begin persona reset", error))?;
        expire_active_persona(&txn, user_id).await?;
        set_personalization(&txn, user_id, false, true).await?;
        bump_context_version(&txn, user_id).await?;
        txn.commit()
            .await
            .map_err(|error| map_err("commit persona reset", error))?;
        Ok(PersonaResetResult { reset: true })
    }

    async fn rebuild_persona(&self, user_id: u64) -> Result<PersonaRebuildResult, AppError> {
        let txn = self
            .db
            .begin()
            .await
            .map_err(|error| map_err("begin persona rebuild", error))?;
        let profile = user_profiles::Entity::find()
            .filter(user_profiles::Column::UserId.eq(user_id))
            .one(&txn)
            .await
            .map_err(|error| map_err("load profile for persona rebuild", error))?;
        let mut query = user_memories::Entity::find()
            .filter(user_memories::Column::UserId.eq(user_id))
            .filter(user_memories::Column::Status.eq(1))
            .filter(user_memories::Column::MemoryType.is_in(ALLOWED_MEMORY_TYPES));
        if let Some(reset_at) = profile
            .as_ref()
            .and_then(|value| value.personalization_reset_at)
        {
            query = query.filter(user_memories::Column::CreatedAt.gt(reset_at));
        }
        let memories = query
            .order_by_asc(user_memories::Column::MemoryId)
            .all(&txn)
            .await
            .map_err(|error| map_err("load memories for persona rebuild", error))?;

        let mut communication_preferences = Vec::new();
        let mut stable_facts = Vec::new();
        let mut recurring_topics = Vec::new();
        let mut goals = Vec::new();
        for memory in &memories {
            match memory.memory_type.as_str() {
                "preference" => communication_preferences.push(memory.content.clone()),
                "fact" => stable_facts.push(memory.content.clone()),
                "emotional_pattern" => recurring_topics.push(memory.content.clone()),
                "goal" => goals.push(memory.content.clone()),
                _ => {}
            }
        }
        let snapshot_data = json!({
            "communication_preferences": communication_preferences,
            "support_preferences": [],
            "style_observations": {
                "tone": "neutral",
                "directness": "medium",
                "structure": "step_by_step",
                "question_frequency": "low"
            },
            "stable_facts": stable_facts,
            "recurring_topics": recurring_topics,
            "goals": goals,
            "sensitive_context": []
        });
        let input_hash = Sha256::digest(snapshot_data.to_string().as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();

        let supersedes_id = user_persona_snapshots::Entity::find()
            .filter(user_persona_snapshots::Column::UserId.eq(user_id))
            .filter(user_persona_snapshots::Column::Status.eq("active"))
            .one(&txn)
            .await
            .map_err(|error| map_err("load previous persona", error))?
            .map(|value| value.snapshot_id);
        expire_active_persona(&txn, user_id).await?;
        let active = user_persona_snapshots::ActiveModel {
            user_id: Set(user_id),
            status: Set("active".into()),
            snapshot_data: Set(snapshot_data),
            source_memory_ids: Set(json!(
                memories
                    .iter()
                    .map(|memory| memory.memory_id)
                    .collect::<Vec<_>>()
            )),
            source_summary_ids: Set(Some(json!([]))),
            source_recent_message_ids: Set(Some(json!([]))),
            input_hash: Set(input_hash),
            model_name: Set("deterministic-memory-aggregate".into()),
            prompt_version: Set("persona-rebuild-v1".into()),
            schema_version: Set("1.0".into()),
            generation_ms: Set(0),
            supersedes_id: Set(supersedes_id),
            error_message: Set(None),
            created_at: Set(Utc::now().naive_utc()),
            expires_at: Set(None),
            ..Default::default()
        };
        let saved = active
            .insert(&txn)
            .await
            .map_err(|error| map_err("save rebuilt persona", error))?;
        set_personalization(&txn, user_id, true, false).await?;
        bump_context_version(&txn, user_id).await?;
        txn.commit()
            .await
            .map_err(|error| map_err("commit persona rebuild", error))?;

        Ok(PersonaRebuildResult {
            snapshot_id: saved.snapshot_id,
        })
    }

    async fn clear_transcript(&self, user_id: u64) -> Result<TranscriptClearResult, AppError> {
        let txn = self
            .db
            .begin()
            .await
            .map_err(|error| map_err("begin transcript clear", error))?;
        let cleared =
            clear_transcript_in_transaction(&txn, user_id, &self.summary_collection).await?;
        bump_context_version(&txn, user_id).await?;
        txn.commit()
            .await
            .map_err(|error| map_err("commit transcript clear", error))?;

        Ok(TranscriptClearResult {
            cleared_messages: cleared.messages_deleted > 0,
            cleared_summaries: cleared.summaries_deleted > 0,
            memories_preserved: true,
            persona_preserved: true,
            post_risk_audits_cleared: cleared.audits_deleted > 0,
            summary_ids: cleared.summary_ids,
        })
    }

    async fn forget(&self, user_id: u64) -> Result<ForgetResult, AppError> {
        let txn = self
            .db
            .begin()
            .await
            .map_err(|error| map_err("begin forget", error))?;
        let cleared =
            clear_transcript_in_transaction(&txn, user_id, &self.summary_collection).await?;
        let memories = user_memories::Entity::find()
            .filter(user_memories::Column::UserId.eq(user_id))
            .all(&txn)
            .await
            .map_err(|error| map_err("load memories for forget", error))?;
        let memory_ids = memories
            .iter()
            .map(|memory| memory.memory_id)
            .collect::<Vec<_>>();
        for memory_id in &memory_ids {
            enqueue_delete_job(
                &txn,
                "memory",
                *memory_id,
                &self.memory_collection,
                format!("user_memory:{memory_id}"),
            )
            .await?;
        }
        let memories_disabled = txn
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE user_memories SET status = 0, updated_at = UTC_TIMESTAMP(6) \
                 WHERE user_id = ? AND status <> 0",
                [user_id.into()],
            ))
            .await
            .map_err(|error| map_err("disable memories for forget", error))?
            .rows_affected();
        let persona_expired = expire_active_persona(&txn, user_id).await? > 0;
        let audits_deleted = post_conversation_risk_audits::Entity::delete_many()
            .filter(post_conversation_risk_audits::Column::UserId.eq(user_id))
            .exec(&txn)
            .await
            .map_err(|error| map_err("delete user risk audits", error))?
            .rows_affected;
        set_personalization(&txn, user_id, false, true).await?;
        bump_context_version(&txn, user_id).await?;
        txn.commit()
            .await
            .map_err(|error| map_err("commit forget", error))?;

        Ok(ForgetResult {
            messages_cleared: cleared.messages_deleted > 0,
            summaries_cleared: cleared.summaries_deleted > 0,
            memories_disabled,
            persona_expired,
            post_risk_audits_deleted: cleared.audits_deleted + audits_deleted > 0,
            personalization_disabled: true,
            summary_ids: cleared.summary_ids,
            memory_ids,
        })
    }
}
