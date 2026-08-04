//! MySQL 目录快照仓储：实现 [`crate::DirectoryStoreT`]。
//!
//! 快照绑定 `account_id`，有账号作用域索引；幂等（相同 snapshot_id 不重复写入）；
//! 跨重启恢复。JSON 列用 `CAST(... AS CHAR)` 读取后 `serde_json::from_str`（CLAUDE.md 教训）。
//! 平台 ID 以字符串保留精度，BIGINT UNSIGNED 用 `u64`。

use async_trait::async_trait;
use sea_orm::{
    DatabaseBackend, DatabaseConnection, FromQueryResult, Statement, TransactionTrait, Value,
};
use tracing::{debug, warn};

use crate::{
    ConversationKind, ConversationRef, DirectoryEvidence, DirectorySnapshot, DirectorySnapshotId,
    DirectorySourceApi, DirectoryStatus, DirectoryStoreError, DirectoryStoreT, IngestionGapId,
    ScopeBoundary, ScopeKind, SourceAccountRef,
};

use super::mysql_retriever::resolve_account_id;

/// 把 `sea_orm::DbErr` 转为 `DirectoryStoreError`（复用 `From` 实现）。
fn db_err(e: sea_orm::DbErr) -> DirectoryStoreError {
    DirectoryStoreError::Database(e.to_string())
}

/// 把 `Option<String>` 转为 `sea_orm::Value`（None -> SQL NULL）。
fn opt_str_to_value(s: Option<String>) -> Value {
    match s {
        Some(v) => v.into(),
        None => Value::Bool(None),
    }
}

pub(crate) struct MySqlDirectoryStore {
    db: DatabaseConnection,
}

impl MySqlDirectoryStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

/// 快照行（不含 Scope 列表）。
#[derive(sea_orm::FromQueryResult)]
struct SnapshotRow {
    snapshot_id: String,
    account_id: u64,
    source_api: String,
    status: String,
    evidence_json: String,
    #[allow(dead_code)]
    scope_count: u32,
    created_at_unix_secs: i64,
}

/// Scope 行。
#[derive(sea_orm::FromQueryResult)]
struct ScopeRow {
    scope_kind: String,
    conversation_kind: String,
    platform_conversation_id: String,
    boundary_message_id: Option<String>,
    boundary_msg_time: Option<String>,
    display_name: Option<String>,
}

impl MySqlDirectoryStore {
    /// 把 `DirectorySnapshot` 序列化为 JSON（不存储完整 API 响应，只存聚合证据）。
    fn serialize_evidence(evidence: &DirectoryEvidence) -> Result<String, DirectoryStoreError> {
        serde_json::to_string(evidence).map_err(|e| {
            DirectoryStoreError::InvalidData(format!("failed to serialize evidence: {e}"))
        })
    }

    /// 从行重建 `DirectorySnapshot`。
    async fn load_snapshot_from_row(
        &self,
        row: SnapshotRow,
    ) -> Result<DirectorySnapshot, DirectoryStoreError> {
        let snapshot_id = DirectorySnapshotId::new(&row.snapshot_id)
            .map_err(|e| DirectoryStoreError::InvalidData(e.to_string()))?;
        let source_api = DirectorySourceApi::parse_from_str(&row.source_api).ok_or_else(|| {
            DirectoryStoreError::InvalidData(format!("unknown source_api: {}", row.source_api))
        })?;
        let status = DirectoryStatus::parse_from_str(&row.status).ok_or_else(|| {
            DirectoryStoreError::InvalidData(format!("unknown status: {}", row.status))
        })?;
        let evidence: DirectoryEvidence =
            serde_json::from_str(&row.evidence_json).map_err(|e| {
                DirectoryStoreError::Database(format!("failed to parse evidence_json: {e}"))
            })?;

        // 读取 Scope 列表。
        let scope_rows = ScopeRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT scope_kind, conversation_kind, platform_conversation_id,
                      boundary_message_id, boundary_msg_time, display_name
               FROM secretary_directory_scopes
               WHERE snapshot_id = ?
               ORDER BY id"#,
            [row.snapshot_id.clone().into()],
        ))
        .all(&self.db)
        .await
        .map_err(db_err)?;

        let scopes = scope_rows
            .into_iter()
            .filter_map(|sr| {
                let conv_kind = match sr.conversation_kind.as_str() {
                    "private" => ConversationKind::Private,
                    "group" => ConversationKind::Group,
                    "owner_control" => ConversationKind::OwnerControl,
                    _ => return None,
                };
                let conv = ConversationRef::new(conv_kind, sr.platform_conversation_id).ok()?;
                let scope_kind = ScopeKind::parse_from_str(&sr.scope_kind)?;
                let boundary = match (sr.boundary_message_id, sr.boundary_msg_time) {
                    (Some(id), Some(time)) => Some(ScopeBoundary::new(id, time)),
                    _ => None,
                };
                Some(crate::ConversationScope {
                    conversation: conv,
                    scope_kind,
                    boundary,
                    display_name: sr.display_name,
                })
            })
            .collect();

        // 重建 account_ref（从 account_id 反查）。
        let account = self.resolve_account_ref(row.account_id).await?;

        Ok(DirectorySnapshot {
            snapshot_id,
            account,
            source_api,
            status,
            evidence,
            scopes,
            created_at_unix_secs: row.created_at_unix_secs,
        })
    }

    /// 通过 account_id 反查 SourceAccountRef。
    async fn resolve_account_ref(
        &self,
        account_id: u64,
    ) -> Result<SourceAccountRef, DirectoryStoreError> {
        #[derive(sea_orm::FromQueryResult)]
        struct AccountRow {
            source_channel: String,
            platform_account_id: String,
        }
        let row = AccountRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT source_channel, platform_account_id FROM secretary_accounts WHERE id = ?",
            [account_id.into()],
        ))
        .one(&self.db)
        .await
        .map_err(db_err)?
        .ok_or_else(|| {
            DirectoryStoreError::InvalidData(format!("account_id {account_id} not found"))
        })?;

        let channel = match row.source_channel.as_str() {
            "napcat" => crate::MessageSource::NapCat,
            "qq_open_platform" => crate::MessageSource::QqOpenPlatform,
            _ => {
                return Err(DirectoryStoreError::InvalidData(format!(
                    "unknown source_channel: {}",
                    row.source_channel
                )));
            }
        };
        SourceAccountRef::new(channel, row.platform_account_id)
            .map_err(|e| DirectoryStoreError::InvalidData(format!("invalid account ref: {e}")))
    }
}

