//! QQBot Baseline v2 的隔离 MySQL 验收。
//!
//! 需要 QQBOT_TEST_DATABASE_URL 指向 `qqbot_accept_*` 测试 schema。测试只创建并精确删除随机
//! 派生 schema，不修改配置 URL 指向的基础 schema。

mod common;

use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const CURRENT_BASELINE_RECORD: &str = "baseline:20260806_qqbot_schema_v2.sql";
const LEGACY_BASELINE_RECORD: &str = "baseline:20260803_qqbot_schema_v1.sql";
const FOLDED_MIGRATIONS: &[&str] = &[
    "20260804_qqbot_reply_reconcile.sql",
    "20260805_qqbot_realtime_spool_recovery.sql",
    "20260806_qqbot_artifact_reprocess.sql",
    "20260806_qqbot_non_message_history_signals.sql",
    "20260806_qqbot_notification_reconciliation_seed.sql",
    "20260806_qqbot_thread_decision_revision_paging.sql",
    "20260806_qqbot_thread_link_structured_references.sql",
    "20260806_qqbot_thread_semantic_reconfirmation.sql",
];

#[tokio::test]
#[ignore]
async fn baseline_v2_load_upgrade_and_fail_closed_paths_are_equivalent() {
    let base_url = std::env::var("QQBOT_TEST_DATABASE_URL").expect("QQBOT_TEST_DATABASE_URL");
    let base_schema = base_url
        .split('?')
        .next()
        .and_then(|value| value.rsplit('/').next())
        .unwrap_or_default();
    assert!(base_schema.starts_with("qqbot_accept_"));

    let suffix = &Uuid::new_v4().simple().to_string()[..10];
    let schemas = ["v2", "v1", "legacy", "partial"].map(|kind| {
        let tail = format!("_dbv2_{kind}_{suffix}");
        let keep = 64usize.checked_sub(tail.len()).unwrap();
        format!("{}{}", &base_schema[..base_schema.len().min(keep)], tail)
    });
    let admin = Database::connect(&base_url)
        .await
        .expect("connect test admin");
    for schema in &schemas {
        admin
            .execute_unprepared(&format!("CREATE DATABASE `{schema}`"))
            .await
            .expect("create isolated schema");
    }

    let task_base_url = base_url.clone();
    let task_schemas = schemas.clone();
    let task = tokio::spawn(async move {
        let database_dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../qqbot-server/database");
        let migrations_dir = database_dir.join("migrations");

        let fresh = connect_schema(&task_base_url, &task_schemas[0]).await;
        common::try_apply_qqbot_migrations(&fresh)
            .await
            .expect("fresh v2 baseline must load");
        common::try_apply_qqbot_migrations(&fresh)
            .await
            .expect("v2 baseline reload must be idempotent");
        assert_schema_shape(&fresh, 84, 2).await;
        assert_eq!(record_count(&fresh, CURRENT_BASELINE_RECORD).await, 1);
        assert_eq!(folded_record_count(&fresh).await, 0);
        assert_seeded(&fresh).await;

        let upgraded_v1 = connect_schema(&task_base_url, &task_schemas[1]).await;
        apply_sql_file(
            &upgraded_v1,
            &database_dir.join("archive/pre_v2/20260803_qqbot_schema_v1.sql"),
        )
        .await;
        create_ledger(&upgraded_v1).await;
        record(&upgraded_v1, LEGACY_BASELINE_RECORD).await;
        common::try_apply_qqbot_migrations(&upgraded_v1)
            .await
            .expect("v1 baseline must upgrade through archived migrations");
        assert_eq!(record_count(&upgraded_v1, CURRENT_BASELINE_RECORD).await, 1);
        assert_eq!(folded_record_count(&upgraded_v1).await, 8);
        assert_seeded(&upgraded_v1).await;
        assert_schema_equivalent(&upgraded_v1, &fresh).await;

        let upgraded_legacy = connect_schema(&task_base_url, &task_schemas[2]).await;
        apply_sql_file(
            &upgraded_legacy,
            &database_dir.join("archive/pre_v2/20260803_qqbot_schema_v1.sql"),
        )
        .await;
        create_ledger(&upgraded_legacy).await;
        let mut old_migrations = sql_files(&database_dir.join("archive/pre_v1"));
        old_migrations.sort();
        assert_eq!(old_migrations.len(), 33);
        for migration in old_migrations {
            record(
                &upgraded_legacy,
                migration.file_name().unwrap().to_str().unwrap(),
            )
            .await;
        }
        common::try_apply_qqbot_migrations(&upgraded_legacy)
            .await
            .expect("complete pre-v1 chain must upgrade to v2");
        assert_eq!(
            record_count(&upgraded_legacy, CURRENT_BASELINE_RECORD).await,
            1
        );
        assert_eq!(folded_record_count(&upgraded_legacy).await, 8);
        assert_schema_equivalent(&upgraded_legacy, &fresh).await;

        let partial = connect_schema(&task_base_url, &task_schemas[3]).await;
        partial
            .execute_unprepared(
                "CREATE TABLE secretary_partial (id BIGINT NOT NULL PRIMARY KEY) ENGINE=InnoDB",
            )
            .await
            .expect("create unmanaged partial object");
        let error = common::try_apply_qqbot_migrations(&partial)
            .await
            .expect_err("unmanaged secretary object must fail closed");
        assert!(
            error.contains("refusing to apply v2"),
            "unexpected error: {error}"
        );
        assert_eq!(record_count(&partial, CURRENT_BASELINE_RECORD).await, 0);

        assert_eq!(
            sql_files(&migrations_dir)
                .into_iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["20260807_qqbot_owner_response_delivery.sql"]
        );
    });

    let result = task.await;
    for schema in &schemas {
        if let Err(error) = admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS `{schema}`"))
            .await
        {
            eprintln!("failed to clean baseline-v2 schema: {error}");
        }
    }
    result.expect("baseline-v2 scenario task must complete");
}

