use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, Set, Statement, Value,
};

use crate::domain::risk::detection_types::RiskLevel;
use crate::domain::risk::risk_detection_result::{NewRiskDetectionResult, RiskDetectionResult};
use crate::domain::risk::risk_repository::RiskRepository;
use crate::shared::error::AppError;

use super::super::entities::risk_detection_results;

// ── Enum ↔ String helpers (serde-based, matches SCREAMING_SNAKE_CASE) ──

fn enum_to_str<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v)
        .unwrap_or_default()
        .trim_matches('"')
        .to_string()
}

fn str_to_enum<T: serde::de::DeserializeOwned>(s: &str) -> T {
    serde_json::from_str(&format!("\"{}\"", s))
        .unwrap_or_else(|_| serde_json::from_str("\"UNKNOWN\"").unwrap())
}

pub struct SeaOrmRiskRepository {
    db: DatabaseConnection,
}

impl SeaOrmRiskRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn map(m: risk_detection_results::Model) -> RiskDetectionResult {
    RiskDetectionResult {
        id: m.id,
        user_id: m.user_id,
        message_id: Some(m.message_id),
        conversation_id: Some(m.conversation_id),
        risk_level: str_to_enum(&m.risk_level),
        polarity: str_to_enum(&m.polarity),
        intent: str_to_enum(&m.intent),
        target: str_to_enum(&m.target),
        confidence: m.confidence.to_string().parse::<f64>().unwrap_or(0.0),
        evidence: m
            .evidence
            .map(|v| serde_json::to_string(&v).unwrap_or_default())
            .unwrap_or_default(),
        reason: m.reason,
        raw_payload: m
            .raw_payload
            .map(|v| serde_json::to_string(&v).unwrap_or_default()),
        model_name: m.model_name,
        detector_version: m.detector_version,
        is_processed: m.is_processed != 0,
        process_notes: m.process_notes,
        created_at: m.created_at,
    }
}

fn map_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

fn json_from_str(s: &str) -> Option<serde_json::Value> {
    serde_json::from_str(s).ok()
}

fn json_from_opt_str(s: &Option<String>) -> Option<serde_json::Value> {
    s.as_ref().and_then(|s| serde_json::from_str(s).ok())
}

#[async_trait]
impl RiskRepository for SeaOrmRiskRepository {
    async fn save(&self, r: NewRiskDetectionResult) -> Result<RiskDetectionResult, AppError> {
        let now = chrono::Utc::now();
        let decimal_confidence = {
            let s = format!("{:.3}", r.confidence);
            s.parse::<sea_orm::prelude::Decimal>()
                .unwrap_or(sea_orm::prelude::Decimal::ZERO)
        };
        let am = risk_detection_results::ActiveModel {
            user_id: Set(r.user_id),
            message_id: Set(r.message_id.unwrap_or(0)),
            conversation_id: Set(r.conversation_id.unwrap_or(0)),
            risk_level: Set(enum_to_str(&r.risk_level)),
            polarity: Set(enum_to_str(&r.polarity)),
            intent: Set(enum_to_str(&r.intent)),
            target: Set(enum_to_str(&r.target)),
            confidence: Set(decimal_confidence),
            evidence: Set(json_from_str(&r.evidence)),
            reason: Set(r.reason),
            raw_payload: Set(json_from_opt_str(&r.raw_payload)),
            model_name: Set(r.model_name),
            detector_version: Set(r.detector_version),
            is_processed: Set(0_i8),
            process_notes: Set(None),
            created_at: Set(now),
            ..Default::default()
        };
        let inserted = am.insert(&self.db).await.map_err(map_err)?;
        Ok(map(inserted))
    }

    async fn find_by_user_id_paginated(
        &self,
        user_id: u64,
        limit: u64,
        offset: u64,
    ) -> Result<(Vec<RiskDetectionResult>, u64), AppError> {
        let paginator = risk_detection_results::Entity::find()
            .filter(risk_detection_results::Column::UserId.eq(user_id))
            .order_by_desc(risk_detection_results::Column::CreatedAt)
            .paginate(&self.db, limit);
        let count = paginator.num_items().await.map_err(map_err)?;
        let page_num = offset / limit;
        let items = paginator.fetch_page(page_num).await.map_err(map_err)?;
        Ok((items.into_iter().map(map).collect(), count))
    }

