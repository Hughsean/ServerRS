//! QQBot MySQL 测试共用迁移加载器。
//!
//! 测试 schema 的创建与销毁由外层脚本负责；本模块以迁移记录表保证重复加载不重复执行 DDL。

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use std::sync::OnceLock;
use tokio::sync::Mutex;

const MIGRATION_RECORDS_TABLE: &str = "qqbot_test_schema_migrations";
static MIGRATION_LOAD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub async fn apply_qqbot_migrations(db: &DatabaseConnection, migrations_dir: &std::path::Path) {
    // 同一测试进程可并行启动多个连接；迁移记录检查、DDL 与记录写入必须作为一个临界区。
    let _guard = MIGRATION_LOAD_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .await;
    ensure_migration_records_table(db).await;
    let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(migrations_dir)
        .unwrap_or_else(|error| panic!("failed to read migrations directory: {error}"))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
        .collect();
    entries.sort_by_key(|path| migration_order(path));

    for path in entries {
        let migration_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if migration_is_applied(db, migration_name).await {
            continue;
        }
        let sql = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let stripped = sql
            .lines()
            .map(|line| line.split_once("--").map_or(line, |(prefix, _)| prefix))
            .collect::<Vec<_>>()
            .join("\n");
        for statement in stripped
            .split(';')
            .map(str::trim)
            .filter(|statement| !statement.is_empty())
        {
            db.execute_raw(Statement::from_string(DatabaseBackend::MySql, statement))
                .await
                .unwrap_or_else(|error| panic!("migration {} failed: {error}", path.display()));
        }
        record_migration(db, migration_name).await;
    }
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

async fn record_migration(db: &DatabaseConnection, migration_name: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO qqbot_test_schema_migrations (migration_name) VALUES (?)",
        [migration_name.into()],
    ))
    .await
    .unwrap_or_else(|error| panic!("failed to record migration {migration_name}: {error}"));
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

fn migration_order(path: &std::path::Path) -> u8 {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    match name {
        name if name.contains("_ingestion.sql") => 0,
        name if name.contains("_continuity.sql") => 1,
        name if name.contains("_backfill.sql") => 2,
        name if name.contains("_threads.sql") => 3,
        name if name.contains("_thread_links.sql") => 4,
        name if name.contains("_thread_semantics.sql") => 5,
        name if name.contains("_thread_mutations.sql") => 6,
        name if name.contains("_thread_revisions.sql") => 7,
        name if name.contains("_memory.sql") => 8,
        name if name.contains("_memory_controls_followups.sql") => 9,
        name if name.contains("_qq_open_platform.sql") => 10,
        name if name.contains("_action_planner.sql") => 11,
        name if name.contains("_action_planner_hardening.sql") => 12,
        name if name.contains("_directory.sql") => 13,
        name if name.contains("_gap_freeze_hardening.sql") => 14,
        name if name.contains("_event_type_recall.sql") => 15,
        name if name.contains("_recall.sql") => 16,
        name if name.contains("_artifacts.sql") => 17,
        name if name.contains("_recall_inbox.sql") => 18,
        name if name.contains("_artifact_derivations.sql") => 19,
        name if name.contains("_owner_agenda.sql") => 20,
        name if name.contains("_owner_notification_policy_feedback_v1.sql") => 21,
        name if name.contains("_owner_notification_policy_evaluation_v1.sql") => 22,
        // ResponseExpectation 表被 owner_work_close 的外键引用，必须先建；
        // 不能再落 99（read_dir 顺序未定义）。
        name if name.contains("_response_expectations.sql") => 23,
        // FollowUp 控制审计先建表，snooze 扩展列、batch 复合唯一键依次执行；
        // 都落在 99 会退化为 read_dir 的未定义顺序。
        name if name.contains("_follow_up_owner_controls.sql") => 24,
        name if name.contains("_follow_up_snooze.sql") => 25,
        name if name.contains("_follow_up_batch_controls.sql") => 26,
        // 完成/关闭控制：FollowUp 审计约束扩展 + ResponseExpectation 审计表。
        name if name.contains("_owner_work_close.sql") => 27,
        // 记忆候选：依赖 accounts/source_events/message_contents/action_runs/memory_facts
        // 等既有表，排序在全部 FollowUp 控制迁移之后（28）。
        name if name.contains("_memory_candidates.sql") => 28,
        // 参与者档案与结构关系 VIEW：依赖 ingestion/threading/recall，排序在记忆候选之后（29）。
        name if name.contains("_participant_context.sql") => 29,
        _ => 99,
    }
}
