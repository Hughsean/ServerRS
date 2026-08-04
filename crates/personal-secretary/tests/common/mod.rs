//! 隔离 MySQL 集成测试共享夹具（schema 派生、真实入站路径、授权链 fixture）。
//!
//! 需要 QQBOT_TEST_DATABASE_URL 指向隔离的 MySQL schema（`qqbot_accept_` 前缀）；
//! 每个测试用基础 schema + 后缀 + 随机段派生独立 schema，测试结束时删除。
#![allow(dead_code)]

use personal_secretary::{
    ActionLeaseToken, ActionRunId, Clock, ConversationKind, ConversationRef,
    InboundMessageEnvelope, MessageSource, SourceAccountRef, SourceEventId, SourceMessageRef,
    SystemClock, VerifiedActor, VerifiedActorKind,
};
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use std::sync::Arc;
use uuid::Uuid;

#[path = "../../../../apps/qqbot-server/database/test_support/qqbot_migrations.rs"]
mod qqbot_migrations;

pub async fn isolated_db(suffix: &str) -> (DatabaseConnection, String) {
    let base_url = std::env::var("QQBOT_TEST_DATABASE_URL").expect("QQBOT_TEST_DATABASE_URL");
    let base_schema = base_url
        .split('?')
        .next()
        .and_then(|value| value.rsplit('/').next())
        .unwrap_or_default()
        .to_owned();
    assert!(
        base_schema.starts_with("qqbot_accept_")
            && base_schema
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    );
    assert!(
        suffix
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    );
    let random = Uuid::new_v4().simple().to_string();
    let tail = format!("{suffix}-{}", &random[..12]);
    let max_base_len = 64usize
        .checked_sub(tail.len())
        .expect("test schema suffix must fit MySQL identifier limit");
    assert!(max_base_len >= "qqbot_accept_".len());
    let schema = format!(
        "{}{tail}",
        &base_schema[..base_schema.len().min(max_base_len)]
    );
    let base = Database::connect(&base_url).await.expect("connect base");
    base.execute_unprepared(&format!("CREATE DATABASE IF NOT EXISTS `{schema}`"))
        .await
        .expect("create schema");
    drop(base);
    let (prefix, _) = base_url.rsplit_once('/').expect("url parse");
    let db = Database::connect(format!("{prefix}/{schema}"))
        .await
        .expect("connect derived");
    qqbot_migrations::apply_qqbot_migrations(
        &db,
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/qqbot-server/database/migrations"),
    )
    .await;
    (db, schema)
}

#[allow(dead_code)] // 仅被部分 MySQL 测试 target 引用（cmd009/project_commitment）
pub async fn drop_schema(db: &DatabaseConnection, schema: &str) {
    db.execute_unprepared(&format!("DROP DATABASE IF EXISTS `{schema}`"))
        .await
        .expect("drop isolated test schema");
}

pub async fn scalar_u64(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> u64 {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            values,
        ))
        .await
        .expect("query")
        .expect("row");
    match row.try_get::<u64>("", "value") {
        Ok(value) => value,
        Err(unsigned_error) => {
            let signed = row.try_get::<i64>("", "value").unwrap_or_else(|signed_error| {
                panic!(
                    "value must decode as a MySQL integer: unsigned={unsigned_error}; signed={signed_error}"
                )
            });
            u64::try_from(signed).expect("value must not be negative")
        }
    }
}

/// 共享夹具被多个测试 target 使用不同子集；未使用方不触发 dead_code。
#[allow(dead_code)]
pub async fn scalar_string(
    db: &DatabaseConnection,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> String {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            values,
        ))
        .await
        .expect("query")
        .expect("row");
    row.try_get::<String>("", "value")
        .expect("value must decode as string")
}

pub fn account(subject: &str) -> SourceAccountRef {
    SourceAccountRef::new(MessageSource::NapCat, subject).expect("valid account")
}

/// 通过真实入站路径插入群消息：自动建立账号、会话、来源事件和正文投影。
#[allow(clippy::too_many_arguments)]
pub async fn insert_group_message(
    inbound: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    managed_id: &str,
    message_id: &str,
    conversation: &str,
    actor_id: &str,
    actor_kind: VerifiedActorKind,
    occurred_at_unix_secs: i64,
    text: &str,
) -> SourceEventId {
    inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(MessageSource::NapCat, managed_id, message_id).unwrap(),
                ConversationRef::new(ConversationKind::Group, conversation).unwrap(),
                VerifiedActor::new(actor_kind, actor_id).unwrap(),
                occurred_at_unix_secs,
                text,
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .expect("insert message")
        .source_event_id()
        .clone()
}

/// 插入 OwnerCommand（qq_open_platform + owner_control + Owner actor）并建立
/// active OwnerBinding，返回命令事件 ID。
pub async fn owner_command_with_binding(
    db: &DatabaseConnection,
    inbound: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    managed_id: &str,
    command_account_id: &str,
    message_id: &str,
    text: &str,
    occurred_at_unix_secs: i64,
) -> SourceEventId {
    let command_event_id = inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(
                    MessageSource::QqOpenPlatform,
                    command_account_id,
                    message_id,
                )
                .unwrap(),
                ConversationRef::new(ConversationKind::OwnerControl, "owner-conv").unwrap(),
                VerifiedActor::new(VerifiedActorKind::Owner, "owner-openid").unwrap(),
                occurred_at_unix_secs,
                text,
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .expect("insert owner command")
        .source_event_id()
        .clone();
    let inserted = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_owner_bindings \
             (binding_id, managed_account_id, command_account_id, owner_actor_id, status) \
             SELECT ?, managed.id, command.id, 'owner-openid', 'active' \
             FROM secretary_accounts managed JOIN secretary_accounts command \
             WHERE managed.source_channel = 'napcat' AND managed.platform_account_id = ? \
               AND command.source_channel = 'qq_open_platform' \
               AND command.platform_account_id = ?",
            vec![
                Uuid::new_v4().to_string().into(),
                managed_id.to_owned().into(),
                command_account_id.to_owned().into(),
            ],
        ))
        .await
        .expect("create binding");
    assert_eq!(
        inserted.rows_affected(),
        1,
        "exactly one active OwnerBinding"
    );
    command_event_id
}

/// 插入 running Action Run（含 lease token）。
pub async fn insert_action_run(
    db: &DatabaseConnection,
    account: &SourceAccountRef,
    run_id: &ActionRunId,
    command_source_event_id: &SourceEventId,
    lease_token: &ActionLeaseToken,
) {
    let updated = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_action_runs \
             (run_id, account_id, command_source_event_id, command_text, conversation_id, \
              occurred_at_unix_secs, timezone_offset_secs, timezone_name, recent_events_json, \
              status, lease_token, lease_expires_at) \
             SELECT ?, id, ?, '完成跟进', 'owner-conv', ?, 0, 'UTC', JSON_ARRAY(), \
                    'running', ?, UTC_TIMESTAMP(6) + INTERVAL 60 SECOND \
             FROM secretary_accounts \
             WHERE source_channel = ? AND platform_account_id = ?",
            vec![
                run_id.as_str().into(),
                command_source_event_id.as_str().into(),
                SystemClock.now_unix_secs().into(),
                lease_token.as_str().into(),
                account.channel.as_str().into(),
                account.account_id.clone().into(),
            ],
        ))
        .await
        .expect("insert action run");
    assert_eq!(updated.rows_affected(), 1, "action run must be inserted");
}
