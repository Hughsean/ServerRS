use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set, Statement, Value,
};

use crate::domain::risk::post_conversation_risk_audit::{
    NewPostConversationRiskAudit, PostConversationRiskAudit, PostRiskAuditResult,
};
use crate::domain::risk::risk_repository::RiskRepository;
use crate::shared::error::AppError;

use super::super::entities::post_conversation_risk_audits;

pub struct SeaOrmRiskRepository {
    db: DatabaseConnection,
}

impl SeaOrmRiskRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn map_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

fn decimal_to_f64(d: sea_orm::prelude::Decimal) -> f64 {
    use std::str::FromStr;
    f64::from_str(&d.to_string()).unwrap_or(0.0)
}

fn f64_to_decimal(v: f64) -> sea_orm::prelude::Decimal {
    use std::str::FromStr;
    let s = format!("{:.4}", v);
    sea_orm::prelude::Decimal::from_str(&s).unwrap_or(sea_orm::prelude::Decimal::ZERO)
}

fn map_audit(m: post_conversation_risk_audits::Model) -> PostConversationRiskAudit {
    PostConversationRiskAudit {
        audit_id: m.audit_id,
        user_id: m.user_id,
        conversation_id: m.conversation_id,
        audit_scope: m.audit_scope,
        user_message_ref_id: m.user_message_ref_id,
        assistant_message_ref_id: m.assistant_message_ref_id,
        user_message_id: m.user_message_id,
        assistant_message_id: m.assistant_message_id,
        status: m.status,
        risk_level: m.risk_level,
        risk_categories: m.risk_categories.map(|j| j.into()),
        confidence: m.confidence.map(decimal_to_f64),
        input_hash: m.input_hash,
        detector_name: m.detector_name,
        detector_version: m.detector_version,
        model_name: m.model_name,
        checked_at: m.checked_at.map(|v| v.and_utc()),
        error_message: m.error_message,
        metadata: m.metadata.map(|j| j.into()),
        source_deleted: m.source_deleted != 0,
        created_at: m.created_at.and_utc(),
        updated_at: m.updated_at.and_utc(),
    }
}

#[async_trait]
impl RiskRepository for SeaOrmRiskRepository {
    async fn create_pending(
        &self,
        new_audit: NewPostConversationRiskAudit,
    ) -> Result<PostConversationRiskAudit, AppError> {
        let now = Utc::now().naive_utc();
        let am = post_conversation_risk_audits::ActiveModel {
            audit_id: sea_orm::ActiveValue::NotSet,
            user_id: Set(new_audit.user_id),
            conversation_id: Set(new_audit.conversation_id),
            audit_scope: Set(new_audit.audit_scope),
            user_message_ref_id: Set(new_audit.user_message_ref_id),
            assistant_message_ref_id: Set(new_audit.assistant_message_ref_id),
            user_message_id: Set(new_audit.user_message_id),
            assistant_message_id: Set(new_audit.assistant_message_id),
            status: Set("pending".to_string()),
            risk_level: Set(None),
            risk_categories: Set(None),
            confidence: Set(None),
            input_hash: Set(None),
            detector_name: Set(None),
            detector_version: Set(None),
            model_name: Set(None),
            checked_at: Set(None),
            error_message: Set(None),
            metadata: Set(None),
            source_deleted: Set(0),
            created_at: Set(now),
            updated_at: Set(now),
        };
        let saved = am.insert(&self.db).await.map_err(map_err)?;
        Ok(map_audit(saved))
    }

    async fn fetch_pending(&self, limit: u64) -> Result<Vec<PostConversationRiskAudit>, AppError> {
        let rows = post_conversation_risk_audits::Entity::find()
            .filter(post_conversation_risk_audits::Column::Status.eq("pending"))
            .order_by_asc(post_conversation_risk_audits::Column::CreatedAt)
            .limit(limit)
            .all(&self.db)
            .await
            .map_err(map_err)?;
        Ok(rows.into_iter().map(map_audit).collect())
    }

