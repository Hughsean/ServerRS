use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};

use crate::domain::risk::risk_detection_result::{NewRiskDetectionResult, RiskDetectionResult};
use crate::domain::risk::risk_repository::RiskRepository;
use crate::shared::error::AppError;

use super::entities::risk_detection_result;

pub struct SeaOrmRiskRepository {
    db: DatabaseConnection,
}

impl SeaOrmRiskRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn map(m: risk_detection_result::Model) -> RiskDetectionResult {
    RiskDetectionResult {
        id: m.id,
        user_id: m.user_id,
        message_id: m.message_id,
        conversation_id: m.conversation_id,
        risk_level: m.risk_level,
        polarity: m.polarity,
        intent: m.intent,
        target: m.target,
        confidence: m.confidence,
        evidence: m.evidence,
        reason: m.reason,
        raw_payload: m.raw_payload,
        model_name: m.model_name,
        detector_version: m.detector_version,
        is_processed: m.is_processed,
        process_notes: m.process_notes,
        created_at: m.created_at,
    }
}

fn map_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

#[async_trait]
impl RiskRepository for SeaOrmRiskRepository {
    async fn save(&self, r: NewRiskDetectionResult) -> Result<RiskDetectionResult, AppError> {
        let now = chrono::Utc::now();
        let am = risk_detection_result::ActiveModel {
            user_id: Set(r.user_id),
            message_id: Set(r.message_id),
            conversation_id: Set(r.conversation_id),
            risk_level: Set(r.risk_level),
            polarity: Set(r.polarity),
            intent: Set(r.intent),
            target: Set(r.target),
            confidence: Set(r.confidence),
            evidence: Set(r.evidence),
            reason: Set(r.reason),
            raw_payload: Set(r.raw_payload),
            model_name: Set(r.model_name),
            detector_version: Set(r.detector_version),
            is_processed: Set(false),
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
        let paginator = risk_detection_result::Entity::find()
            .filter(risk_detection_result::Column::UserId.eq(user_id))
            .order_by_desc(risk_detection_result::Column::CreatedAt)
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
        risk_detection_result::Entity::find()
            .filter(risk_detection_result::Column::ConversationId.eq(cid))
            .order_by_asc(risk_detection_result::Column::CreatedAt)
            .all(&self.db)
            .await
            .map_err(map_err)
            .map(|v| v.into_iter().map(map).collect())
    }

    async fn delete_by_conversation_id(&self, cid: u64) -> Result<u64, AppError> {
        let r = risk_detection_result::Entity::delete_many()
            .filter(risk_detection_result::Column::ConversationId.eq(cid))
            .exec(&self.db)
            .await
            .map_err(map_err)?;
        Ok(r.rows_affected)
    }
}