#[async_trait]
impl DirectoryStoreT for MySqlDirectoryStore {
    async fn snapshot_directory(
        &self,
        snapshot: &DirectorySnapshot,
    ) -> Result<(), DirectoryStoreError> {
        let account_id = resolve_account_id(&self.db, &snapshot.account)
            .await
            .map_err(|e| DirectoryStoreError::Database(e.to_string()))?;
        let evidence_json = Self::serialize_evidence(&snapshot.evidence)?;
        let scope_count = snapshot.scopes.len() as u32;

        let txn = self
            .db
            .begin()
            .await
            .map_err(|e| DirectoryStoreError::Database(e.to_string()))?;

        // 幂等：相同 snapshot_id 不重复写入。
        let existing: Option<SnapshotRow> =
            SnapshotRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"SELECT snapshot_id, account_id, source_api, status,
                          CAST(evidence_json AS CHAR) AS evidence_json,
                          scope_count, created_at_unix_secs
                   FROM secretary_directory_snapshots
                   WHERE snapshot_id = ?"#,
                [snapshot.snapshot_id.as_str().into()],
            ))
            .one(&txn)
            .await
            .map_err(db_err)?;

        if existing.is_some() {
            txn.commit()
                .await
                .map_err(|e| DirectoryStoreError::Database(e.to_string()))?;
            debug!(
                snapshot_id = snapshot.snapshot_id.as_str(),
                "目录快照已存在，幂等跳过"
            );
            return Ok(());
        }

        // 插入快照。
        sea_orm::ConnectionTrait::execute_raw(
            &txn,
            Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT INTO secretary_directory_snapshots
                   (snapshot_id, account_id, source_api, status, evidence_json,
                    scope_count, created_at_unix_secs)
                   VALUES (?, ?, ?, ?, ?, ?, ?)"#,
                [
                    snapshot.snapshot_id.as_str().into(),
                    account_id.into(),
                    snapshot.source_api.as_str().into(),
                    snapshot.status.as_str().into(),
                    evidence_json.into(),
                    scope_count.into(),
                    snapshot.created_at_unix_secs.into(),
                ],
            ),
        )
        .await
        .map_err(db_err)?;

        // 批量插入 Scope 条目。
        for scope in &snapshot.scopes {
            let (boundary_msg_id, boundary_msg_time) = match &scope.boundary {
                Some(b) => (Some(b.message_id.clone()), Some(b.msg_time.clone())),
                None => (None, None),
            };
            sea_orm::ConnectionTrait::execute_raw(
                &txn,
                Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"INSERT IGNORE INTO secretary_directory_scopes
                       (snapshot_id, account_id, scope_kind, conversation_kind,
                        platform_conversation_id, boundary_message_id, boundary_msg_time,
                        display_name)
                       VALUES (?, ?, ?, ?, ?, ?, ?, ?)"#,
                    [
                        snapshot.snapshot_id.as_str().into(),
                        account_id.into(),
                        scope.scope_kind.as_str().into(),
                        scope.conversation.kind.as_str().into(),
                        scope.conversation.id.clone().into(),
                        opt_str_to_value(boundary_msg_id),
                        opt_str_to_value(boundary_msg_time),
                        opt_str_to_value(scope.display_name.clone()),
                    ],
                ),
            )
            .await
            .map_err(db_err)?;
        }

        txn.commit()
            .await
            .map_err(|e| DirectoryStoreError::Database(e.to_string()))?;

        debug!(
            snapshot_id = snapshot.snapshot_id.as_str(),
            account_id,
            scope_count,
            status = snapshot.status.as_str(),
            "目录快照已持久化"
        );
        Ok(())
    }

    async fn load_latest_snapshot(
        &self,
        account: &SourceAccountRef,
    ) -> Result<Option<DirectorySnapshot>, DirectoryStoreError> {
        let account_id = resolve_account_id(&self.db, account)
            .await
            .map_err(|e| DirectoryStoreError::Database(e.to_string()))?;

        let row = SnapshotRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT snapshot_id, account_id, source_api, status,
                      CAST(evidence_json AS CHAR) AS evidence_json,
                      scope_count, created_at_unix_secs
               FROM secretary_directory_snapshots
               WHERE account_id = ?
               ORDER BY created_at_unix_secs DESC, snapshot_id DESC
               LIMIT 1"#,
            [account_id.into()],
        ))
        .one(&self.db)
        .await
        .map_err(db_err)?;

        match row {
            Some(row) => Ok(Some(self.load_snapshot_from_row(row).await?)),
            None => Ok(None),
        }
    }

    async fn freeze_for_gap(
        &self,
        gap_id: &IngestionGapId,
        account: &SourceAccountRef,
    ) -> Result<Option<DirectorySnapshot>, DirectoryStoreError> {
        let account_id = resolve_account_id(&self.db, account)
            .await
            .map_err(|e| DirectoryStoreError::Database(e.to_string()))?;

        // 读取最新快照。
        let row = SnapshotRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT snapshot_id, account_id, source_api, status,
                      CAST(evidence_json AS CHAR) AS evidence_json,
                      scope_count, created_at_unix_secs
               FROM secretary_directory_snapshots
               WHERE account_id = ?
               ORDER BY created_at_unix_secs DESC, snapshot_id DESC
               LIMIT 1"#,
            [account_id.into()],
        ))
        .one(&self.db)
        .await
        .map_err(db_err)?;

        let Some(row) = row else {
            warn!(
                gap_id = gap_id.as_str(),
                "无目录快照可冻结，Gap 将无目录证据"
            );
            return Ok(None);
        };

        // 冻结快照引用到 Gap。首写获胜：已有冻结不得被后续目录同步覆盖。
        sea_orm::ConnectionTrait::execute_raw(
            &self.db,
            Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT INTO secretary_directory_gap_freeze (gap_id, snapshot_id, account_id)
                   VALUES (?, ?, ?)
                   ON DUPLICATE KEY UPDATE gap_id = gap_id"#,
                [
                    gap_id.as_str().into(),
                    row.snapshot_id.clone().into(),
                    account_id.into(),
                ],
            ),
        )
        .await
        .map_err(db_err)?;

        let frozen = self.load_frozen_for_gap(gap_id).await?;
        debug!(
            gap_id = gap_id.as_str(),
            snapshot_id = frozen
                .as_ref()
                .map(|snapshot| snapshot.snapshot_id.as_str()),
            "目录快照冻结结果已从持久化引用回读"
        );
        Ok(frozen)
    }

    async fn load_frozen_for_gap(
        &self,
        gap_id: &IngestionGapId,
    ) -> Result<Option<DirectorySnapshot>, DirectoryStoreError> {
        #[derive(sea_orm::FromQueryResult)]
        struct FreezeRow {
            snapshot_id: Option<String>,
        }
        let freeze = FreezeRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT snapshot_id FROM secretary_directory_gap_freeze WHERE gap_id = ?",
            [gap_id.as_str().into()],
        ))
        .one(&self.db)
        .await
        .map_err(db_err)?;

        let Some(snapshot_id) = freeze.and_then(|row| row.snapshot_id) else {
            return Ok(None);
        };

        let row = SnapshotRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT snapshot_id, account_id, source_api, status,
                      CAST(evidence_json AS CHAR) AS evidence_json,
                      scope_count, created_at_unix_secs
               FROM secretary_directory_snapshots
               WHERE snapshot_id = ?"#,
            [snapshot_id.into()],
        ))
        .one(&self.db)
        .await
        .map_err(db_err)?;

        match row {
            Some(row) => Ok(Some(self.load_snapshot_from_row(row).await?)),
            None => Ok(None),
        }
    }

    async fn has_valid_snapshot(
        &self,
        account: &SourceAccountRef,
        ttl_secs: u64,
        now_unix_secs: i64,
    ) -> Result<bool, DirectoryStoreError> {
        let account_id = resolve_account_id(&self.db, account)
            .await
            .map_err(|e| DirectoryStoreError::Database(e.to_string()))?;

        #[derive(sea_orm::FromQueryResult)]
        struct CountRow {
            cnt: i64,
        }
        let row = CountRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT COUNT(*) AS cnt
               FROM secretary_directory_snapshots
               WHERE account_id = ? AND created_at_unix_secs >= ?"#,
            [account_id.into(), (now_unix_secs - ttl_secs as i64).into()],
        ))
        .one(&self.db)
        .await
        .map_err(db_err)?;

        Ok(row.map(|r| r.cnt > 0).unwrap_or(false))
    }
}
