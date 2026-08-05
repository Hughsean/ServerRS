use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};

use crate::{
    SecretaryAction, SecretaryActionProposal, SecretaryActionReceipt, ThreadControlEffectRequest,
    ThreadControlStoreError, ThreadControlStoreT,
};

pub(crate) struct MySqlThreadControlStore {
    db: DatabaseConnection,
}

impl MySqlThreadControlStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ThreadControlStoreT for MySqlThreadControlStore {
    async fn apply_effect(
        &self,
        request: &ThreadControlEffectRequest,
    ) -> Result<SecretaryActionReceipt, ThreadControlStoreError> {
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

        let applied = apply_control(&transaction, request, account_id).await?;
        insert_control_audit(&transaction, request, account_id, &applied).await?;
        let inserted = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT IGNORE INTO secretary_action_effect_receipts \
                 (effect_id, run_id, proposal_json, result_ref) VALUES (?, ?, ?, ?)",
                [
                    request.effect_id.clone().into(),
                    request.run_id.as_str().into(),
                    request.proposal_json.clone().into(),
                    applied.result_ref.clone().into(),
                ],
            ))
            .await
            .map_err(database_error)?;
        if inserted.rows_affected() != 1 {
            let receipt = load_receipt(&transaction, request)
                .await?
                .ok_or(ThreadControlStoreError::Database)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(receipt);
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(SecretaryActionReceipt {
            proposal_id: request.proposal_id.clone(),
            result_ref: applied.result_ref,
            tool_kind: Some(request.action.kind()),
        })
    }
}

struct AppliedControl {
    thread_id: String,
    target_kind: &'static str,
    target_id: String,
    control_kind: &'static str,
    previous_status: String,
    current_status: String,
    reason: String,
    result_ref: String,
}

