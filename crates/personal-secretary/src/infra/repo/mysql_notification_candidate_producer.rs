use sea_orm::{ConnectionTrait, DatabaseBackend, FromQueryResult, Statement};

use crate::{
    ConversationRef, EventKind, InboundEventStoreError, MatchField, MessageSource,
    NotificationCategory, NotificationMatchKeyV1, SourceAccountRef, StructuredImportance,
};

use super::mysql_inbound::store_error;

/// 已锁定且已验证的新鲜通知来源。
///
/// 调用方必须在同一事务中锁定并验证来源后才可构造该值，避免协调流程从遗留
/// Outbox payload 反推来源，或在 producer 内开启嵌套事务造成 TOCTOU。
#[derive(Debug)]
pub(crate) enum LockedNotificationSource {
    Agenda {
        account_id: u64,
        item_id: String,
        version: u64,
        source_channel: String,
        platform_account_id: String,
    },
    FollowUp {
        account_id: u64,
        follow_up_id: String,
        source_version: u64,
        source_channel: String,
        platform_account_id: String,
    },
    ResponseExpectation {
        account_id: u64,
        expectation_id: String,
        source_version: u64,
        source_channel: String,
        platform_account_id: String,
        conversation: ConversationRef,
        actor_id: String,
    },
}

pub(crate) struct NotificationCandidateProduction {
    pub(crate) candidate_created: bool,
    pub(crate) request_created: bool,
}

struct LockedNotificationSourceParts<'a> {
    account_id: u64,
    source_kind: &'static str,
    source_id: &'a str,
    source_version: u64,
    source_channel: &'a str,
    platform_account_id: &'a str,
    category: NotificationCategory,
    event_kind: EventKind,
    conversation: MatchField<ConversationRef>,
    actor_id: MatchField<String>,
}

impl LockedNotificationSource {
    fn parts(&self) -> LockedNotificationSourceParts<'_> {
        match self {
            Self::Agenda {
                account_id,
                item_id,
                version,
                source_channel,
                platform_account_id,
            } => LockedNotificationSourceParts {
                account_id: *account_id,
                source_kind: "agenda",
                source_id: item_id,
                source_version: *version,
                source_channel,
                platform_account_id,
                category: NotificationCategory::Agenda,
                event_kind: EventKind::AgendaDue,
                conversation: MatchField::Absent,
                actor_id: MatchField::Absent,
            },
            Self::FollowUp {
                account_id,
                follow_up_id,
                source_version,
                source_channel,
                platform_account_id,
            } => LockedNotificationSourceParts {
                account_id: *account_id,
                source_kind: "follow_up",
                source_id: follow_up_id,
                source_version: *source_version,
                source_channel,
                platform_account_id,
                category: NotificationCategory::FollowUp,
                event_kind: EventKind::FollowUpDue,
                conversation: MatchField::Absent,
                actor_id: MatchField::Absent,
            },
            Self::ResponseExpectation {
                account_id,
                expectation_id,
                source_version,
                source_channel,
                platform_account_id,
                conversation,
                actor_id,
            } => LockedNotificationSourceParts {
                account_id: *account_id,
                source_kind: "response_expectation",
                source_id: expectation_id,
                source_version: *source_version,
                source_channel,
                platform_account_id,
                category: NotificationCategory::FollowUp,
                event_kind: EventKind::ResponseOverdue,
                conversation: MatchField::Known(conversation.clone()),
                actor_id: MatchField::Known(actor_id.clone()),
            },
        }
    }
}

