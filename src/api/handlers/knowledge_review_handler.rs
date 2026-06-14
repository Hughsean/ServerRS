use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::api::AdminState;
use crate::app::auth::auth_service::AuthenticatedUser;
use crate::domain::web_ingestion::review::{
    KnowledgeReviewAuditEntry, KnowledgeReviewDetail, KnowledgeReviewItem, KnowledgeReviewPage,
    ReviewPublishRequest,
};
use crate::shared::error::AppError;

#[derive(Debug, Deserialize)]
pub struct ReviewListQuery {
    pub page: Option<u64>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<u64>,
    pub status: Option<String>,
    #[serde(rename = "sourceId")]
    pub source_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct PublishReviewedKnowledge {
    pub notes: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct KnowledgeReviewDto {
    pub publish_record_id: u64,
    pub source_id: u64,
    pub source_name: String,
    pub page_id: u64,
    pub run_id: u64,
    pub document_id: u64,
    pub version_key: String,
    pub title: Option<String>,
    pub source_url: String,
    pub publish_status: String,
    pub active: bool,
    pub run_status: String,
    pub run_stage: String,
    pub quality_score: Option<f64>,
    pub quality_result: Option<JsonValue>,
    pub risk_flags: Option<JsonValue>,
    pub should_publish: Option<bool>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct KnowledgeReviewListDto {
    pub items: Vec<KnowledgeReviewDto>,
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
}

#[derive(Debug, Serialize)]
pub struct KnowledgeReviewAuditDto {
    pub action: String,
    pub status: String,
    pub message: String,
    pub metadata: Option<JsonValue>,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct KnowledgeReviewDetailDto {
    pub review: KnowledgeReviewDto,
    pub clean_text: Option<String>,
    pub distilled_json: Option<JsonValue>,
    pub audit_logs: Vec<KnowledgeReviewAuditDto>,
}

#[derive(Debug, Serialize)]
pub struct ReviewPublishRequestDto {
    pub publish_record_id: u64,
    pub event_id: u64,
    pub event_status: String,
    pub already_requested: bool,
}

pub async fn list_reviews(
    State(state): State<AdminState>,
    Query(query): Query<ReviewListQuery>,
) -> Result<Json<KnowledgeReviewListDto>, AppError> {
    let page = state
        .knowledge_review
        .list(
            query.status.as_deref(),
            query.source_id,
            query.page.unwrap_or(1),
            query.page_size.unwrap_or(20),
        )
        .await?;
    Ok(Json(map_page(page)))
}

pub async fn get_review(
    State(state): State<AdminState>,
    Path(publish_record_id): Path<u64>,
) -> Result<Json<KnowledgeReviewDetailDto>, AppError> {
    let item = state.knowledge_review.get(publish_record_id).await?;
    Ok(Json(map_detail(item)))
}

pub async fn publish_reviewed(
    State(state): State<AdminState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(publish_record_id): Path<u64>,
    Json(body): Json<PublishReviewedKnowledge>,
) -> Result<(StatusCode, Json<ReviewPublishRequestDto>), AppError> {
    let result = state
        .knowledge_review
        .request_publish(publish_record_id, auth.user_id, auth.username, body.notes)
        .await?;
    Ok((StatusCode::ACCEPTED, Json(map_publish_request(result))))
}

fn map_page(page: KnowledgeReviewPage) -> KnowledgeReviewListDto {
    KnowledgeReviewListDto {
        items: page.items.into_iter().map(map_item).collect(),
        page: page.page,
        page_size: page.page_size,
        total: page.total,
    }
}

fn map_item(item: KnowledgeReviewItem) -> KnowledgeReviewDto {
    KnowledgeReviewDto {
        publish_record_id: item.publish_record_id,
        source_id: item.source_id,
        source_name: item.source_name,
        page_id: item.page_id,
        run_id: item.run_id,
        document_id: item.document_id,
        version_key: item.version_key,
        title: item.title,
        source_url: item.source_url,
        publish_status: item.publish_status,
        active: item.active,
        run_status: item.run_status,
        run_stage: item.run_stage,
        quality_score: item.quality_score,
        quality_result: item.quality_result,
        risk_flags: item.risk_flags,
        should_publish: item.should_publish,
        created_at: item.created_at.to_rfc3339(),
        updated_at: item.updated_at.to_rfc3339(),
    }
}

fn map_publish_request(result: ReviewPublishRequest) -> ReviewPublishRequestDto {
    ReviewPublishRequestDto {
        publish_record_id: result.publish_record_id,
        event_id: result.event_id,
        event_status: result.event_status,
        already_requested: result.already_requested,
    }
}

fn map_detail(detail: KnowledgeReviewDetail) -> KnowledgeReviewDetailDto {
    KnowledgeReviewDetailDto {
        review: map_item(detail.review),
        clean_text: detail.clean_text,
        distilled_json: detail.distilled_json,
        audit_logs: detail.audit_logs.into_iter().map(map_audit).collect(),
    }
}

fn map_audit(entry: KnowledgeReviewAuditEntry) -> KnowledgeReviewAuditDto {
    KnowledgeReviewAuditDto {
        action: entry.action,
        status: entry.status,
        message: entry.message,
        metadata: entry.metadata,
        created_at: entry.created_at.to_rfc3339(),
    }
}