    async fn mark_running(&self, audit_id: u64) -> Result<(), AppError> {
        let model = post_conversation_risk_audits::Entity::find_by_id(audit_id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or_else(|| AppError::NotFound(format!("risk audit {audit_id} not found")))?;
        let mut am: post_conversation_risk_audits::ActiveModel = model.into();
        am.status = Set("running".to_string());
        am.updated_at = Set(Utc::now().naive_utc());
        am.update(&self.db).await.map_err(map_err)?;
        Ok(())
    }

    async fn mark_completed(
        &self,
        audit_id: u64,
        result: PostRiskAuditResult,
    ) -> Result<(), AppError> {
        let model = post_conversation_risk_audits::Entity::find_by_id(audit_id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or_else(|| AppError::NotFound(format!("risk audit {audit_id} not found")))?;
        let mut am: post_conversation_risk_audits::ActiveModel = model.into();
        am.status = Set("completed".to_string());
        am.risk_level = Set(Some(result.risk_level));
        am.risk_categories = Set(result.risk_categories.map(|v| v.into()));
        am.confidence = Set(result.confidence.map(f64_to_decimal));
        am.input_hash = Set(result.input_hash);
        am.detector_name = Set(result.detector_name);
        am.detector_version = Set(result.detector_version);
        am.model_name = Set(result.model_name);
        am.checked_at = Set(Some(result.checked_at.naive_utc()));
        am.updated_at = Set(Utc::now().naive_utc());
        am.update(&self.db).await.map_err(map_err)?;
        Ok(())
    }

    async fn mark_failed(&self, audit_id: u64, error_message: String) -> Result<(), AppError> {
        let model = post_conversation_risk_audits::Entity::find_by_id(audit_id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or_else(|| AppError::NotFound(format!("risk audit {audit_id} not found")))?;
        let mut am: post_conversation_risk_audits::ActiveModel = model.into();
        am.status = Set("failed".to_string());
        am.error_message = Set(Some(error_message));
        am.updated_at = Set(Utc::now().naive_utc());
        am.update(&self.db).await.map_err(map_err)?;
        Ok(())
    }

    async fn delete_for_user(&self, user_id: u64) -> Result<u64, AppError> {
        let r = post_conversation_risk_audits::Entity::delete_many()
            .filter(post_conversation_risk_audits::Column::UserId.eq(user_id))
            .exec(&self.db)
            .await
            .map_err(map_err)?;
        Ok(r.rows_affected)
    }

    async fn delete_for_conversation(&self, conversation_id: u64) -> Result<u64, AppError> {
        let r = post_conversation_risk_audits::Entity::delete_many()
            .filter(post_conversation_risk_audits::Column::ConversationId.eq(conversation_id))
            .exec(&self.db)
            .await
            .map_err(map_err)?;
        Ok(r.rows_affected)
    }

    async fn find_by_user_id_paginated(
        &self,
        user_id: u64,
        limit: u64,
        offset: u64,
    ) -> Result<(Vec<PostConversationRiskAudit>, u64), AppError> {
        let paginator = post_conversation_risk_audits::Entity::find()
            .filter(post_conversation_risk_audits::Column::UserId.eq(user_id))
            .order_by_desc(post_conversation_risk_audits::Column::CreatedAt)
            .paginate(&self.db, limit);
        let total = paginator.num_items().await.map_err(map_err)?;
        let page_num = if limit > 0 { offset / limit } else { 0 };
        let items = paginator.fetch_page(page_num).await.map_err(map_err)?;
        Ok((items.into_iter().map(map_audit).collect(), total))
    }

    async fn find_by_conversation_id(
        &self,
        conversation_id: u64,
    ) -> Result<Vec<PostConversationRiskAudit>, AppError> {
        let rows = post_conversation_risk_audits::Entity::find()
            .filter(post_conversation_risk_audits::Column::ConversationId.eq(conversation_id))
            .order_by_desc(post_conversation_risk_audits::Column::CreatedAt)
            .all(&self.db)
            .await
            .map_err(map_err)?;
        Ok(rows.into_iter().map(map_audit).collect())
    }

    async fn find_all_paginated(
        &self,
        limit: u64,
        offset: u64,
        risk_level: Option<&str>,
    ) -> Result<(Vec<PostConversationRiskAudit>, u64), AppError> {
        let mut query = post_conversation_risk_audits::Entity::find();
        if let Some(level) = risk_level {
            query = query
                .filter(post_conversation_risk_audits::Column::RiskLevel.eq(level.to_string()));
        }
        let paginator = query
            .order_by_desc(post_conversation_risk_audits::Column::CreatedAt)
            .paginate(&self.db, limit);
        let total = paginator.num_items().await.map_err(map_err)?;
        let page_num = if limit > 0 { offset / limit } else { 0 };
        let items = paginator.fetch_page(page_num).await.map_err(map_err)?;
        Ok((items.into_iter().map(map_audit).collect(), total))
    }

    async fn find_conversation_ids_paginated(
        &self,
        limit: u64,
        offset: u64,
        risk_level: Option<&str>,
    ) -> Result<(Vec<u64>, u64), AppError> {
        let (filter, values) = match risk_level {
            Some(level) => (
                " AND risk_level = ?",
                vec![Value::String(Some(level.to_string()))],
            ),
            None => ("", Vec::new()),
        };

        let backend = self.db.get_database_backend();
        let count_sql = format!(
            "SELECT COUNT(DISTINCT conversation_id) AS total \
             FROM post_conversation_risk_audits \
             WHERE conversation_id <> 0{filter}"
        );
        let count_stmt = Statement::from_sql_and_values(backend, count_sql, values.clone());
        let total = self
            .db
            .query_one_raw(count_stmt)
            .await
            .map_err(map_err)?
            .map(|row| {
                row.try_get::<i64>("", "total")
                    .map(|v| v as u64)
                    .unwrap_or(0)
            })
            .unwrap_or(0);

        let page_sql = format!(
            "SELECT conversation_id, MAX(created_at) AS latest \
             FROM post_conversation_risk_audits \
             WHERE conversation_id <> 0{filter} \
             GROUP BY conversation_id ORDER BY latest DESC \
             LIMIT {limit} OFFSET {offset}"
        );
        let page_stmt = Statement::from_sql_and_values(backend, page_sql, values);
        let ids = self
            .db
            .query_all_raw(page_stmt)
            .await
            .map_err(map_err)?
            .into_iter()
            .map(|row| row.try_get::<u64>("", "conversation_id").map_err(map_err))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((ids, total))
    }

    async fn count_all(&self) -> Result<u64, AppError> {
        post_conversation_risk_audits::Entity::find()
            .count(&self.db)
            .await
            .map_err(map_err)
    }

    async fn count_trend(&self, days: u32) -> Result<Vec<(String, u64)>, AppError> {
        let since = chrono::Utc::now() - chrono::Duration::days(days as i64 - 1);
        let start = since.format("%Y-%m-%d").to_string();
        let stmt = Statement::from_sql_and_values(
            self.db.get_database_backend(),
            r#"
            SELECT DATE(created_at) AS day, COUNT(*) AS cnt
            FROM post_conversation_risk_audits
            WHERE created_at >= CAST(? AS DATETIME)
            GROUP BY DATE(created_at)
            ORDER BY day
            "#,
            vec![Value::String(Some(start))],
        );
        let rows = self.db.query_all_raw(stmt).await.map_err(map_err)?;
        let mut daily: Vec<(String, u64)> = rows
            .into_iter()
            .filter_map(|row| {
                let day: String = row.try_get("", "day").ok()?;
                let cnt: i64 = row.try_get("", "cnt").ok()?;
                Some((day, cnt as u64))
            })
            .collect();
        Ok(fill_trend_daily(days, &mut daily))
    }

    async fn count_by_risk_level(&self) -> Result<Vec<(String, u64)>, AppError> {
        let stmt = Statement::from_sql_and_values(
            self.db.get_database_backend(),
            r#"
            SELECT COALESCE(risk_level, 'unknown') AS level, COUNT(*) AS cnt
            FROM post_conversation_risk_audits
            GROUP BY risk_level
            "#,
            vec![],
        );
        let rows = self.db.query_all_raw(stmt).await.map_err(map_err)?;
        let result: Vec<(String, u64)> = rows
            .into_iter()
            .filter_map(|row| {
                let level: String = row.try_get("", "level").ok()?;
                let cnt: i64 = row.try_get("", "cnt").ok()?;
                Some((level, cnt as u64))
            })
            .collect();
        Ok(result)
    }
}

fn fill_trend_daily(days: u32, daily: &mut [(String, u64)]) -> Vec<(String, u64)> {
    daily.sort_by(|a, b| a.0.cmp(&b.0));
    let mut result = Vec::with_capacity(days as usize);
    let today = chrono::Utc::now().date_naive();
    for i in (0..days).rev() {
        let date = today - chrono::Duration::days(i as i64);
        let label = date.format("%m-%d").to_string();
        let full = date.format("%Y-%m-%d").to_string();
        let count = daily
            .iter()
            .find(|(d, _)| *d == full)
            .map(|(_, c)| *c)
            .unwrap_or(0);
        result.push((label, count));
    }
    result
}
