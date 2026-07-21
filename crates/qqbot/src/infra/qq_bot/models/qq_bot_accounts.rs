use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Default, Debug, DeriveEntity)]
pub struct Entity;

impl EntityName for Entity {
    fn table_name(&self) -> &'static str {
        "qq_bot_accounts"
    }
}

#[derive(Clone, Debug, PartialEq, DeriveModel, DeriveActiveModel, Eq, Serialize, Deserialize)]
pub struct Model {
    pub bot_account_id: u64,
    pub platform: String,
    pub self_qq_id: i64,
    pub display_name: Option<String>,
    pub adapter: String,
    pub connection_mode: String,
    pub enabled: i8,
    pub config: Option<Json>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveColumn)]
pub enum Column {
    BotAccountId,
    Platform,
    SelfQqId,
    DisplayName,
    Adapter,
    ConnectionMode,
    Enabled,
    Config,
    CreatedAt,
    UpdatedAt,
}

impl ColumnTrait for Column {
    type EntityName = Entity;
    fn def(&self) -> ColumnDef {
        match self {
            Self::BotAccountId => ColumnType::BigUnsigned.def(),
            Self::Platform => ColumnType::String(StringLen::N(32)).def(),
            Self::SelfQqId => ColumnType::BigInteger.def().unique(),
            Self::DisplayName => ColumnType::String(StringLen::N(128)).def().null(),
            Self::Adapter => ColumnType::String(StringLen::N(64)).def(),
            Self::ConnectionMode => ColumnType::String(StringLen::N(32)).def(),
            Self::Enabled => ColumnType::TinyInteger.def(),
            Self::Config => ColumnType::Json.def().null(),
            Self::CreatedAt => ColumnType::DateTime.def(),
            Self::UpdatedAt => ColumnType::DateTime.def(),
        }
    }
}

#[derive(Copy, Clone, Debug, EnumIter)]
pub enum PrimaryKey {
    BotAccountId,
}

impl PrimaryKeyTrait for PrimaryKey {
    type ValueType = u64;
    fn auto_increment() -> bool { true }
}

impl ActiveModelBehavior for ActiveModel {}