async fn apply_control<C: ConnectionTrait>(
    db: &C,
    request: &ThreadControlEffectRequest,
    account_id: u64,
) -> Result<AppliedControl, ThreadControlStoreError> {
    match &request.action {
        SecretaryAction::ConfirmThreadDecision { decision_id } => {
            let row = lock_decision(db, decision_id.as_str(), account_id).await?;
            if row.status != "proposed" && row.status != "confirmed" {
                return Err(ThreadControlStoreError::InvalidData(
                    "only a proposed decision can be confirmed".into(),
                ));
            }
            if row.status == "proposed" {
                update_status(
                    db,
                    "secretary_thread_decisions",
                    "decision_id",
                    decision_id.as_str(),
                    "proposed",
                    "confirmed",
                )
                .await?;
            }
            Ok(AppliedControl {
                thread_id: row.thread_id,
                target_kind: "decision",
                target_id: decision_id.as_str().to_owned(),
                control_kind: "confirm_decision",
                previous_status: row.status,
                current_status: "confirmed".into(),
                reason: "Owner confirmed thread decision".into(),
                result_ref: format!("线程结论 {} 已确认", decision_id.as_str()),
            })
        }
        SecretaryAction::RevokeThreadDecision {
            decision_id,
            reason,
        } => {
            let row = lock_decision(db, decision_id.as_str(), account_id).await?;
            if !matches!(row.status.as_str(), "proposed" | "confirmed" | "revoked") {
                return Err(ThreadControlStoreError::InvalidData(
                    "superseded decisions cannot be revoked".into(),
                ));
            }
            if row.status != "revoked" {
                update_status(
                    db,
                    "secretary_thread_decisions",
                    "decision_id",
                    decision_id.as_str(),
                    &row.status,
                    "revoked",
                )
                .await?;
            }
            Ok(AppliedControl {
                thread_id: row.thread_id,
                target_kind: "decision",
                target_id: decision_id.as_str().to_owned(),
                control_kind: "revoke_decision",
                previous_status: row.status,
                current_status: "revoked".into(),
                reason: reason.clone(),
                result_ref: format!("线程结论 {} 已撤销", decision_id.as_str()),
            })
        }
        SecretaryAction::DismissThreadQuestion {
            question_id,
            reason,
        } => {
            let row = lock_question(db, question_id.as_str(), account_id).await?;
            if !matches!(row.status.as_str(), "open" | "dismissed") {
                return Err(ThreadControlStoreError::InvalidData(
                    "answered questions cannot be dismissed".into(),
                ));
            }
            if row.status == "open" {
                update_status(
                    db,
                    "secretary_thread_open_questions",
                    "question_id",
                    question_id.as_str(),
                    "open",
                    "dismissed",
                )
                .await?;
            }
            Ok(AppliedControl {
                thread_id: row.thread_id,
                target_kind: "question",
                target_id: question_id.as_str().to_owned(),
                control_kind: "dismiss_question",
                previous_status: row.status,
                current_status: "dismissed".into(),
                reason: reason.clone(),
                result_ref: format!("未决问题 {} 已忽略", question_id.as_str()),
            })
        }
        SecretaryAction::ReconfirmThreadSemantics { thread_id, reason } => {
            let row = lock_thread(db, thread_id.as_str(), account_id).await?;
            let pending = CountRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"SELECT COUNT(*) AS value
                   FROM secretary_thread_semantic_invalidations invalidation
                   WHERE invalidation.thread_id = ?
                     AND NOT EXISTS (
                       SELECT 1
                       FROM secretary_thread_semantic_reconfirmations reconfirmation
                       WHERE reconfirmation.thread_id = invalidation.thread_id
                         AND reconfirmation.created_at >= invalidation.created_at
                     )"#,
                [thread_id.as_str().into()],
            ))
            .one(db)
            .await
            .map_err(database_error)?
            .map(|row| u64::try_from(row.value).unwrap_or_default())
            .unwrap_or_default();
            if pending == 0 {
                return Err(ThreadControlStoreError::InvalidData(
                    "thread has no pending semantic invalidation to reconfirm".into(),
                ));
            }
            let reconfirmation_id = stable_id("thread-semantic-reconfirmation", &request.effect_id);
            let inserted = db
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"INSERT INTO secretary_thread_semantic_reconfirmations
                       (reconfirmation_id, thread_id, command_source_event_id, effect_id, reason)
                       VALUES (?, ?, ?, ?, ?)"#,
                    [
                        reconfirmation_id.into(),
                        thread_id.as_str().into(),
                        request.command_source_event_id.as_str().into(),
                        request.effect_id.clone().into(),
                        reason.clone().into(),
                    ],
                ))
                .await
                .map_err(database_error)?;
            if inserted.rows_affected() != 1 {
                return Err(ThreadControlStoreError::Database);
            }
            db.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "DELETE FROM secretary_thread_semantic_state WHERE thread_id = ?",
                [thread_id.as_str().into()],
            ))
            .await
            .map_err(database_error)?;
            Ok(AppliedControl {
                thread_id: thread_id.as_str().to_owned(),
                target_kind: "thread",
                target_id: thread_id.as_str().to_owned(),
                control_kind: "reconfirm_thread_semantics",
                previous_status: row.status.clone(),
                current_status: row.status,
                reason: reason.clone(),
                result_ref: format!("线程 {} 的语义已重新确认", thread_id.as_str()),
            })
        }
        SecretaryAction::SetThreadLifecycle {
            thread_id,
            expected_status,
            target_status,
            reason,
        } => {
            let row = lock_thread(db, thread_id.as_str(), account_id).await?;
            if row.status != expected_status.as_str() {
                return Err(ThreadControlStoreError::InvalidData(
                    "thread status changed since approval".into(),
                ));
            }
            let control_kind = match target_status {
                crate::ThreadStatus::Closed
                    if matches!(
                        expected_status,
                        crate::ThreadStatus::Open
                            | crate::ThreadStatus::Waiting
                            | crate::ThreadStatus::Reopened
                    ) =>
                {
                    let open_question =
                        QuestionIdRow::find_by_statement(Statement::from_sql_and_values(
                            DatabaseBackend::MySql,
                            "SELECT question_id FROM secretary_thread_open_questions \
                         WHERE thread_id = ? AND status = 'open' LIMIT 1 FOR UPDATE",
                            [thread_id.as_str().into()],
                        ))
                        .one(db)
                        .await
                        .map_err(database_error)?;
                    if open_question.is_some() {
                        return Err(ThreadControlStoreError::InvalidData(
                            "thread has open questions; dismiss or answer them before closing"
                                .into(),
                        ));
                    }
                    "close_thread"
                }
                crate::ThreadStatus::Reopened
                    if matches!(
                        expected_status,
                        crate::ThreadStatus::Closed | crate::ThreadStatus::Resolved
                    ) =>
                {
                    "reopen_thread"
                }
                _ => {
                    return Err(ThreadControlStoreError::InvalidData(
                        "unsupported owner thread lifecycle transition".into(),
                    ));
                }
            };
            let updated = db
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "UPDATE secretary_event_threads SET status = ? \
                     WHERE thread_id = ? AND account_id = ? AND status = ?",
                    [
                        target_status.as_str().into(),
                        thread_id.as_str().into(),
                        account_id.into(),
                        expected_status.as_str().into(),
                    ],
                ))
                .await
                .map_err(database_error)?;
            if updated.rows_affected() != 1 {
                return Err(ThreadControlStoreError::InvalidData(
                    "thread lifecycle compare-and-set failed".into(),
                ));
            }
            let change_id = stable_id("thread-status", &request.effect_id);
            db.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT INTO secretary_thread_status_history \
                 (change_id, thread_id, from_status, to_status, authority, reason) \
                 VALUES (?, ?, ?, ?, 'owner_confirmed', ?)",
                [
                    change_id.clone().into(),
                    thread_id.as_str().into(),
                    expected_status.as_str().into(),
                    target_status.as_str().into(),
                    reason.clone().into(),
                ],
            ))
            .await
            .map_err(database_error)?;
            db.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT INTO secretary_thread_status_sources (change_id, source_event_id) \
                 VALUES (?, ?)",
                [
                    change_id.into(),
                    request.command_source_event_id.as_str().into(),
                ],
            ))
            .await
            .map_err(database_error)?;
            Ok(AppliedControl {
                thread_id: thread_id.as_str().to_owned(),
                target_kind: "thread",
                target_id: thread_id.as_str().to_owned(),
                control_kind,
                previous_status: expected_status.as_str().into(),
                current_status: target_status.as_str().into(),
                reason: reason.clone(),
                result_ref: format!(
                    "线程 {} 状态已从 {} 改为 {}",
                    thread_id.as_str(),
                    expected_status.as_str(),
                    target_status.as_str()
                ),
            })
        }
        _ => Err(ThreadControlStoreError::InvalidData(
            "action is not a thread control".into(),
        )),
    }
}

