use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set, TryIntoModel,
};

use crate::domain::qq_bot::attention::BotAccount;
use crate::domain::qq_bot::repository::BotAccountRepository;
use crate::shared::error::AppError;

use crate::infra::db::entities::qq_bot_accounts;

pub struct SeaOrmBotAccountRepository {
    db: DatabaseConnection,
}

impl SeaOrmBotAccountRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn model_to_domain(m: qq_bot_accounts::Model) -> BotAccount {
    BotAccount {
        bot_account_id: m.bot_account_id,
        platform: m.platform,
        self_qq_id: m.self_qq_id,
        display_name: m.display_name,
        adapter: m.adapter,
        connection_mode: m.connection_mode,
        enabled: m.enabled != 0,
    }
}

fn map_db_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

#[async_trait]
impl BotAccountRepository for SeaOrmBotAccountRepository {
    async fn find_by_self_qq_id(&self, self_qq_id: i64) -> Result<Option<BotAccount>, AppError> {
        qq_bot_accounts::Entity::find()
            .filter(qq_bot_accounts::Column::SelfQqId.eq(self_qq_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)
            .map(|opt| opt.map(model_to_domain))
    }

    async fn find_enabled(&self) -> Result<Vec<BotAccount>, AppError> {
        qq_bot_accounts::Entity::find()
            .filter(qq_bot_accounts::Column::Enabled.eq(1))
            .all(&self.db)
            .await
            .map_err(map_db_err)
            .map(|rows| rows.into_iter().map(model_to_domain).collect())
    }

    async fn upsert(&self, account: &BotAccount) -> Result<BotAccount, AppError> {
        // Check if a record with this self_qq_id already exists
        let existing = self.find_by_self_qq_id(account.self_qq_id).await?;

        let model = match existing {
            Some(existing) => {
                // Update existing record
                qq_bot_accounts::ActiveModel {
                    bot_account_id: Set(existing.bot_account_id),
                    platform: Set(account.platform.clone()),
                    self_qq_id: Set(account.self_qq_id),
                    display_name: Set(account.display_name.clone()),
                    adapter: Set(account.adapter.clone()),
                    connection_mode: Set(account.connection_mode.clone()),
                    enabled: Set(if account.enabled { 1 } else { 0 }),
                    ..Default::default()
                }
            }
            None => {
                // Insert new record (bot_account_id is auto-increment)
                qq_bot_accounts::ActiveModel {
                    bot_account_id: Set(0u64), // ignored for auto-increment
                    platform: Set(account.platform.clone()),
                    self_qq_id: Set(account.self_qq_id),
                    display_name: Set(account.display_name.clone()),
                    adapter: Set(account.adapter.clone()),
                    connection_mode: Set(account.connection_mode.clone()),
                    enabled: Set(if account.enabled { 1 } else { 0 }),
                    ..Default::default()
                }
            }
        };

        let result = model.save(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_domain(result.try_into_model().unwrap()))
    }
}
