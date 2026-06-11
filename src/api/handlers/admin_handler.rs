use std::sync::Arc;
use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};

use crate::api::ApiState;
use crate::api::dto::session_dto::{ConversationMessageResponse, ConversationResponse};
use crate::application::auth::auth_service::AuthenticatedUser;
use crate::domain::risk::detection_types::RiskLevel;
use crate::domain::user::user::{UserStatus, UserUpdate};
use crate::shared::error::AppError;

// ── DTOs ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct UserDto {
    pub id: u64,
    pub username: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub nickname: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_login_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PaginatedUsers {
    pub items: Vec<UserDto>,
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
}

#[derive(Debug, Deserialize)]
pub struct PageQuery {
    pub page: Option<u64>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct RiskConvQuery {
    pub page: Option<u64>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<u64>,
    #[serde(rename = "riskLevel")]
    pub risk_level: Option<RiskLevel>,
}

#[derive(Debug, Deserialize)]
pub struct AdminPatchUser {
    pub status: Option<i32>,
    pub role: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ProcessNotes {
    pub notes: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RiskConversationDetail {
    pub conversation: ConversationResponse,
    pub messages: Vec<ConversationMessageResponse>,
    pub risk_detections: Vec<RiskDetectionAdminDto>,
}

#[derive(Debug, Serialize)]
pub struct RiskDetectionAdminDto {
    pub id: u64,
    pub conversation_id: Option<u64>,
    pub risk_level: String,
    pub polarity: String,
    pub intent: String,
    pub confidence: f64,
    pub reason: Option<String>,
    pub is_processed: bool,
    pub process_notes: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct PaginatedRiskConversations {
    pub items: Vec<ConversationResponse>,
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn enum_str<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v)
        .unwrap_or_default()
        .trim_matches('"')
        .to_string()
}

// ── Handlers ──────────────────────────────────────────────────────────────────

pub async fn list_users(
    Extension(state): Extension<Arc<ApiState>>,
    Query(q): Query<PageQuery>,
) -> Result<Json<PaginatedUsers>, AppError> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);

    let all = state.user.list_users().await?;
    let total = all.len() as u64;
    let offset = ((page - 1) * page_size) as usize;
    let items = all
        .into_iter()
        .skip(offset)
        .take(page_size as usize)
        .map(|u| UserDto {
            id: u.id,
            username: u.username,
            email: u.email,
            phone: u.phone,
            nickname: u.nickname,
            status: u.status.as_str().to_string(),
            created_at: u.created_at.to_rfc3339(),
            updated_at: u.updated_at.to_rfc3339(),
            last_login_at: u.last_login_at.map(|t| t.to_rfc3339()),
        })
        .collect();

    Ok(Json(PaginatedUsers {
        items,
        page,
        page_size,
        total,
    }))
}

pub async fn get_user(
    Extension(state): Extension<Arc<ApiState>>,
    Path(id): Path<u64>,
) -> Result<Json<UserDto>, AppError> {
    let u = state
        .user
        .admin_get_user(id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("user {id} not found")))?;

    Ok(Json(UserDto {
        id: u.id,
        username: u.username,
        email: u.email,
        phone: u.phone,
        nickname: u.nickname,
        status: u.status.as_str().to_string(),
        created_at: u.created_at.to_rfc3339(),
        updated_at: u.updated_at.to_rfc3339(),
        last_login_at: u.last_login_at.map(|t| t.to_rfc3339()),
    }))
}

pub async fn patch_user(
    Extension(state): Extension<Arc<ApiState>>,
    Path(id): Path<u64>,
    Json(body): Json<AdminPatchUser>,
) -> Result<Json<UserDto>, AppError> {
    let status = body
        .status
        .map(|s| UserStatus::from_i32(s).unwrap_or(UserStatus::Disabled));

    let update = UserUpdate {
        email: None,
        phone: None,
        nickname: None,
        status,
        role: None,
    };

    let u = state.user.admin_update_user(id, update).await?;

    Ok(Json(UserDto {
        id: u.id,
        username: u.username,
        email: u.email,
        phone: u.phone,
        nickname: u.nickname,
        status: u.status.as_str().to_string(),
        created_at: u.created_at.to_rfc3339(),
        updated_at: u.updated_at.to_rfc3339(),
        last_login_at: u.last_login_at.map(|t| t.to_rfc3339()),
    }))
}

