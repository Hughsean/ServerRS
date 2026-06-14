use axum::{
    Json,
    extract::{Extension, Path, State},
    http::StatusCode,
};

use crate::api::dto::chat_dto::*;
use crate::api::response::ApiResponse;
use crate::api::state::AppState;
use crate::application::auth::auth_service::AuthenticatedUser;
use crate::shared::error::AppError;

/// POST /api/v1/chat/open
pub async fn chat_open(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ApiResponse<ChatOpenResponse>>, AppError> {
    let conv = state.chat.chat_service.open(auth_user.user_id).await?;
    Ok(Json(ApiResponse::ok(ChatOpenResponse {
        conversation_id: conv.id,
        message_count: conv.message_count as u64,
        title: conv.title,
        created_at: conv.created_at.to_string(),
    })))
}

/// POST /api/v1/chat/messages
pub async fn chat_send_message(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Json(body): Json<ChatMessageRequest>,
) -> Result<Json<ApiResponse<ChatMessageResponse>>, AppError> {
    let result = state
        .chat
        .chat_service
        .send_message(auth_user.user_id, body.text, body.emotion, body.location)
        .await?;
    Ok(Json(ApiResponse::ok(ChatMessageResponse {
        conversation_id: result.conversation_id,
        reply: result.reply,
    })))
}

/// GET /api/v1/chat/history
pub async fn chat_history(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ApiResponse<ChatHistoryResponse>>, AppError> {
    let conv = state
        .chat
        .conv_repo
        .find_single_by_user_id(auth_user.user_id)
        .await?
        .ok_or_else(|| AppError::NotFound("no conversation found".into()))?;

    let messages = state
        .chat
        .conv_repo
        .find_messages_by_conversation_id(conv.id)
        .await?;

    let items: Vec<ChatMessageItem> = messages
        .into_iter()
        .map(|m| ChatMessageItem {
            id: m.id,
            sender_role: m.sender_role,
            content: m.content,
            created_at: m.created_at.to_string(),
        })
        .collect();

    Ok(Json(ApiResponse::ok(ChatHistoryResponse {
        conversation_id: conv.id,
        messages: items,
    })))
}

/// GET /api/v1/chat/memories
pub async fn chat_memories(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ApiResponse<ChatMemoryResponse>>, AppError> {
    let memories = state
        .internal
        .memory
        .find_by_user_id(auth_user.user_id, Some(1))
        .await
        .unwrap_or_default();

    let items: Vec<ChatMemoryItem> = memories
        .into_iter()
        .map(|m| ChatMemoryItem {
            memory_id: m.memory_id,
            memory_type: m.memory_type,
            content: m.content,
            confidence: m.confidence,
            created_at: m.created_at.to_string(),
        })
        .collect();

    Ok(Json(ApiResponse::ok(ChatMemoryResponse {
        memories: items,
    })))
}

/// GET /api/v1/chat/persona
///
/// Not yet implemented — PersonaEngine and snapshot retrieval will be added in
/// a later phase. Returns 501 rather than a fake-success empty body.
pub async fn chat_persona(
    State(_state): State<AppState>,
    Extension(_auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ApiResponse<ChatPersonaResponse>>, AppError> {
    Err(AppError::NotImplemented(
        "persona view is not yet implemented".into(),
    ))
}

/// POST /api/v1/chat/memory/{id}/disable
///
/// Disables a memory after verifying it belongs to the authenticated user
/// (prevents IDOR — users cannot disable other users' memories by guessing
/// ids). Also syncs the Qdrant index deletion.
pub async fn chat_disable_memory(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(memory_id): Path<u64>,
) -> Result<Json<ApiResponse<DisableMemoryResponse>>, AppError> {
    // MemoryService::disable verifies ownership (mem.user_id != user_id → 403)
    // and syncs vector index deletion.
    state
        .internal
        .memory
        .disable(memory_id, auth_user.user_id)
        .await?;
    Ok(Json(ApiResponse::ok(DisableMemoryResponse {
        memory_id,
        disabled: true,
    })))
}

/// POST /api/v1/chat/persona/reset
///
/// Not yet implemented — full semantics (expire active persona snapshot,
/// bump context_version, disable personalization) will be added in phase 4.
pub async fn chat_persona_reset(
    State(_state): State<AppState>,
    Extension(_auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ApiResponse<PersonaResetResponse>>, AppError> {
    Err(AppError::NotImplemented(
        "persona reset is not yet implemented".into(),
    ))
}

/// POST /api/v1/chat/persona/rebuild
///
/// Not yet implemented — PersonaEngine rebuild will be added in a later phase.
pub async fn chat_persona_rebuild(
    State(_state): State<AppState>,
    Extension(_auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ApiResponse<PersonaRebuildResponse>>, AppError> {
    Err(AppError::NotImplemented(
        "persona rebuild is not yet implemented".into(),
    ))
}

/// POST /api/v1/chat/transcript/clear
///
/// Not yet implemented — full semantics (delete messages + summaries +
/// post-risk-audits, null evidence FKs, reset conversation counters, enqueue
/// vector delete jobs, bump context_version) will be added in phase 4.
/// The current `clear_transcript` only deletes messages, which is incomplete.
pub async fn chat_transcript_clear(
    State(_state): State<AppState>,
    Extension(_auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ApiResponse<TranscriptClearResponse>>, AppError> {
    Err(AppError::NotImplemented(
        "transcript clear is not yet implemented".into(),
    ))
}

/// POST /api/v1/chat/forget
///
/// Not yet implemented — full semantics (transcript/clear + disable all
/// memories + expire persona + delete post-risk-audits + disable
/// personalization + enqueue vector deletes + bump context_version) will be
/// added in phase 4.
pub async fn chat_forget(
    State(_state): State<AppState>,
    Extension(_auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ApiResponse<ForgetResponse>>, AppError> {
    Err(AppError::NotImplemented(
        "forget is not yet implemented".into(),
    ))
}