/// 从已锁定的新鲜来源产生 Candidate 和 generation-1 Request。
///
/// `INSERT IGNORE` 仅在精确的业务唯一键回查成功时视为幂等；该函数不打开事务。
pub(crate) async fn produce_from_locked_source<C: ConnectionTrait>(
    db: &C,
    source: &LockedNotificationSource,
) -> Result<NotificationCandidateProduction, InboundEventStoreError> {
    let LockedNotificationSourceParts {
        account_id,
        source_kind,
        source_id,
        source_version,
        source_channel,
        platform_account_id,
        category,
        event_kind,
        conversation,
        actor_id,
    } = source.parts();
    let account = SourceAccountRef::new(
        parse_source(source_channel)?,
        platform_account_id.to_owned(),
    )
    .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
    let match_key = NotificationMatchKeyV1::new(
        account,
        conversation,
        actor_id,
        MatchField::Known(category),
        MatchField::Known(false),
        MatchField::Known(StructuredImportance::Normal),
        MatchField::Known(event_kind),
    )
    .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
    let candidate_id = uuid::Uuid::new_v4().to_string();
    let inserted = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT IGNORE INTO secretary_notification_candidates \
             (notification_candidate_id, account_id, source_kind, source_id, source_version, match_key_json) \
             VALUES (?, ?, ?, ?, ?, CAST(? AS JSON))",
            [
                candidate_id.clone().into(),
                account_id.into(),
                source_kind.into(),
                source_id.to_owned().into(),
                source_version.into(),
                serde_json::to_string(&match_key)
                    .map_err(|_| {
                        InboundEventStoreError::InvalidData(
                            "notification match key serialization failed".into(),
                        )
                    })?
                    .into(),
            ],
        ))
        .await
        .map_err(store_error)?;
    let (candidate_id, candidate_created) = match inserted.rows_affected() {
        1 => (candidate_id, true),
        0 => (
            load_candidate_id(db, account_id, source_kind, source_id, source_version).await?,
            false,
        ),
        _ => {
            return Err(InboundEventStoreError::InvalidData(
                "notification candidate insert affected multiple rows".into(),
            ));
        }
    };
    let request = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT IGNORE INTO secretary_notification_evaluation_requests \
             (evaluation_request_id, notification_candidate_id, evaluation_generation, trigger_kind) \
             VALUES (?, ?, 1, 'source_due')",
            [
                uuid::Uuid::new_v4().to_string().into(),
                candidate_id.clone().into(),
            ],
        ))
        .await
        .map_err(store_error)?;
    let request_created = match request.rows_affected() {
        1 => true,
        0 => {
            ensure_generation_one_request(db, &candidate_id).await?;
            false
        }
        _ => {
            return Err(InboundEventStoreError::InvalidData(
                "notification evaluation request insert affected multiple rows".into(),
            ));
        }
    };
    Ok(NotificationCandidateProduction {
        candidate_created,
        request_created,
    })
}

async fn load_candidate_id<C: ConnectionTrait>(
    db: &C,
    account_id: u64,
    source_kind: &str,
    source_id: &str,
    source_version: u64,
) -> Result<String, InboundEventStoreError> {
    CandidateIdRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT notification_candidate_id FROM secretary_notification_candidates \
         WHERE account_id = ? AND source_kind = ? AND source_id = ? AND source_version = ? FOR UPDATE",
        [
            account_id.into(),
            source_kind.into(),
            source_id.into(),
            source_version.into(),
        ],
    ))
    .one(db)
    .await
    .map_err(store_error)?
    .map(|row| row.notification_candidate_id)
    .ok_or_else(|| {
        InboundEventStoreError::InvalidData(
            "notification candidate uniqueness check found no row".into(),
        )
    })
}

async fn ensure_generation_one_request<C: ConnectionTrait>(
    db: &C,
    candidate_id: &str,
) -> Result<(), InboundEventStoreError> {
    let existing = RequestIdRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT evaluation_request_id FROM secretary_notification_evaluation_requests \
         WHERE notification_candidate_id = ? AND evaluation_generation = 1 FOR UPDATE",
        [candidate_id.into()],
    ))
    .one(db)
    .await
    .map_err(store_error)?
    .ok_or_else(|| {
        InboundEventStoreError::InvalidData(
            "notification evaluation request uniqueness check found no row".into(),
        )
    })?;
    if existing.evaluation_request_id.is_empty() {
        return Err(InboundEventStoreError::InvalidData(
            "notification evaluation request identity is empty".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, FromQueryResult)]
struct CandidateIdRow {
    notification_candidate_id: String,
}

#[derive(Debug, FromQueryResult)]
struct RequestIdRow {
    evaluation_request_id: String,
}

fn parse_source(value: &str) -> Result<MessageSource, InboundEventStoreError> {
    match value {
        "napcat" => Ok(MessageSource::NapCat),
        "qq_open_platform" => Ok(MessageSource::QqOpenPlatform),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown message source: {value}"
        ))),
    }
}