async fn insert_control_audit<C: ConnectionTrait>(
    db: &C,
    request: &ThreadControlEffectRequest,
    account_id: u64,
    applied: &AppliedControl,
) -> Result<(), ThreadControlStoreError> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_thread_owner_controls \
             (control_id, effect_id, run_id, proposal_id, account_id, thread_id, target_kind, \
              target_id, control_kind, previous_status, current_status, command_source_event_id, reason) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            [
                stable_id("thread-control", &request.effect_id).into(),
                request.effect_id.clone().into(),
                request.run_id.as_str().into(),
                request.proposal_id.clone().into(),
                account_id.into(),
                applied.thread_id.clone().into(),
                applied.target_kind.into(),
                applied.target_id.clone().into(),
                applied.control_kind.into(),
                applied.previous_status.clone().into(),
                applied.current_status.clone().into(),
                request.command_source_event_id.as_str().into(),
                applied.reason.clone().into(),
            ],
        ))
        .await
        .map_err(database_error)?;
    if result.rows_affected() != 1 {
        return Err(ThreadControlStoreError::Database);
    }
    Ok(())
}

async fn lock_account<C: ConnectionTrait>(
    db: &C,
    request: &ThreadControlEffectRequest,
) -> Result<u64, ThreadControlStoreError> {
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
    .ok_or(ThreadControlStoreError::Unauthorized)
}

async fn verify_action_lease<C: ConnectionTrait>(
    db: &C,
    request: &ThreadControlEffectRequest,
    account_id: u64,
) -> Result<(), ThreadControlStoreError> {
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
        return Err(ThreadControlStoreError::LeaseLost);
    }
    Ok(())
}

