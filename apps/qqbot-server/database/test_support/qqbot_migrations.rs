//! QQBot MySQL 测试共用 Schema 加载器。
//!
//! 全新 schema 只执行 Baseline v2 与其后的增量迁移。Baseline v1 或压缩前完整 33 迁移链
//! 会从 pre-v2 归档补齐折叠迁移后登记采用 v2；部分迁移或无记录的既有业务表会 fail-closed，
//! 避免在未知结构上误跑基线。

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use std::sync::OnceLock;
use tokio::sync::Mutex;

const MIGRATION_RECORDS_TABLE: &str = "qqbot_test_schema_migrations";
const CURRENT_BASELINE_FILE_NAME: &str = "20260806_qqbot_schema_v2.sql";
const CURRENT_BASELINE_RECORD_NAME: &str = "baseline:20260806_qqbot_schema_v2.sql";
const LEGACY_BASELINE_RECORD_NAME: &str = "baseline:20260803_qqbot_schema_v1.sql";
static MIGRATION_LOAD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

const FOLDED_V1_TO_V2_MIGRATIONS: &[&str] = &[
    "20260804_qqbot_reply_reconcile.sql",
    "20260805_qqbot_realtime_spool_recovery.sql",
    "20260806_qqbot_artifact_reprocess.sql",
    "20260806_qqbot_non_message_history_signals.sql",
    "20260806_qqbot_notification_reconciliation_seed.sql",
    "20260806_qqbot_thread_decision_revision_paging.sql",
    "20260806_qqbot_thread_link_structured_references.sql",
    "20260806_qqbot_thread_semantic_reconfirmation.sql",
];

const PRE_V1_MIGRATIONS: &[&str] = &[
    "20260723_personal_secretary_ingestion.sql",
    "20260723_personal_secretary_continuity.sql",
    "20260723_personal_secretary_backfill.sql",
    "20260724_personal_secretary_threads.sql",
    "20260724_personal_secretary_thread_links.sql",
    "20260724_personal_secretary_thread_semantics.sql",
    "20260724_personal_secretary_thread_mutations.sql",
    "20260724_personal_secretary_thread_revisions.sql",
    "20260724_personal_secretary_memory.sql",
    "20260724_personal_secretary_memory_controls_followups.sql",
    "20260724_personal_secretary_qq_open_platform.sql",
    "20260725_personal_secretary_action_planner.sql",
    "20260726_personal_secretary_action_planner_hardening.sql",
    "20260726_personal_secretary_directory.sql",
    "20260727_personal_secretary_gap_freeze_hardening.sql",
    "20260726_personal_secretary_event_type_recall.sql",
    "20260726_personal_secretary_recall.sql",
    "20260726_personal_secretary_artifacts.sql",
    "20260727_personal_secretary_recall_inbox.sql",
    "20260727_personal_secretary_artifact_derivations.sql",
    "20260727_personal_secretary_owner_agenda.sql",
    "20260728_owner_notification_policy_feedback_v1.sql",
    "20260729_owner_notification_policy_evaluation_v1.sql",
    "20260731_personal_secretary_response_expectations.sql",
    "20260801_personal_secretary_follow_up_owner_controls.sql",
    "20260801_personal_secretary_follow_up_snooze.sql",
    "20260801_personal_secretary_follow_up_batch_controls.sql",
    "20260801_personal_secretary_owner_work_close.sql",
    "20260802_personal_secretary_memory_candidates.sql",
    "20260802_personal_secretary_participant_context.sql",
    "20260729_owner_notification_policy_task7_reconciliation.sql",
    "20260731_personal_secretary_project_blocker_followups.sql",
    "20260731_personal_secretary_thread_owner_controls.sql",
];

pub async fn apply_qqbot_migrations(db: &DatabaseConnection, migrations_dir: &std::path::Path) {
    try_apply_qqbot_migrations(db, migrations_dir)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}