pub async fn delete_user(
    Extension(state): Extension<Arc<ApiState>>,
    Path(id): Path<u64>,
) -> Result<impl IntoResponse, AppError> {
    state.user.admin_delete_user(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_risk_conversations(
    Extension(state): Extension<Arc<ApiState>>,
    Query(q): Query<RiskConvQuery>,
) -> Result<Json<PaginatedRiskConversations>, AppError> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).min(100);

    let (items, total) = state
        .query
        .admin_list_risk_conversations(page, page_size, q.risk_level)
        .await?;

    Ok(Json(PaginatedRiskConversations {
        items: items
            .into_iter()
            .map(|c| ConversationResponse {
                id: c.id,
                user_id: c.user_id,
                title: c.title,
                is_title_generated: c.is_title_generated,
                last_message_at: c.last_message_at.map(|t| t.to_rfc3339()),
                message_count: c.message_count,
                created_at: c.created_at.to_rfc3339(),
            })
            .collect(),
        page,
        page_size,
        total,
    }))
}

pub async fn get_risk_conversation(
    Extension(state): Extension<Arc<ApiState>>,
    Path(id): Path<u64>,
) -> Result<Json<RiskConversationDetail>, AppError> {
    let conv = state
        .query
        .admin_get_conversation(id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("conversation {id} not found")))?;

    let messages = state.query.admin_get_conversation_messages(id).await?;

    let detections = state
        .query
        .admin_get_conversation_risk_detections(id)
        .await?;

    Ok(Json(RiskConversationDetail {
        conversation: ConversationResponse {
            id: conv.id,
            user_id: conv.user_id,
            title: conv.title,
            is_title_generated: conv.is_title_generated,
            last_message_at: conv.last_message_at.map(|t| t.to_rfc3339()),
            message_count: conv.message_count,
            created_at: conv.created_at.to_rfc3339(),
        },
        messages: messages
            .into_iter()
            .map(|m| ConversationMessageResponse {
                id: m.id,
                conversation_id: m.conversation_id,
                sender_role: m.sender_role,
                sender_user_id: m.sender_user_id,
                message_type: m.message_type,
                content: m.content,
                token_count: m.token_count,
                created_at: m.created_at.to_rfc3339(),
            })
            .collect(),
        risk_detections: detections
            .into_iter()
            .map(|r| RiskDetectionAdminDto {
                id: r.id,
                conversation_id: r.conversation_id,
                risk_level: enum_str(&r.risk_level),
                polarity: enum_str(&r.polarity),
                intent: enum_str(&r.intent),
                confidence: r.confidence,
                reason: r.reason,
                is_processed: r.is_processed,
                process_notes: r.process_notes,
                created_at: r.created_at.to_rfc3339(),
            })
            .collect(),
    }))
}

pub async fn process_risk_detection(
    Extension(state): Extension<Arc<ApiState>>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(id): Path<u64>,
    Json(body): Json<ProcessNotes>,
) -> Result<Json<RiskDetectionAdminDto>, AppError> {
    let r = state
        .query
        .admin_process_risk_detection(id, auth.user_id, body.notes)
        .await?;

    Ok(Json(RiskDetectionAdminDto {
        id: r.id,
        conversation_id: r.conversation_id,
        risk_level: enum_str(&r.risk_level),
        polarity: enum_str(&r.polarity),
        intent: enum_str(&r.intent),
        confidence: r.confidence,
        reason: r.reason,
        is_processed: r.is_processed,
        process_notes: r.process_notes,
        created_at: r.created_at.to_rfc3339(),
    }))
}