    async fn find_by_conversation_id(
        &self,
        cid: u64,
    ) -> Result<Vec<RiskDetectionResult>, AppError> {
        risk_detection_results::Entity::find()
            .filter(risk_detection_results::Column::ConversationId.eq(cid))
            .order_by_asc(risk_detection_results::Column::CreatedAt)
            .all(&self.db)
            .await
            .map_err(map_err)
            .map(|v| v.into_iter().map(map).collect())
    }

    async fn find_all_paginated(
        &self,
        limit: u64,
        offset: u64,
        risk_level: Option<RiskLevel>,
    ) -> Result<(Vec<RiskDetectionResult>, u64), AppError> {
        let mut query = risk_detection_results::Entity::find();
        if let Some(level) = risk_level {
            query = query.filter(risk_detection_results::Column::RiskLevel.eq(enum_to_str(&level)));
        }

        let paginator = query
            .order_by_desc(risk_detection_results::Column::CreatedAt)
            .paginate(&self.db, limit);
        let count = paginator.num_items().await.map_err(map_err)?;
        let page_num = offset / limit;
        let items = paginator.fetch_page(page_num).await.map_err(map_err)?;
        Ok((items.into_iter().map(map).collect(), count))
    }

    async fn find_conversation_ids_paginated(
        &self,
        limit: u64,
        offset: u64,
        risk_level: Option<RiskLevel>,
    ) -> Result<(Vec<u64>, u64), AppError> {
        let (filter, values) = match risk_level {
            Some(level) => (
                " AND risk_level = ?",
                vec![Value::String(Some(enum_to_str(&level)))],
            ),
            None => ("", Vec::new()),
        };
        let backend = self.db.get_database_backend();
        let count_sql = format!(
            "SELECT COUNT(DISTINCT conversation_id) AS total \
             FROM risk_detection_results WHERE conversation_id <> 0{filter}"
        );
        let count_statement = Statement::from_sql_and_values(backend, count_sql, values.clone());
        let total = self
            .db
            .query_one_raw(count_statement)
            .await
            .map_err(map_err)?
            .map(|row| row.try_get::<u64>("", "total"))
            .transpose()
            .map_err(map_err)?
            .unwrap_or(0);

        let page_sql = format!(
            "SELECT conversation_id, MAX(created_at) AS latest \
             FROM risk_detection_results \
             WHERE conversation_id <> 0{filter} \
             GROUP BY conversation_id ORDER BY latest DESC \
             LIMIT {limit} OFFSET {offset}"
        );
        let page_statement = Statement::from_sql_and_values(backend, page_sql, values);
        let ids = self
            .db
            .query_all_raw(page_statement)
            .await
            .map_err(map_err)?
            .into_iter()
            .map(|row| row.try_get::<u64>("", "conversation_id").map_err(map_err))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((ids, total))
    }

    async fn mark_processed(
        &self,
        id: u64,
        notes: Option<String>,
    ) -> Result<RiskDetectionResult, AppError> {
        let existing = risk_detection_results::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or_else(|| AppError::NotFound(format!("risk detection {id} not found")))?;

        let mut active: risk_detection_results::ActiveModel = existing.into();
        active.is_processed = Set(1_i8);
        active.process_notes = Set(notes);
        let updated = active.update(&self.db).await.map_err(map_err)?;
        Ok(map(updated))
    }

    async fn delete_by_conversation_id(&self, cid: u64) -> Result<u64, AppError> {
        let r = risk_detection_results::Entity::delete_many()
            .filter(risk_detection_results::Column::ConversationId.eq(cid))
            .exec(&self.db)
            .await
            .map_err(map_err)?;
        Ok(r.rows_affected)
    }
}
