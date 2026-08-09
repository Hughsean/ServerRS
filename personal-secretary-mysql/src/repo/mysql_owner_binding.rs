use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};
use uuid::Uuid;

use crate::{InboundEventStoreError, OwnerBinding, OwnerBindingStoreT};

use super::mysql_inbound::store_error;

pub(crate) struct MySqlOwnerBindingStore {
    db: DatabaseConnection,
}

impl MySqlOwnerBindingStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl OwnerBindingStoreT for MySqlOwnerBindingStore {
    async fn ensure_owner_binding(
        &self,
        binding: &OwnerBinding,
    ) -> Result<(), InboundEventStoreError> {
        if binding.owner_actor_id.trim().is_empty() || binding.owner_actor_id.len() > 191 {
            return Err(InboundEventStoreError::InvalidData(
                "owner actor id must contain 1..=191 bytes".into(),
            ));
        }
        let transaction = self.db.begin().await.map_err(store_error)?;
        for account in [&binding.managed_account, &binding.command_account] {
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"INSERT INTO secretary_accounts (source_channel, platform_account_id, status)
                   VALUES (?, ?, 'active')
                   ON DUPLICATE KEY UPDATE status = 'active'"#,
                    [
                        account.channel.as_str().into(),
                        account.account_id.clone().into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
        }
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_owner_bindings existing
                   JOIN secretary_accounts managed ON managed.id = existing.managed_account_id
                   JOIN secretary_accounts command ON command.id = existing.command_account_id
                   SET existing.status = 'revoked'
                   WHERE existing.status = 'active'
                     AND existing.owner_actor_id <> ?
                     AND managed.source_channel = ? AND managed.platform_account_id = ?
                     AND command.source_channel = ? AND command.platform_account_id = ?"#,
                [
                    binding.owner_actor_id.clone().into(),
                    binding.managed_account.channel.as_str().into(),
                    binding.managed_account.account_id.clone().into(),
                    binding.command_account.channel.as_str().into(),
                    binding.command_account.account_id.clone().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT INTO secretary_owner_bindings
                 (binding_id, managed_account_id, command_account_id, owner_actor_id, status)
               SELECT ?, managed.id, command.id, ?, 'active'
               FROM secretary_accounts managed JOIN secretary_accounts command
               WHERE managed.source_channel = ? AND managed.platform_account_id = ?
                 AND command.source_channel = ? AND command.platform_account_id = ?
               ON DUPLICATE KEY UPDATE status = 'active'"#,
                [
                    Uuid::new_v4().to_string().into(),
                    binding.owner_actor_id.clone().into(),
                    binding.managed_account.channel.as_str().into(),
                    binding.managed_account.account_id.clone().into(),
                    binding.command_account.channel.as_str().into(),
                    binding.command_account.account_id.clone().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        transaction.commit().await.map_err(store_error)?;
        tracing::info!(
            managed_channel = binding.managed_account.channel.as_str(),
            "local QQ Open Platform owner binding ensured"
        );
        Ok(())
    }
}
