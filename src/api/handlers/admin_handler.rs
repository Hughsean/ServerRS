use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};

use crate::api::AdminState;
use crate::app::auth::auth_service::AuthenticatedUser;
use crate::domain::user::user::{UserRole, UserStatus, UserUpdate};
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
    pub role: String,
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
    pub risk_level: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AdminPatchUser {
    pub status: Option<i32>,
    pub role: Option<String>,
}

// ── Admin conversation DTOs ──────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ConversationResponse {
    pub id: u64,
    pub user_id: u64,
    pub title: Option<String>,
    pub is_title_generated: bool,
    pub last_message_at: Option<String>,
    pub message_count: i32,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct ConversationMessageResponse {
    pub id: u64,
    pub conversation_id: u64,
    pub sender_role: String,
    pub sender_user_id: Option<u64>,
    pub message_type: String,
    pub content: String,
    pub token_count: Option<i32>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct ProcessNotes {
    pub notes: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RiskConversationDetail {
    pub conversation: ConversationResponse,
    pub messages: Vec<ConversationMessageResponse>,
    pub risk_audits: Vec<RiskAuditAdminDto>,
}

/// Admin-facing projection of a `post_conversation_risk_audit` row.
#[derive(Debug, Serialize)]
pub struct RiskAuditAdminDto {
    pub audit_id: u64,
    pub conversation_id: u64,
    pub audit_scope: String,
    pub status: String,
    pub risk_level: Option<String>,
    pub confidence: Option<f64>,
    pub detector_name: Option<String>,
    pub error_message: Option<String>,
    pub source_deleted: bool,
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
#[allow(dead_code)]
fn enum_str<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v)
        .unwrap_or_default()
        .trim_matches('"')
        .to_string()
}

// ── Handlers ──────────────────────────────────────────────────────────────────

pub async fn list_users(
    State(state): State<AdminState>,
    Query(q): Query<PageQuery>,
) -> Result<Json<PaginatedUsers>, AppError> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);

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
            role: u.role.as_str().to_string(),
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
    State(state): State<AdminState>,
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
        role: u.role.as_str().to_string(),
        created_at: u.created_at.to_rfc3339(),
        updated_at: u.updated_at.to_rfc3339(),
        last_login_at: u.last_login_at.map(|t| t.to_rfc3339()),
    }))
}

pub async fn patch_user(
    State(state): State<AdminState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(id): Path<u64>,
    Json(body): Json<AdminPatchUser>,
) -> Result<Json<UserDto>, AppError> {
    let status = body
        .status
        .map(|s| {
            UserStatus::from_i32(s)
                .ok_or_else(|| AppError::Validation(format!("invalid user status: {s}")))
        })
        .transpose()?;
    let role = body
        .role
        .map(|role| {
            UserRole::from_str(&role)
                .ok_or_else(|| AppError::Validation(format!("invalid user role: {role}")))
        })
        .transpose()?;
    if id == auth.user_id
        && (matches!(status, Some(UserStatus::Disabled)) || matches!(role, Some(UserRole::User)))
    {
        return Err(AppError::Validation(
            "administrators cannot disable or demote their own account".into(),
        ));
    }

    let update = UserUpdate {
        email: None,
        phone: None,
        nickname: None,
        status,
        role,
    };

    let u = state.user.admin_update_user(id, update).await?;

    Ok(Json(UserDto {
        id: u.id,
        username: u.username,
        email: u.email,
        phone: u.phone,
        nickname: u.nickname,
        status: u.status.as_str().to_string(),
        role: u.role.as_str().to_string(),
        created_at: u.created_at.to_rfc3339(),
        updated_at: u.updated_at.to_rfc3339(),
        last_login_at: u.last_login_at.map(|t| t.to_rfc3339()),
    }))
}

pub async fn delete_user(
    State(state): State<AdminState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(id): Path<u64>,
) -> Result<impl IntoResponse, AppError> {
    if id == auth.user_id {
        return Err(AppError::Validation(
            "administrators cannot delete their own account".into(),
        ));
    }
    state.user.admin_delete_user(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_risk_conversations(
    State(state): State<AdminState>,
    Query(q): Query<RiskConvQuery>,
) -> Result<Json<PaginatedRiskConversations>, AppError> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);

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
                title: c.title.clone(),
                is_title_generated: c.title.is_some(),
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
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> Result<Json<RiskConversationDetail>, AppError> {
    let conv = state
        .query
        .admin_get_conversation(id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("conversation {id} not found")))?;

    let messages = state.query.admin_get_conversation_messages(id).await?;

    let audits = state.query.admin_get_conversation_risk_audits(id).await?;

    Ok(Json(RiskConversationDetail {
        conversation: ConversationResponse {
            id: conv.id,
            user_id: conv.user_id,
            title: conv.title.clone(),
            is_title_generated: conv.title.is_some(),
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
        risk_audits: audits
            .into_iter()
            .map(|a| RiskAuditAdminDto {
                audit_id: a.audit_id,
                conversation_id: a.conversation_id,
                audit_scope: a.audit_scope,
                status: a.status,
                risk_level: a.risk_level,
                confidence: a.confidence,
                detector_name: a.detector_name,
                error_message: a.error_message,
                source_deleted: a.source_deleted,
                created_at: a.created_at.to_rfc3339(),
            })
            .collect(),
    }))
}

pub async fn process_risk_detection(
    State(_state): State<AdminState>,
    Extension(_auth): Extension<AuthenticatedUser>,
    Path(_id): Path<u64>,
    Json(_body): Json<ProcessNotes>,
) -> Result<StatusCode, AppError> {
    // Manual "mark processed" is no longer applicable: post-conversation risk
    // audits are completed by the PostConversationRiskAuditWorker, not by an
    // admin. This handler is retained only to keep the route mount valid until
    // the router is updated to drop it.
    Err(AppError::NotFound(
        "manual risk processing is not supported in the post-conversation audit model".into(),
    ))
}