/// 应用全部待执行迁移，并在某个增量 SQL 失败时保留错误供负向测试断言。
/// 迁移记录只在对应 SQL 文件全部成功后写入。
pub async fn try_apply_qqbot_migrations(
    db: &DatabaseConnection,
    migrations_dir: &std::path::Path,
) -> Result<(), String> {
    let _guard = MIGRATION_LOAD_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .await;
    ensure_migration_records_table(db).await;

    let database_dir = migrations_dir.parent().unwrap_or_else(|| {
        panic!(
            "migrations directory has no database parent: {}",
            migrations_dir.display()
        )
    });
    try_ensure_baseline(db, database_dir).await?;

    let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(migrations_dir)
        .map_err(|error| format!("failed to read migrations directory: {error}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
        .collect();
    entries.sort();

    for path in entries {
        let migration_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("migration name is not valid UTF-8: {}", path.display()))?;
        if migration_is_applied(db, migration_name).await {
            continue;
        }
        try_apply_sql_file(db, &path).await?;
        try_record_migration(db, migration_name).await?;
    }
    assert_reconciliation_lease_seeded(db).await;
    Ok(())
}

/// 显式重放一个已折叠迁移，仅供迁移幂等和结构漂移负向测试使用。
#[allow(dead_code)]
pub async fn try_replay_folded_migration(
    db: &DatabaseConnection,
    migrations_dir: &std::path::Path,
    migration_name: &str,
) -> Result<(), String> {
    let _guard = MIGRATION_LOAD_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .await;
    ensure_migration_records_table(db).await;
    if !FOLDED_V1_TO_V2_MIGRATIONS.contains(&migration_name) {
        return Err(format!(
            "migration is not part of the v1-to-v2 archive: {migration_name}"
        ));
    }
    let database_dir = migrations_dir.parent().ok_or_else(|| {
        format!(
            "migrations directory has no database parent: {}",
            migrations_dir.display()
        )
    })?;
    try_apply_sql_file(
        db,
        &database_dir.join("archive/pre_v2").join(migration_name),
    )
    .await?;
    if !migration_is_applied(db, migration_name).await {
        try_record_migration(db, migration_name).await?;
    }
    Ok(())
}

async fn try_ensure_baseline(
    db: &DatabaseConnection,
    database_dir: &std::path::Path,
) -> Result<(), String> {
    if migration_is_applied(db, CURRENT_BASELINE_RECORD_NAME).await {
        return Ok(());
    }

    let has_legacy_baseline = migration_is_applied(db, LEGACY_BASELINE_RECORD_NAME).await;

    let applied_legacy = count_applied_legacy_migrations(db).await;
    if has_legacy_baseline || applied_legacy == PRE_V1_MIGRATIONS.len() {
        apply_folded_v1_to_v2_migrations(db, database_dir).await?;
        try_record_migration(db, CURRENT_BASELINE_RECORD_NAME).await?;
        return Ok(());
    }

    let object_count = secretary_object_count(db).await;
    if object_count != 0 || applied_legacy != 0 {
        return Err(format!(
            "QQBot schema contains {object_count} secretary_* objects and {applied_legacy}/{} pre-v1 migration records without an authoritative baseline; refusing to apply v2 over a partial or unmanaged schema",
            PRE_V1_MIGRATIONS.len()
        ));
    }

    let baseline_path = database_dir
        .join("baseline")
        .join(CURRENT_BASELINE_FILE_NAME);
    try_apply_sql_file(db, &baseline_path).await?;
    try_record_migration(db, CURRENT_BASELINE_RECORD_NAME).await?;
    Ok(())
}

async fn apply_folded_v1_to_v2_migrations(
    db: &DatabaseConnection,
    database_dir: &std::path::Path,
) -> Result<(), String> {
    let archive_dir = database_dir.join("archive/pre_v2");
    for migration_name in FOLDED_V1_TO_V2_MIGRATIONS {
        if migration_is_applied(db, migration_name).await {
            continue;
        }
        try_apply_sql_file(db, &archive_dir.join(migration_name)).await?;
        try_record_migration(db, migration_name).await?;
    }
    Ok(())
}

async fn assert_reconciliation_lease_seeded(db: &DatabaseConnection) {
    let seeded = db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::MySql,
            "SELECT 1 FROM secretary_notification_reconciliation_leases \
             WHERE lease_name = 'legacy_owner_outbox_v1' LIMIT 1",
        ))
        .await
        .unwrap_or_else(|error| {
            panic!("failed to verify notification reconciliation lease seed: {error}")
        })
        .is_some();
    assert!(
        seeded,
        "QQBot baseline must seed the legacy Owner Outbox reconciliation lease"
    );
}

async fn try_apply_sql_file(db: &DatabaseConnection, path: &std::path::Path) -> Result<(), String> {
    let sql = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let stripped = sql
        .lines()
        .map(|line| line.split_once("--").map_or(line, |(prefix, _)| prefix))
        .collect::<Vec<_>>()
        .join("\n");
    for (statement_index, statement) in stripped
        .split(';')
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
        .enumerate()
    {
        db.execute_raw(Statement::from_string(DatabaseBackend::MySql, statement))
            .await
            .map_err(|error| {
                format!(
                    "migration {} statement {} failed: {error}",
                    path.display(),
                    statement_index + 1
                )
            })?;
    }
    Ok(())
}

async fn ensure_migration_records_table(db: &DatabaseConnection) {
    db.execute_raw(Statement::from_string(
        DatabaseBackend::MySql,
        format!(
            "CREATE TABLE IF NOT EXISTS {MIGRATION_RECORDS_TABLE} (\
             migration_name VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL PRIMARY KEY, \
             applied_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)) \
             ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci"
        ),
    ))
    .await
    .unwrap_or_else(|error| panic!("failed to create migration records table: {error}"));
}

async fn try_record_migration(db: &DatabaseConnection, migration_name: &str) -> Result<(), String> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO qqbot_test_schema_migrations (migration_name) VALUES (?)",
        [migration_name.into()],
    ))
    .await
    .map_err(|error| format!("failed to record migration {migration_name}: {error}"))?;
    Ok(())
}

async fn migration_is_applied(db: &DatabaseConnection, migration_name: &str) -> bool {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT 1 FROM qqbot_test_schema_migrations WHERE migration_name = ? LIMIT 1",
        [migration_name.into()],
    ))
    .await
    .unwrap_or_else(|error| panic!("migration record query failed: {error}"))
    .is_some()
}

async fn count_applied_legacy_migrations(db: &DatabaseConnection) -> usize {
    let mut count = 0;
    for migration_name in PRE_V1_MIGRATIONS {
        if migration_is_applied(db, migration_name).await {
            count += 1;
        }
    }
    count
}

async fn secretary_object_count(db: &DatabaseConnection) -> u64 {
    let row = db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::MySql,
            "SELECT COUNT(*) AS value FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() AND LEFT(TABLE_NAME, 10) = 'secretary_'",
        ))
        .await
        .unwrap_or_else(|error| panic!("failed to inspect QQBot schema objects: {error}"))
        .expect("COUNT(*) must return one row");
    let count = row
        .try_get::<i64>("", "value")
        .unwrap_or_else(|error| panic!("failed to decode QQBot schema object count: {error}"));
    u64::try_from(count).expect("QQBot schema object count must be non-negative")
}