/// 复验命令事件是 OwnerCommand（含权威 actor_kind）且 active OwnerBinding
/// 匹配（CMD-010 防线 A 四元组）。委托共享授权 helper，禁止复制授权 SQL。
async fn verify_owner_command<C: ConnectionTrait>(
    db: &C,
    request: &ThreadControlEffectRequest,
    managed_account_id: u64,
) -> Result<(), ThreadControlStoreError> {
    super::owner_authorization::verify_owner_command(
        db,
        &request.command_source_event_id,
        managed_account_id,
    )
    .await
    .map_err(|error| match error {
        super::owner_authorization::OwnerAuthError::Unauthorized => {
            ThreadControlStoreError::Unauthorized
        }
        super::owner_authorization::OwnerAuthError::Database => ThreadControlStoreError::Database,
    })
}

async fn load_receipt<C: ConnectionTrait>(
    db: &C,
    request: &ThreadControlEffectRequest,
) -> Result<Option<SecretaryActionReceipt>, ThreadControlStoreError> {
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
            .map_err(|_| ThreadControlStoreError::Database)?;
        if row.run_id != request.run_id.as_str()
            || proposal.proposal_id != request.proposal_id
            || proposal.action != request.action
        {
            return Err(ThreadControlStoreError::InvalidData(
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

async fn lock_decision<C: ConnectionTrait>(
    db: &C,
    decision_id: &str,
    account_id: u64,
) -> Result<TargetRow, ThreadControlStoreError> {
    TargetRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT d.thread_id, d.status FROM secretary_thread_decisions d \
         INNER JOIN secretary_event_threads t ON t.thread_id = d.thread_id \
         WHERE d.decision_id = ? AND t.account_id = ? FOR UPDATE",
        [decision_id.into(), account_id.into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or_else(|| ThreadControlStoreError::InvalidData("decision not found in account".into()))
}

async fn lock_question<C: ConnectionTrait>(
    db: &C,
    question_id: &str,
    account_id: u64,
) -> Result<TargetRow, ThreadControlStoreError> {
    TargetRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT q.thread_id, q.status FROM secretary_thread_open_questions q \
         INNER JOIN secretary_event_threads t ON t.thread_id = q.thread_id \
         WHERE q.question_id = ? AND t.account_id = ? FOR UPDATE",
        [question_id.into(), account_id.into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or_else(|| ThreadControlStoreError::InvalidData("question not found in account".into()))
}

async fn lock_thread<C: ConnectionTrait>(
    db: &C,
    thread_id: &str,
    account_id: u64,
) -> Result<TargetRow, ThreadControlStoreError> {
    TargetRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT thread_id, status FROM secretary_event_threads \
         WHERE thread_id = ? AND account_id = ? FOR UPDATE",
        [thread_id.into(), account_id.into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or_else(|| ThreadControlStoreError::InvalidData("thread not found in account".into()))
}

async fn update_status<C: ConnectionTrait>(
    db: &C,
    table: &str,
    id_column: &str,
    id: &str,
    previous: &str,
    current: &str,
) -> Result<(), ThreadControlStoreError> {
    let sql = match (table, id_column) {
        ("secretary_thread_decisions", "decision_id") => {
            "UPDATE secretary_thread_decisions SET status = ? WHERE decision_id = ? AND status = ?"
        }
        ("secretary_thread_open_questions", "question_id") => {
            "UPDATE secretary_thread_open_questions SET status = ? WHERE question_id = ? AND status = ?"
        }
        _ => return Err(ThreadControlStoreError::Database),
    };
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            [current.into(), id.into(), previous.into()],
        ))
        .await
        .map_err(database_error)?;
    if result.rows_affected() != 1 {
        return Err(ThreadControlStoreError::InvalidData(
            "thread semantic compare-and-set failed".into(),
        ));
    }
    Ok(())
}

fn stable_id(namespace: &str, effect_id: &str) -> String {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        format!("{namespace}:{effect_id}").as_bytes(),
    )
    .to_string()
}

fn database_error(_: sea_orm::DbErr) -> ThreadControlStoreError {
    ThreadControlStoreError::Database
}

#[derive(FromQueryResult)]
struct IdRow {
    id: u64,
}

#[derive(FromQueryResult)]
struct ReceiptRow {
    run_id: String,
    proposal_json: String,
    result_ref: String,
}

#[derive(FromQueryResult)]
struct TargetRow {
    thread_id: String,
    status: String,
}

#[derive(FromQueryResult)]
struct QuestionIdRow {
    #[allow(dead_code)]
    question_id: String,
}

#[derive(FromQueryResult)]
struct CountRow {
    value: i64,
}
