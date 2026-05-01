use axum::{Extension, Json, extract::Path, extract::Query, extract::State};
use serde::Deserialize;
use validator::Validate;

use crate::api::ApiState;
use crate::api::dto::risk_dto::{RiskDetectionPage, RiskDetectionResponse};
use crate::api::dto::session_dto::{
    ConversationMessageResponse, ConversationResponse, MessageRequest, MessageResponse,
    SessionCreateRequest, SessionCreateResponse, SessionStatusResponse,
};
use crate::api::response::ApiResponse;
use crate::application::auth::auth_service::AuthenticatedUser;
use crate::shared::error::AppError;

pub async fn create_session(
    State(state): State<ApiState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Json(payload): Json<SessionCreateRequest>,
) -> Result<Json<ApiResponse<SessionCreateResponse>>, AppError> {
    payload.validate().map_err(AppError::validation)?;
    let sess = state
        .session
        .create(
            auth_user.user_id,
            payload.dialogue_id,
            payload.location.as_ref(),
        )
        .await?;
    Ok(Json(ApiResponse::ok(SessionCreateResponse {
        session_id: sess.id,
        prompt: sess.prompt,
        location: payload.location,
        user_profile: None,
        timeout_seconds: 120,
        dialogue_id: sess.dialogue_id,
    })))
}

pub async fn post_message(
    State(state): State<ApiState>,
    Path(session_id): Path<String>,
    Json(payload): Json<MessageRequest>,
) -> Result<Json<ApiResponse<MessageResponse>>, AppError> {
    payload.validate().map_err(AppError::validation)?;
    let result = state
        .session
        .process_message(&session_id, &payload.text, payload.emotion.as_deref())
        .await?
        .ok_or(AppError::NotFound("session not found or expired".into()))?;
    Ok(Json(ApiResponse::ok(MessageResponse {
        session_id,
        reply: result.reply,
        session_closed: result.session_closed,
        dialogue_id: result.dialogue_id,
        title: result.title,
    })))
}

pub async fn get_session_status(
    State(state): State<ApiState>,
    Path(session_id): Path<String>,
) -> Result<Json<ApiResponse<SessionStatusResponse>>, AppError> {
    let status = state
        .session
        .status(&session_id)
        .await
        .ok_or(AppError::NotFound("session not found or expired".into()))?;
    Ok(Json(ApiResponse::ok(SessionStatusResponse {
        session_id: status["sessionId"].as_str().unwrap_or("").into(),
        user_id: status["userId"].as_u64().unwrap_or(0),
        dialogue_id: status["dialogueId"].as_u64(),
        timeout_seconds: status["timeoutSeconds"].as_u64().unwrap_or(120),
    })))
}

pub async fn list_conversations(
    State(state): State<ApiState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(user_id): Path<u64>,
) -> Result<Json<ApiResponse<Vec<ConversationResponse>>>, AppError> {
    if auth_user.user_id != user_id {
        return Err(AppError::Forbidden("not your conversations".into()));
    }
    let convs = state.query.list_conversations(user_id).await?;
    Ok(Json(ApiResponse::ok(
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
    )))
}

pub async fn list_conversation_messages(
    State(state): State<ApiState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path((_user_id, conv_id)): Path<(u64, u64)>,
) -> Result<Json<ApiResponse<Vec<ConversationMessageResponse>>>, AppError> {
    let msgs = state
        .query
        .list_messages(conv_id, auth_user.user_id)
        .await?;
    Ok(Json(ApiResponse::ok(
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
    )))
}

#[derive(Debug, Deserialize)]
pub struct RiskListQuery {
    pub page: Option<u64>,
    pub size: Option<u64>,
}

pub async fn list_risk_detections(
    State(state): State<ApiState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Query(query): Query<RiskListQuery>,
) -> Result<Json<ApiResponse<RiskDetectionPage>>, AppError> {
    let page = query.page.unwrap_or(1).max(1);
    let size = query.size.unwrap_or(10).min(100);
    let (items, total) = state
        .query
        .list_risk_detections(auth_user.user_id, page, size)
        .await?;
    Ok(Json(ApiResponse::ok(RiskDetectionPage {
        items: items
            .into_iter()
            .map(|r| RiskDetectionResponse {
                id: r.id,
                conversation_id: r.conversation_id,
                risk_level: r.risk_level,
                polarity: r.polarity,
                intent: r.intent,
                reason: r.reason,
                confidence: r.confidence,
                created_at: r.created_at.to_rfc3339(),
            })
            .collect(),
        total,
        page,
        size,
    })))
}