async fn connect_schema(base_url: &str, schema: &str) -> DatabaseConnection {
    let (prefix, _) = base_url.rsplit_once('/').expect("database URL");
    Database::connect(format!("{prefix}/{schema}"))
        .await
        .expect("connect isolated schema")
}

fn sql_files(directory: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(directory)
        .expect("read SQL directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
        .collect()
}

async fn apply_sql_file(db: &DatabaseConnection, path: &Path) {
    let sql = std::fs::read_to_string(path).expect("read migration SQL");
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
        db.execute_unprepared(statement)
            .await
            .unwrap_or_else(|error| panic!("{} failed: {error}", path.display()));
    }
}

async fn create_ledger(db: &DatabaseConnection) {
    db.execute_unprepared(
        "CREATE TABLE qqbot_test_schema_migrations (\
         migration_name VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL PRIMARY KEY, \
         applied_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)) \
         ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci",
    )
    .await
    .expect("create migration ledger");
}

async fn record(db: &DatabaseConnection, migration_name: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO qqbot_test_schema_migrations (migration_name) VALUES (?)",
        [migration_name.into()],
    ))
    .await
    .expect("record migration");
}

async fn record_count(db: &DatabaseConnection, migration_name: &str) -> u64 {
    common::scalar_u64(
        db,
        "SELECT COUNT(*) AS value FROM qqbot_test_schema_migrations WHERE migration_name = ?",
        vec![migration_name.into()],
    )
    .await
}

async fn folded_record_count(db: &DatabaseConnection) -> u64 {
    let placeholders = std::iter::repeat_n("?", FOLDED_MIGRATIONS.len())
        .collect::<Vec<_>>()
        .join(",");
    common::scalar_u64(
        db,
        &format!(
            "SELECT COUNT(*) AS value FROM qqbot_test_schema_migrations \
             WHERE migration_name IN ({placeholders})"
        ),
        FOLDED_MIGRATIONS
            .iter()
            .map(|name| (*name).into())
            .collect(),
    )
    .await
}

async fn assert_seeded(db: &DatabaseConnection) {
    assert_eq!(
        common::scalar_u64(
            db,
            "SELECT COUNT(*) AS value FROM secretary_notification_reconciliation_leases \
             WHERE lease_name = 'legacy_owner_outbox_v1'",
            Vec::new(),
        )
        .await,
        1
    );
}

