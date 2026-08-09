use async_trait::async_trait;
use personal_secretary::{
    ClaimedOwnerResponse, InboundEventStoreError, NotificationFailureKind,
    OwnerResponseDeliveryScope, OwnerResponseDeliveryStoreT, OwnerResponseDraft, OwnerResponseId,
    OwnerResponseLeaseToken, OwnerResponseTarget,
};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};

use super::mysql_inbound::store_error;

pub(crate) struct MySqlOwnerResponseDeliveryStore {
    db: DatabaseConnection,
}

impl MySqlOwnerResponseDeliveryStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl OwnerResponseDeliveryStoreT for MySqlOwnerResponseDeliveryStore {
    async fn claim_pending_response(
        &self,
        scope: &OwnerResponseDeliveryScope,
        now_unix_secs: i64,
        lease_secs: u64,
        max_reply_age_secs: u64,
    ) -> Result<Option<ClaimedOwnerResponse>, InboundEventStoreError> {
        if now_unix_secs < 0
            || !(1..=3600).contains(&lease_secs)
            || !(30..=300).contains(&max_reply_age_secs)
        {
            return Err(InboundEventStoreError::InvalidData(
                "owner response claim bounds are invalid".into(),
            ));
        }
        let cutoff = now_unix_secs.saturating_sub(max_reply_age_secs as i64);
        let transaction = self.db.begin().await.map_err(store_error)?;

        // 将已经完成的 Owner Action 响应幂等物化为独立 Outbox。授权四元组在这里及
        // 后续领取查询中都会重新验证，普通 C2C、群消息和 NapCat 观察无法进入候选集。
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT IGNORE INTO secretary_owner_response_outbox (response_id)
                   SELECT response.response_id
                   FROM secretary_action_responses response
                   LEFT JOIN secretary_owner_response_outbox existing_outbox
                     ON existing_outbox.response_id = response.response_id
                   JOIN secretary_action_runs run ON run.run_id = response.run_id
                   JOIN secretary_accounts managed_account ON managed_account.id = run.account_id
                   JOIN secretary_source_events command
                     ON command.source_event_id = run.command_source_event_id
                   JOIN secretary_accounts command_account ON command_account.id = command.account_id
                   JOIN secretary_qq_raw_events raw_event
                     ON raw_event.source_event_id = command.source_event_id
                    AND raw_event.event_kind IN ('c2c_message', 'group_at_message')
                   JOIN secretary_owner_bindings binding
                     ON binding.managed_account_id = run.account_id
                    AND binding.command_account_id = command.account_id
                    AND binding.owner_actor_id = command.actor_platform_id
                    AND binding.status = 'active'
                   WHERE response.invalidated = FALSE
                     AND existing_outbox.response_id IS NULL
                     AND run.status = 'completed'
                     AND managed_account.source_channel = ?
                     AND managed_account.platform_account_id = ?
                     AND command_account.source_channel = ?
                     AND command_account.platform_account_id = ?
                     AND command.source_channel = 'qq_open_platform'
                     AND command.event_type = 'message'
                     AND command.actor_kind = 'owner'
                     AND command.message_role = 'owner_command'
                     AND command.actor_platform_id = ?
                   ORDER BY response.created_at, response.response_id
                   LIMIT 100"#,
                [
                    scope.managed_account.channel.as_str().into(),
                    scope.managed_account.account_id.clone().into(),
                    scope.command_account.channel.as_str().into(),
                    scope.command_account.account_id.clone().into(),
                    scope.owner_actor_id.clone().into(),
                ],
            ))
            .await
            .map_err(store_error)?;

        // 已离开平台被动回复窗口的草稿保留审计，但绝不退化为主动消息发送。
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_owner_response_outbox outbox
                   JOIN secretary_action_responses response ON response.response_id = outbox.response_id
                   JOIN secretary_action_runs run ON run.run_id = response.run_id
                   JOIN secretary_accounts managed_account ON managed_account.id = run.account_id
                   JOIN secretary_source_events command
                     ON command.source_event_id = run.command_source_event_id
                   SET outbox.delivery_status = 'failed',
                       outbox.last_error_code = 'reply_context_expired',
                       outbox.lease_token = NULL, outbox.lease_expires_at = NULL,
                       outbox.next_eligible_at = NULL
                   WHERE outbox.delivery_status = 'pending'
                     AND command.occurred_at_unix_secs < ?
                     AND managed_account.source_channel = ?
                     AND managed_account.platform_account_id = ?"#,
                [
                    cutoff.into(),
                    scope.managed_account.channel.as_str().into(),
                    scope.managed_account.account_id.clone().into(),
                ],
            ))
            .await
            .map_err(store_error)?;

        // claimed 后进程退出时无法判断 HTTP 是否已经提交，租约过期必须终态化，禁止盲重试。
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_owner_response_outbox outbox
                   JOIN secretary_action_responses response ON response.response_id = outbox.response_id
                   JOIN secretary_action_runs run ON run.run_id = response.run_id
                   JOIN secretary_accounts managed_account ON managed_account.id = run.account_id
                   SET outbox.delivery_status = 'unknown_commit',
                       outbox.last_error_code = 'lease_expired_in_flight',
                       outbox.lease_token = NULL, outbox.lease_expires_at = NULL,
                       outbox.next_eligible_at = NULL
                   WHERE outbox.delivery_status = 'claimed'
                     AND outbox.lease_expires_at < UTC_TIMESTAMP(6)
                     AND managed_account.source_channel = ?
                     AND managed_account.platform_account_id = ?"#,
                [
                    scope.managed_account.channel.as_str().into(),
                    scope.managed_account.account_id.clone().into(),
                ],
            ))
            .await
            .map_err(store_error)?;

        let Some(row) = OwnerResponseClaimRow::find_by_statement(
            Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"SELECT outbox.response_id, CAST(response.response_json AS CHAR) AS response_json,
                          command.platform_event_id AS reply_to_platform_message_id,
                          raw_event.event_kind,
                          JSON_UNQUOTE(JSON_EXTRACT(raw_event.envelope_json, '$.d.group_openid'))
                            AS group_openid
                   FROM secretary_owner_response_outbox outbox
                   JOIN secretary_action_responses response ON response.response_id = outbox.response_id
                   JOIN secretary_action_runs run ON run.run_id = response.run_id
                   JOIN secretary_accounts managed_account ON managed_account.id = run.account_id
                   JOIN secretary_source_events command
                     ON command.source_event_id = run.command_source_event_id
                   JOIN secretary_accounts command_account ON command_account.id = command.account_id
                   JOIN secretary_qq_raw_events raw_event
                     ON raw_event.source_event_id = command.source_event_id
                    AND raw_event.event_kind IN ('c2c_message', 'group_at_message')
                   JOIN secretary_owner_bindings binding
                     ON binding.managed_account_id = run.account_id
                    AND binding.command_account_id = command.account_id
                    AND binding.owner_actor_id = command.actor_platform_id
                    AND binding.status = 'active'
                   WHERE outbox.delivery_status = 'pending'
                     AND (outbox.next_eligible_at IS NULL OR outbox.next_eligible_at <= UTC_TIMESTAMP(6))
                     AND response.invalidated = FALSE
                     AND run.status = 'completed'
                     AND command.occurred_at_unix_secs >= ?
                     AND managed_account.source_channel = ?
                     AND managed_account.platform_account_id = ?
                     AND command_account.source_channel = ?
                     AND command_account.platform_account_id = ?
                     AND command.source_channel = 'qq_open_platform'
                     AND command.event_type = 'message'
                     AND command.actor_kind = 'owner'
                     AND command.message_role = 'owner_command'
                     AND command.actor_platform_id = ?
                   ORDER BY response.created_at, response.response_id
                   LIMIT 1 FOR UPDATE SKIP LOCKED"#,
                [
                    cutoff.into(),
                    scope.managed_account.channel.as_str().into(),
                    scope.managed_account.account_id.clone().into(),
                    scope.command_account.channel.as_str().into(),
                    scope.command_account.account_id.clone().into(),
                    scope.owner_actor_id.clone().into(),
                ],
            ),
        )
        .one(&transaction)
        .await
        .map_err(store_error)?
        else {
            transaction.commit().await.map_err(store_error)?;
            return Ok(None);
        };

        let draft: OwnerResponseDraft = serde_json::from_str(&row.response_json)
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
        if draft.invalidated() {
            transaction.rollback().await.map_err(store_error)?;
            return Err(InboundEventStoreError::InvalidData(
                "claimed owner response draft is invalidated".into(),
            ));
        }
        let response_id = OwnerResponseId::new(row.response_id)?;
        let target = match row.event_kind.as_str() {
            "c2c_message" => OwnerResponseTarget::C2c,
            "group_at_message" => {
                OwnerResponseTarget::group(row.group_openid.ok_or_else(|| {
                    InboundEventStoreError::InvalidData(
                        "authoritative group reply event has no group target".into(),
                    )
                })?)?
            }
            _ => {
                transaction.rollback().await.map_err(store_error)?;
                return Err(InboundEventStoreError::InvalidData(
                    "owner response has an unsupported gateway event kind".into(),
                ));
            }
        };
        let lease_token = OwnerResponseLeaseToken::generate();
        let updated = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_owner_response_outbox
                   SET delivery_status = 'claimed', attempts = attempts + 1,
                       lease_token = ?,
                       lease_expires_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND),
                       next_eligible_at = NULL, last_error_code = NULL
                   WHERE response_id = ? AND delivery_status = 'pending'"#,
                [
                    lease_token.as_str().into(),
                    lease_secs.into(),
                    response_id.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if updated.rows_affected() != 1 {
            transaction.rollback().await.map_err(store_error)?;
            return Err(InboundEventStoreError::LeaseLost);
        }
        transaction.commit().await.map_err(store_error)?;
        Ok(Some(ClaimedOwnerResponse {
            response_id,
            lease_token,
            draft,
            reply_to_platform_message_id: row.reply_to_platform_message_id,
            target,
        }))
    }

    async fn mark_response_delivered(
        &self,
        response_id: &OwnerResponseId,
        lease_token: &OwnerResponseLeaseToken,
        platform_message_id: &str,
    ) -> Result<(), InboundEventStoreError> {
        let updated = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_owner_response_outbox
                   SET delivery_status = 'delivered', platform_message_id = ?,
                       delivered_at = UTC_TIMESTAMP(6), lease_token = NULL,
                       lease_expires_at = NULL, next_eligible_at = NULL,
                       last_error_code = NULL
                   WHERE response_id = ? AND delivery_status = 'claimed'
                     AND lease_token = ? AND lease_expires_at >= UTC_TIMESTAMP(6)"#,
                [
                    platform_message_id.into(),
                    response_id.as_str().into(),
                    lease_token.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        require_fenced_update(updated.rows_affected())
    }

    async fn mark_response_failed(
        &self,
        response_id: &OwnerResponseId,
        lease_token: &OwnerResponseLeaseToken,
        error_code: &str,
        kind: NotificationFailureKind,
    ) -> Result<(), InboundEventStoreError> {
        let (status, next_eligible_sql) = match kind {
            NotificationFailureKind::Retryable => (
                "pending",
                "TIMESTAMPADD(SECOND, CAST(LEAST(300, POW(2, LEAST(GREATEST(attempts - 1, 0), 8))) AS UNSIGNED), UTC_TIMESTAMP(6))",
            ),
            NotificationFailureKind::Permanent => ("failed", "NULL"),
            NotificationFailureKind::UnknownCommit => ("unknown_commit", "NULL"),
        };
        let sql = format!(
            "UPDATE secretary_owner_response_outbox \
             SET delivery_status = ?, last_error_code = ?, lease_token = NULL, \
                 lease_expires_at = NULL, next_eligible_at = {next_eligible_sql} \
             WHERE response_id = ? AND delivery_status = 'claimed' \
               AND lease_token = ? AND lease_expires_at >= UTC_TIMESTAMP(6)"
        );
        let updated = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                sql,
                [
                    status.into(),
                    error_code.into(),
                    response_id.as_str().into(),
                    lease_token.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        require_fenced_update(updated.rows_affected())
    }
}

fn require_fenced_update(rows_affected: u64) -> Result<(), InboundEventStoreError> {
    if rows_affected == 1 {
        Ok(())
    } else {
        Err(InboundEventStoreError::LeaseLost)
    }
}

#[derive(Debug, FromQueryResult)]
struct OwnerResponseClaimRow {
    response_id: String,
    response_json: String,
    reply_to_platform_message_id: String,
    event_kind: String,
    group_openid: Option<String>,
}
