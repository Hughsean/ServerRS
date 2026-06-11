use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use validator::Validate;

use crate::api::SessionState;
use crate::api::dto::risk_dto::{RiskDetectionPage, RiskDetectionResponse};
use crate::api::dto::session_dto::{
    ConversationMessageResponse, ConversationResponse, MessageRequest, MessageResponse,
    SessionCreateRequest, SessionCreateResponse, SessionStatusResponse,
};
use crate::application::auth::auth_service::AuthenticatedUser;
use crate::shared::error::AppError;

pub async fn create_session(
    State(state): State<SessionState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Json(payload): Json<SessionCreateRequest>,
) -> Result<Json<SessionCreateResponse>, AppError> {
    payload.validate().map_err(AppError::validation)?;
    if payload.user_id != 0 && payload.user_id != auth_user.user_id {
        return Err(AppError::Forbidden("not your session user".into()));
    }
    let sess = state
        .session
        .create(
            auth_user.user_id,
            payload.dialogue_id,
            payload.location.as_ref(),
        )
        .await?;
    Ok(Json(SessionCreateResponse {
        session_id: sess.id,
        prompt: sess.prompt,
        location: payload.location,
        user_profile: None,
        timeout_seconds: state.session.timeout_seconds(),
        dialogue_id: sess.dialogue_id,
    }))
}

pub async fn post_message(
    State(state): State<SessionState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(session_id): Path<String>,
    Json(payload): Json<MessageRequest>,
) -> Result<Json<MessageResponse>, AppError> {
    payload.validate().map_err(AppError::validation)?;
    let result = state
        .session
        .process_message(
            auth_user.user_id,
            &session_id,
            &payload.text,
            payload.emotion.as_deref(),
        )
        .await?
        .ok_or(AppError::NotFound("session not found or expired".into()))?;
    Ok(Json(MessageResponse {
        session_id,
        reply: result.reply,
        session_closed: result.session_closed,
        dialogue_id: result.dialogue_id,
        title: result.title,
    }))
}

pub async fn get_session_status(
    State(state): State<SessionState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionStatusResponse>, AppError> {
    let status = state
        .session
        .status(auth_user.user_id, &session_id)
        .await?
        .ok_or(AppError::NotFound("session not found or expired".into()))?;
    Ok(Json(SessionStatusResponse {
        session_id: status.id,
        user_id: status.user_id,
        dialogue_id: status.dialogue_id,
        timeout_seconds: status.timeout_seconds,
    }))
}

pub async fn list_conversations(
    State(state): State<SessionState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(user_id): Path<u64>,
) -> Result<Json<Vec<ConversationResponse>>, AppError> {
    if auth_user.user_id != user_id {
        return Err(AppError::Forbidden("not your conversations".into()));
    }
    let convs = state.query.list_conversations(user_id).await?;
    Ok(Json(
        convs
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
    ))
}

pub async fn list_conversation_messages(
    State(state): State<SessionState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path((user_id, conv_id)): Path<(u64, u64)>,
) -> Result<Json<Vec<ConversationMessageResponse>>, AppError> {
    if auth_user.user_id != user_id {
        return Err(AppError::Forbidden("not your conversations".into()));
    }
    let msgs = state
        .query
        .list_messages(conv_id, auth_user.user_id)
        .await?;
    Ok(Json(
        msgs.into_iter()
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
    ))
}

#[derive(Debug, Deserialize)]
pub struct RiskListQuery {
    pub page: Option<u64>,
    pub size: Option<u64>,
}

pub async fn list_risk_detections(
    State(state): State<SessionState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Query(query): Query<RiskListQuery>,
) -> Result<Json<RiskDetectionPage>, AppError> {
    let page = query.page.unwrap_or(1).max(1);
    let size = query.size.unwrap_or(10).min(100);
    let (items, total) = state
        .query
        .list_risk_detections(auth_user.user_id, page, size)
        .await?;
    Ok(Json(RiskDetectionPage {
        items: items
            .into_iter()
            .map(|r| RiskDetectionResponse {
                id: r.id,
                conversation_id: r.conversation_id,
                risk_level: serde_json::to_string(&r.risk_level)
                    .unwrap_or_default()
                    .trim_matches('"')
                    .to_string(),
                polarity: serde_json::to_string(&r.polarity)
                    .unwrap_or_default()
                    .trim_matches('"')
                    .to_string(),
                intent: serde_json::to_string(&r.intent)
                    .unwrap_or_default()
                    .trim_matches('"')
                    .to_string(),
                reason: r.reason,
                confidence: r.confidence,
                created_at: r.created_at.to_rfc3339(),
            })
            .collect(),
        total,
        page,
        size,
    }))
}
