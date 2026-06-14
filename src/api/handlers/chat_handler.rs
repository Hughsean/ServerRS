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
pub async fn chat_persona(
    State(state): State<AppState>,
    Extension(_auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ApiResponse<ChatPersonaResponse>>, AppError> {
    // Persona snapshot retrieval — placeholder returning None until PersonaEngine is implemented
    Ok(Json(ApiResponse::ok(ChatPersonaResponse {
        snapshot_id: None,
        snapshot_data: None,
    })))
}

/// POST /api/v1/chat/memory/{id}/disable
pub async fn chat_disable_memory(
    State(state): State<AppState>,
    Extension(_auth_user): Extension<AuthenticatedUser>,
    Path(memory_id): Path<u64>,
) -> Result<Json<ApiResponse<DisableMemoryResponse>>, AppError> {
    state.internal.memory.disable_memory(memory_id).await?;
    Ok(Json(ApiResponse::ok(DisableMemoryResponse {
        memory_id,
        disabled: true,
    })))
}

/// POST /api/v1/chat/persona/reset
pub async fn chat_persona_reset(
    State(_state): State<AppState>,
    Extension(_auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ApiResponse<PersonaResetResponse>>, AppError> {
    // Placeholder — will bump context_version and expire persona
    Ok(Json(ApiResponse::ok(PersonaResetResponse { reset: true })))
}

/// POST /api/v1/chat/persona/rebuild
pub async fn chat_persona_rebuild(
    State(_state): State<AppState>,
    Extension(_auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ApiResponse<PersonaRebuildResponse>>, AppError> {
    // Placeholder — will rebuild persona snapshot
    Ok(Json(ApiResponse::ok(PersonaRebuildResponse {
        snapshot_id: None,
    })))
}

/// POST /api/v1/chat/transcript/clear
pub async fn chat_transcript_clear(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ApiResponse<TranscriptClearResponse>>, AppError> {
    state
        .chat
        .chat_service
        .clear_transcript(auth_user.user_id)
        .await?;
    Ok(Json(ApiResponse::ok(TranscriptClearResponse {
        cleared: true,
    })))
}

/// POST /api/v1/chat/forget
pub async fn chat_forget(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ApiResponse<ForgetResponse>>, AppError> {
    let lock = state.chat.chat_service.lock(auth_user.user_id);
    let _guard = lock.lock().await;

    // Delete all messages for this user's conversation
    if let Ok(Some(conv)) = state
        .chat
        .conv_repo
        .find_single_by_user_id(auth_user.user_id)
        .await
    {
        let _ = state
            .chat
            .conv_repo
            .delete_messages_by_conversation_id(conv.id)
            .await;
    }

    Ok(Json(ApiResponse::ok(ForgetResponse { forgotten: true })))
}