async fn assert_schema_shape(db: &DatabaseConnection, tables: u64, views: u64) {
    assert_eq!(
        common::scalar_u64(
            db,
            "SELECT COUNT(*) AS value FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE' \
             AND TABLE_NAME LIKE 'secretary\\_%' ESCAPE '\\\\'",
            Vec::new(),
        )
        .await,
        tables
    );
    assert_eq!(
        common::scalar_u64(
            db,
            "SELECT COUNT(*) AS value FROM information_schema.VIEWS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME LIKE 'secretary\\_%' ESCAPE '\\\\'",
            Vec::new(),
        )
        .await,
        views
    );
}

async fn schema_definitions(db: &DatabaseConnection) -> BTreeMap<String, String> {
    let rows = db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::MySql,
            "SELECT TABLE_NAME, TABLE_TYPE FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME LIKE 'secretary\\_%' ESCAPE '\\\\' \
             ORDER BY TABLE_NAME",
        ))
        .await
        .expect("list secretary objects");
    let mut definitions = BTreeMap::new();
    for row in rows {
        let name: String = row.try_get("", "TABLE_NAME").expect("table name");
        let object_type: String = row.try_get("", "TABLE_TYPE").expect("table type");
        let create = db
            .query_one_raw(Statement::from_string(
                DatabaseBackend::MySql,
                format!(
                    "SHOW CREATE {} `{name}`",
                    if object_type == "VIEW" {
                        "VIEW"
                    } else {
                        "TABLE"
                    }
                ),
            ))
            .await
            .expect("show create")
            .expect("show create row");
        let column = if object_type == "VIEW" {
            "Create View"
        } else {
            "Create Table"
        };
        let ddl: String = create.try_get("", column).expect("create DDL");
        definitions.insert(name, normalize_ddl(&ddl));
    }
    definitions
}

async fn assert_schema_equivalent(left: &DatabaseConnection, right: &DatabaseConnection) {
    let left = schema_definitions(left).await;
    let right = schema_definitions(right).await;
    assert_eq!(
        left.keys().collect::<Vec<_>>(),
        right.keys().collect::<Vec<_>>()
    );
    let different = left
        .iter()
        .filter_map(|(name, ddl)| (right.get(name) != Some(ddl)).then_some(name.as_str()))
        .collect::<Vec<_>>();
    if let Some(name) = different.first() {
        let left_lines = left[*name]
            .lines()
            .collect::<std::collections::BTreeSet<_>>();
        let right_lines = right[*name]
            .lines()
            .collect::<std::collections::BTreeSet<_>>();
        panic!(
            "schema definition differs for {name}; left-only={:?}; right-only={:?}",
            left_lines.difference(&right_lines).collect::<Vec<_>>(),
            right_lines.difference(&left_lines).collect::<Vec<_>>()
        );
    }
    assert!(
        different.is_empty(),
        "schema definitions differ for: {}",
        different.join(", ")
    );
}

fn normalize_ddl(ddl: &str) -> String {
    // MySQL may rewrite an ASCII-column CHECK literal from the connection's utf8mb4
    // introducer to `_ascii` while rebuilding the table for a later FK. The value and
    // comparison semantics are identical, so canonicalize only this display difference.
    let ddl = ddl.replace("\r\n", "\n").replace("_ascii'", "_utf8mb4'");
    if !ddl.starts_with("CREATE TABLE") {
        return ddl
            .split_whitespace()
            .filter(|part| !part.starts_with("DEFINER="))
            .collect::<Vec<_>>()
            .join(" ");
    }
    let mut lines = ddl.lines();
    let first = lines.next().unwrap().trim().to_owned();
    let mut body = Vec::new();
    let mut tail = String::new();
    for line in lines {
        if line.starts_with(") ENGINE=") {
            tail = line.trim().to_owned();
        } else {
            body.push(line.trim().trim_end_matches(',').to_owned());
        }
    }
    body.sort();
    format!("{first}\n{}\n{tail}", body.join("\n"))
}
