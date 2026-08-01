//! Owner 工作控制（FollowUp 与 ResponseExpectation）共用的授权、Receipt 与稳定 ID 逻辑。
//!
//! 两类控制仓储必须复用同一套安全边界：账号锁定、Action 租约复验、OwnerCommand
//! 与唯一 active OwnerBinding 校验、Effect Receipt 快速路径与重放校验，以及
//! 无歧义的稳定 control_id 派生。禁止在 ResponseExpectation 仓储复制
//! 稍有不同的授权 SQL。

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseTransaction, FromQueryResult, Statement};

use crate::{
    ActionLeaseToken, ActionRunId, FollowUpControlEffectRequest,
    ResponseExpectationControlEffectRequest, SecretaryAction, SecretaryActionProposal,
    SecretaryActionReceipt, SourceAccountRef, SourceEventId,
};

/// 两类 Owner 控制 Effect 的共享只读视图；由各自请求类型构造，避免共享函数重复参数。
pub(crate) struct ControlEffectCtx<'a> {
    pub account: &'a SourceAccountRef,
    pub command_source_event_id: &'a SourceEventId,
    pub run_id: &'a ActionRunId,
    pub lease_token: &'a ActionLeaseToken,
    pub effect_id: &'a str,
    pub proposal_id: &'a str,
    pub proposal_json: &'a str,
    pub action: &'a SecretaryAction,
}

impl<'a> From<&'a FollowUpControlEffectRequest> for ControlEffectCtx<'a> {
    fn from(request: &'a FollowUpControlEffectRequest) -> Self {
        Self {
            account: &request.account,
            command_source_event_id: &request.command_source_event_id,
            run_id: &request.run_id,
            lease_token: &request.lease_token,
            effect_id: &request.effect_id,
            proposal_id: &request.proposal_id,
            proposal_json: &request.proposal_json,
            action: &request.action,
        }
    }
}

impl<'a> From<&'a ResponseExpectationControlEffectRequest> for ControlEffectCtx<'a> {
    fn from(request: &'a ResponseExpectationControlEffectRequest) -> Self {
        Self {
            account: &request.account,
            command_source_event_id: &request.command_source_event_id,
            run_id: &request.run_id,
            lease_token: &request.lease_token,
            effect_id: &request.effect_id,
            proposal_id: &request.proposal_id,
            proposal_json: &request.proposal_json,
            action: &request.action,
        }
    }
}

/// 共享授权/回执层的统一错误；由各控制仓储映射到自己的公开错误类型。
/// 错误分类语义与单条/批量控制一致：数据库错误是 UnknownCommit 的来源，
/// 授权/租约/版本/状态冲突是确定性失败，不得伪装成提交不明。
#[derive(Debug, thiserror::Error)]
pub(crate) enum ControlAuthError {
    #[error("owner work control is unauthorized")]
    Unauthorized,
    #[error("owner work control target or state is invalid: {0}")]
    InvalidData(String),
    #[error("owner work control lease was lost")]
    LeaseLost,
    #[error("owner work control database operation failed")]
    Database,
}

impl From<ControlAuthError> for crate::FollowUpControlStoreError {
    fn from(error: ControlAuthError) -> Self {
        match error {
            ControlAuthError::Unauthorized => crate::FollowUpControlStoreError::Unauthorized,
            ControlAuthError::InvalidData(message) => {
                crate::FollowUpControlStoreError::InvalidData(message)
            }
            ControlAuthError::LeaseLost => crate::FollowUpControlStoreError::LeaseLost,
            ControlAuthError::Database => crate::FollowUpControlStoreError::Database,
        }
    }
}

impl From<ControlAuthError> for crate::ResponseExpectationControlStoreError {
    fn from(error: ControlAuthError) -> Self {
        match error {
            ControlAuthError::Unauthorized => {
                crate::ResponseExpectationControlStoreError::Unauthorized
            }
            ControlAuthError::InvalidData(message) => {
                crate::ResponseExpectationControlStoreError::InvalidData(message)
            }
            ControlAuthError::LeaseLost => crate::ResponseExpectationControlStoreError::LeaseLost,
            ControlAuthError::Database => crate::ResponseExpectationControlStoreError::Database,
        }
    }
}

/// 锁定托管账号：渠道 + 平台账号精确匹配且状态为 active。
pub(crate) async fn lock_account<C: ConnectionTrait>(
    db: &C,
    ctx: &ControlEffectCtx<'_>,
) -> Result<u64, ControlAuthError> {
    IdRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT id FROM secretary_accounts WHERE source_channel = ? \
         AND platform_account_id = ? AND status = 'active' FOR UPDATE",
        [
            ctx.account.channel.as_str().into(),
            ctx.account.account_id.clone().into(),
        ],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .map(|row| row.id)
    .ok_or(ControlAuthError::Unauthorized)
}

