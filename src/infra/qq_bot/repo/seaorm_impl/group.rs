use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, JsonValue, QueryFilter, Set};

use crate::domain::qq_bot::config::{GroupConfig, MemoryPolicy, ReplyPolicy, TriggerPolicy};
use crate::domain::qq_bot::repository::GroupRepoT;
use crate::shared::error::AppError;

use crate::infra::repo::entities::qq_groups;

pub struct GroupRepo {
    db: DatabaseConnection,
}

impl GroupRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn trigger_policy_from_str(s: &str) -> TriggerPolicy {
    match s {
        "mention" => TriggerPolicy::Mention,
        "keyword" => TriggerPolicy::Keyword,
        "command" => TriggerPolicy::Command,
        "always" => TriggerPolicy::Always,
        "silent" => TriggerPolicy::Silent,
        _ => TriggerPolicy::Mention,
    }
}

fn trigger_policy_to_str(p: TriggerPolicy) -> &'static str {
    match p {
        TriggerPolicy::Mention => "mention",
        TriggerPolicy::Keyword => "keyword",
        TriggerPolicy::Command => "command",
        TriggerPolicy::Always => "always",
        TriggerPolicy::Silent => "silent",
    }
}

fn memory_policy_from_str(s: &str) -> MemoryPolicy {
    match s {
        "off" => MemoryPolicy::Off,
        "group_only" => MemoryPolicy::GroupOnly,
        "opt_in_user" => MemoryPolicy::OptInUser,
        _ => MemoryPolicy::Off,
    }
}

fn memory_policy_to_str(p: MemoryPolicy) -> &'static str {
    match p {
        MemoryPolicy::Off => "off",
        MemoryPolicy::GroupOnly => "group_only",
        MemoryPolicy::OptInUser => "opt_in_user",
    }
}

fn model_to_domain(m: qq_groups::Model) -> GroupConfig {
    let keywords = m
        .keywords
        .map(|v| serde_json::from_value::<Vec<String>>(v).unwrap_or_default())
        .unwrap_or_default();

    GroupConfig {
        qq_group_id: m.qq_group_id,
        group_name: m.group_name,
        bot_account_id: m.bot_account_id,
        enabled: m.enabled != 0,
        trigger_policy: trigger_policy_from_str(&m.trigger_policy),
        reply_policy: ReplyPolicy {
            cooldown_secs: m.cooldown_secs,
            max_segments: m.max_segments,
            max_chars_per_segment: m.max_chars_per_segment,
            allow_proactive: m.allow_proactive != 0,
            keywords,
        },
        memory_policy: memory_policy_from_str(&m.memory_policy),
    }
}

fn domain_to_active_model(group: &GroupConfig) -> qq_groups::ActiveModel {
    let keywords = if group.reply_policy.keywords.is_empty() {
        None
    } else {
        Some(JsonValue::from(
            serde_json::to_value(&group.reply_policy.keywords).unwrap(),
        ))
    };

    qq_groups::ActiveModel {
        qq_group_id: Set(group.qq_group_id),
        group_name: Set(group.group_name.clone()),
        bot_account_id: Set(group.bot_account_id),
        enabled: Set(if group.enabled { 1 } else { 0 }),
        trigger_policy: Set(trigger_policy_to_str(group.trigger_policy).to_string()),
        cooldown_secs: Set(group.reply_policy.cooldown_secs),
        max_segments: Set(group.reply_policy.max_segments),
        max_chars_per_segment: Set(group.reply_policy.max_chars_per_segment),
        allow_proactive: Set(if group.reply_policy.allow_proactive {
            1
        } else {
            0
        }),
        keywords: Set(keywords),
        memory_policy: Set(memory_policy_to_str(group.memory_policy).to_string()),
        last_seen_at: Set(None),
        ..Default::default()
    }
}

fn map_db_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

#[async_trait]
impl GroupRepoT for GroupRepo {
    async fn find_by_group_id(&self, qq_group_id: i64) -> Result<Option<GroupConfig>, AppError> {
        qq_groups::Entity::find_by_id(qq_group_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)
            .map(|opt| opt.map(model_to_domain))
    }

    async fn find_enabled_by_bot(&self, bot_account_id: u64) -> Result<Vec<GroupConfig>, AppError> {
        qq_groups::Entity::find()
            .filter(qq_groups::Column::BotAccountId.eq(bot_account_id))
            .filter(qq_groups::Column::Enabled.eq(1))
            .all(&self.db)
            .await
            .map_err(map_db_err)
            .map(|rows| rows.into_iter().map(model_to_domain).collect())
    }

    async fn upsert(&self, group: &GroupConfig) -> Result<GroupConfig, AppError> {
        let model = domain_to_active_model(group);

        let update_columns = vec![
            qq_groups::Column::GroupName,
            qq_groups::Column::BotAccountId,
            qq_groups::Column::Enabled,
            qq_groups::Column::TriggerPolicy,
            qq_groups::Column::CooldownSecs,
            qq_groups::Column::MaxSegments,
            qq_groups::Column::MaxCharsPerSegment,
            qq_groups::Column::AllowProactive,
            qq_groups::Column::Keywords,
            qq_groups::Column::MemoryPolicy,
            qq_groups::Column::LastSeenAt,
        ];

        qq_groups::Entity::insert_many([model])
            .on_conflict(
                sea_orm::sea_query::OnConflict::columns([qq_groups::Column::QqGroupId])
                    .update_columns(update_columns)
                    .to_owned(),
            )
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;

        // Fetch the upserted record
        self.find_by_group_id(group.qq_group_id)
            .await?
            .ok_or_else(|| AppError::Internal("group not found after upsert".into()))
    }

    async fn update_last_seen(&self, qq_group_id: i64, _last_seen_at: i64) -> Result<(), AppError> {
        use sea_orm::sea_query::SimpleExpr;
        qq_groups::Entity::update_many()
            .col_expr(
                qq_groups::Column::LastSeenAt,
                SimpleExpr::Value(sea_orm::Value::BigInt(Some(_last_seen_at))),
            )
            .filter(qq_groups::Column::QqGroupId.eq(qq_group_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn update_trigger_policy(
        &self,
        qq_group_id: i64,
        policy: TriggerPolicy,
    ) -> Result<(), AppError> {
        use sea_orm::sea_query::SimpleExpr;
        qq_groups::Entity::update_many()
            .col_expr(
                qq_groups::Column::TriggerPolicy,
                SimpleExpr::Value(sea_orm::Value::String(Some(
                    trigger_policy_to_str(policy).to_string(),
                ))),
            )
            .filter(qq_groups::Column::QqGroupId.eq(qq_group_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn set_enabled(&self, qq_group_id: i64, enabled: bool) -> Result<(), AppError> {
        use sea_orm::sea_query::SimpleExpr;
        qq_groups::Entity::update_many()
            .col_expr(
                qq_groups::Column::Enabled,
                SimpleExpr::Value(sea_orm::Value::TinyInt(Some(if enabled {
                    1i8
                } else {
                    0i8
                }))),
            )
            .filter(qq_groups::Column::QqGroupId.eq(qq_group_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }
}
