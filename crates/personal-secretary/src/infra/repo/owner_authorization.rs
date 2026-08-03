//! 写类 Effect 共用的 OwnerCommand 授权复验（CMD-010 防线 A）。
//!
//! 所有策略写事务必须在最终事务内重新读取：
//! 1. 原始 SourceEvent（`FOR UPDATE` 锁定）：`message_role = 'owner_command'` 且
//!    `actor_kind = 'owner'` —— 身份种类必须来自权威 SourceEvent，不是冗余字段；
//! 2. 当前 active OwnerBinding：托管账号下恰好一条，且 command account 与
//!    owner actor 同时匹配命令事件 —— 完整身份 = managed account（调用方锁定）
//!    + command account + owner actor + identity kind 四元组；
//! 3. 任一偏差按未授权拒绝，绝不信任 Planner、Checkpoint 或 Handler 缓存的
//!    Owner 身份。
//!
//! 本 helper 只做授权判定，不决定业务状态；各 Store 将自己的公开错误类型
//! 通过 `From<OwnerAuthError>` 映射（复用 authorization.rs 的 ControlAuthError
//! 模式，禁止复制稍有不同的授权 SQL）。

use sea_orm::{ConnectionTrait, DatabaseBackend, FromQueryResult, Statement};

/// 共享授权错误。数据库错误与授权失败精确分类：
/// 数据库错误可能已提交（UnknownCommit 的来源），授权失败是确定性拒绝。
#[derive(Debug, thiserror::Error)]
pub(crate) enum OwnerAuthError {
    #[error("OwnerCommand authorization failed")]
    Unauthorized,
    #[error("OwnerCommand authorization database operation failed")]
    Database,
}

/// 复验 OwnerCommand：命令事件必须是权威 SourceEvent 中的
/// `owner_command` 角色且 `actor_kind = 'owner'`，同时托管账号下恰好一条
/// active OwnerBinding 匹配命令账号与 Owner actor。任何偏差按未授权拒绝。
///
/// 命令事件行被 `FOR UPDATE` 锁定；binding 查询同样加锁，因此授权判定
/// 与事务提交之间 binding 被撤销/替换会阻塞到事务结束（A.5：审批后、
/// Effect 提交前撤销必须拒绝）。
pub(crate) async fn verify_owner_command<C: ConnectionTrait>(
    db: &C,
    command_source_event_id: &crate::SourceEventId,
    managed_account_id: u64,
) -> Result<(), OwnerAuthError> {
    let command = CommandRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT command.account_id, command.actor_platform_id, command.actor_kind, \
                command.message_role, command.event_type, account.source_channel, \
                conversation.conversation_kind \
         FROM secretary_source_events command \
         INNER JOIN secretary_accounts account ON account.id = command.account_id \
         INNER JOIN secretary_conversations conversation ON conversation.id = command.conversation_id \
         WHERE command.source_event_id = ? FOR UPDATE",
        [command_source_event_id.as_str().into()],
    ))
    .one(db)
    .await
    .map_err(|_| OwnerAuthError::Database)?
    .ok_or(OwnerAuthError::Unauthorized)?;
    if command.message_role != "owner_command"
        || command.actor_kind != "owner"
        || command.event_type != "message"
        || command.source_channel != "qq_open_platform"
        || command.conversation_kind != "owner_control"
    {
        return Err(OwnerAuthError::Unauthorized);
    }
    let bindings = BindingRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT command_account_id, owner_actor_id FROM secretary_owner_bindings \
         WHERE managed_account_id = ? AND status = 'active' LIMIT 2 FOR UPDATE",
        [managed_account_id.into()],
    ))
    .all(db)
    .await
    .map_err(|_| OwnerAuthError::Database)?;
    match bindings.as_slice() {
        [binding]
            if binding.command_account_id == command.account_id
                && binding.owner_actor_id == command.actor_platform_id =>
        {
            Ok(())
        }
        _ => Err(OwnerAuthError::Unauthorized),
    }
}

#[derive(FromQueryResult)]
struct CommandRow {
    account_id: u64,
    actor_platform_id: String,
    actor_kind: String,
    message_role: String,
    event_type: String,
    source_channel: String,
    conversation_kind: String,
}

#[derive(FromQueryResult)]
struct BindingRow {
    command_account_id: u64,
    owner_actor_id: String,
}