/// 复验 Action Run：running、租约一致、未过期、账号与命令 SourceEvent 一致。
pub(crate) async fn verify_action_lease<C: ConnectionTrait>(
    db: &C,
    ctx: &ControlEffectCtx<'_>,
    account_id: u64,
) -> Result<(), ControlAuthError> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_action_runs SET updated_at = UTC_TIMESTAMP(6) \
             WHERE run_id = ? AND lease_token = ? AND status = 'running' AND account_id = ? \
               AND command_source_event_id = ? AND lease_expires_at >= UTC_TIMESTAMP(6)",
            [
                ctx.run_id.as_str().into(),
                ctx.lease_token.as_str().into(),
                account_id.into(),
                ctx.command_source_event_id.as_str().into(),
            ],
        ))
        .await
        .map_err(database_error)?;
    if result.rows_affected() != 1 {
        return Err(ControlAuthError::LeaseLost);
    }
    Ok(())
}

/// 复验命令 SourceEvent 是 OwnerCommand，且账号下恰好一个 active OwnerBinding
/// 同时匹配托管账号、命令账号与 Owner actor；任何偏差都按未授权拒绝。
pub(crate) async fn verify_owner_command<C: ConnectionTrait>(
    db: &C,
    ctx: &ControlEffectCtx<'_>,
    managed_account_id: u64,
) -> Result<(), ControlAuthError> {
    let command = CommandRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT account_id, actor_platform_id, message_role FROM secretary_source_events \
         WHERE source_event_id = ? FOR UPDATE",
        [ctx.command_source_event_id.as_str().into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or(ControlAuthError::Unauthorized)?;
    if command.message_role != "owner_command" {
        return Err(ControlAuthError::Unauthorized);
    }
    let bindings = BindingRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT command_account_id, owner_actor_id FROM secretary_owner_bindings \
         WHERE managed_account_id = ? AND status = 'active' LIMIT 2 FOR UPDATE",
        [managed_account_id.into()],
    ))
    .all(db)
    .await
    .map_err(database_error)?;
    match bindings.as_slice() {
        [binding]
            if binding.command_account_id == command.account_id
                && binding.owner_actor_id == command.actor_platform_id =>
        {
            Ok(())
        }
        _ => Err(ControlAuthError::Unauthorized),
    }
}

/// 加载既有 Effect Receipt；必须校验 run_id + proposal_id + 完整 Action 完全一致，
/// 不能仅按 effect_id 命中就返回成功（重放语义）。
pub(crate) async fn load_receipt<C: ConnectionTrait>(
    db: &C,
    ctx: &ControlEffectCtx<'_>,
) -> Result<Option<SecretaryActionReceipt>, ControlAuthError> {
    let row = ReceiptRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT run_id, CAST(proposal_json AS CHAR) AS proposal_json, result_ref \
         FROM secretary_action_effect_receipts WHERE effect_id = ?",
        [ctx.effect_id.into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?;
    row.map(|row| {
        let proposal: SecretaryActionProposal =
            serde_json::from_str(&row.proposal_json).map_err(|_| ControlAuthError::Database)?;
        if row.run_id != ctx.run_id.as_str()
            || proposal.proposal_id != ctx.proposal_id
            || proposal.action != *ctx.action
        {
            return Err(ControlAuthError::InvalidData(
                "effect receipt belongs to a different action".into(),
            ));
        }
        Ok(SecretaryActionReceipt {
            proposal_id: proposal.proposal_id,
            result_ref: row.result_ref,
            tool_kind: Some(ctx.action.kind()),
        })
    })
    .transpose()
}

/// 整批只写一条 Effect Receipt；并发抢先写入时加载并校验归属，而不是盲目宣告成功。
pub(crate) async fn insert_receipt_and_commit(
    db: DatabaseTransaction,
    ctx: &ControlEffectCtx<'_>,
    result_ref: String,
) -> Result<SecretaryActionReceipt, ControlAuthError> {
    let inserted = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT IGNORE INTO secretary_action_effect_receipts \
             (effect_id, run_id, proposal_json, result_ref) VALUES (?, ?, ?, ?)",
            [
                ctx.effect_id.into(),
                ctx.run_id.as_str().into(),
                ctx.proposal_json.into(),
                result_ref.clone().into(),
            ],
        ))
        .await
        .map_err(database_error)?;
    if inserted.rows_affected() != 1 {
        let receipt = load_receipt(&db, ctx)
            .await?
            .ok_or(ControlAuthError::Database)?;
        db.commit().await.map_err(database_error)?;
        return Ok(receipt);
    }
    db.commit().await.map_err(database_error)?;
    Ok(SecretaryActionReceipt {
        proposal_id: ctx.proposal_id.to_owned(),
        result_ref,
        tool_kind: Some(ctx.action.kind()),
    })
}

/// 无歧义的稳定 control_id 派生（UUIDv5）；同一 Effect 内每目标唯一，
/// 重放不产生新 ID。
pub(crate) fn stable_id(namespace: &str, effect_id: &str) -> String {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        format!("{namespace}:{effect_id}").as_bytes(),
    )
    .to_string()
}

pub(crate) fn database_error(_: sea_orm::DbErr) -> ControlAuthError {
    ControlAuthError::Database
}

#[derive(FromQueryResult)]
struct IdRow {
    id: u64,
}

#[derive(FromQueryResult)]
struct CommandRow {
    account_id: u64,
    actor_platform_id: String,
    message_role: String,
}

#[derive(FromQueryResult)]
struct BindingRow {
    command_account_id: u64,
    owner_actor_id: String,
}

#[derive(FromQueryResult)]
struct ReceiptRow {
    run_id: String,
    proposal_json: String,
    result_ref: String,
}
