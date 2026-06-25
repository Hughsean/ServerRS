use axum::{
    Json,
    extract::{Extension, Path, Query, State},
};
use validator::Validate;

use crate::api::dto::chat_dto::*;
use crate::api::state::AppState;
use crate::app::auth::auth_service::AuthenticatedUser;
use crate::shared::error::AppError;

/// POST /api/v1/chat/open
pub async fn chat_open(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ChatOpenResponse>, AppError> {
    let conv = state.chat.chat.open(auth_user.user_id).await?;
    Ok(Json(ChatOpenResponse {
        conversation: ChatConversationInfo {
            id: conv.conversation.id,
            message_count: conv.conversation.message_count as u64,
            last_message_at: conv
                .conversation
                .last_message_at
                .map(|value| value.to_rfc3339()),
        },
        personalization_enabled: conv.personalization_enabled,
    }))
}

/// POST /api/v1/chat/messages
pub async fn chat_send_message(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Json(body): Json<ChatMessageRequest>,
) -> Result<Json<ChatMessageResponse>, AppError> {
    // 校验请求参数
    body.validate().map_err(AppError::validation)?;
    let result = state
        .chat
        .chat
        .send_message(auth_user.user_id, body.text, body.emotion, body.location)
        .await?;
    Ok(Json(ChatMessageResponse {
        conversation_id: result.conversation_id,
        reply: result.reply,
        tool_calls: result
            .tool_calls
            .into_iter()
            .map(|tool| ChatToolCallItem {
                name: tool.tool_name,
                arguments: tool.arguments,
            })
            .collect(),
    }))
}

/// GET /api/v1/chat/history
pub async fn chat_history(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Query(query): Query<ChatHistoryQuery>,
) -> Result<Json<ChatHistoryResponse>, AppError> {
    let limit = query.limit.unwrap_or(50);
    if limit == 0 || limit > 100 {
        return Err(AppError::Validation(
            "history limit must be between 1 and 100".into(),
        ));
    }
    let Some(conv) = state
        .chat
        .conv_repo
        .find_single_by_user_id(auth_user.user_id)
        .await?
    else {
        return Ok(Json(ChatHistoryResponse {
            messages: Vec::new(),
            next_before_id: None,
        }));
    };

    let messages = state
        .chat
        .conv_repo
        .find_messages_before(conv.id, query.before_id, limit)
        .await?;
    let next_before_id = (messages.len() == limit as usize)
        .then(|| messages.first().map(|message| message.id))
        .flatten();

    let items: Vec<ChatMessageItem> = messages
        .into_iter()
        .map(|m| ChatMessageItem {
            id: m.id,
            sender_role: m.sender_role,
            message_type: m.message_type,
            content: serde_json::from_str(&m.content)
                .unwrap_or_else(|_| serde_json::json!({ "text": m.content })),
            created_at: m.created_at.to_string(),
        })
        .collect();

    Ok(Json(ChatHistoryResponse {
        messages: items,
        next_before_id,
    }))
}

/// GET /api/v1/chat/memories
pub async fn chat_memories(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Query(query): Query<ChatMemoryQuery>,
) -> Result<Json<ChatMemoryResponse>, AppError> {
    let limit = query.limit.unwrap_or(50);
    if limit == 0 || limit > 100 {
        return Err(AppError::Validation(
            "memory limit must be between 1 and 100".into(),
        ));
    }
    let requested_types = query
        .memory_types
        .as_deref()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if requested_types
        .iter()
        .any(|memory_type| !crate::domain::memory::is_allowed_memory_type(memory_type))
    {
        return Err(AppError::Validation(
            "memory type filter contains an unsupported type".into(),
        ));
    }

    let mut memories = state
        .internal
        .memory
        .find_by_user_id(auth_user.user_id, Some(1))
        .await?;
    if !requested_types.is_empty() {
        memories.retain(|memory| requested_types.contains(&memory.memory_type));
    }
    let total_active = memories.len();
    memories.truncate(limit);

    let items: Vec<ChatMemoryItem> = memories
        .into_iter()
        .map(|m| ChatMemoryItem {
            memory_id: m.memory_id,
            memory_type: m.memory_type,
            content: m.content,
            confidence: m.confidence,
            reinforce_count: m.reinforce_count,
            created_at: m.created_at.to_string(),
            reinforced_at: m.reinforced_at.map(|value| value.to_rfc3339()),
        })
        .collect();

    Ok(Json(ChatMemoryResponse {
        memories: items,
        total_active,
    }))
}

/// GET /api/v1/chat/persona
///
pub async fn chat_persona(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ChatPersonaResponse>, AppError> {
    let persona = state.chat.chat.persona(auth_user.user_id).await?;
    Ok(Json(ChatPersonaResponse {
        has_active_persona: persona.has_active_persona,
        generated_at: persona.generated_at.map(|value| value.to_rfc3339()),
        snapshot_summary: ChatPersonaSnapshotSummary {
            communication_preferences_count: persona
                .snapshot_summary
                .communication_preferences_count,
            stable_facts_count: persona.snapshot_summary.stable_facts_count,
            recurring_topics_count: persona.snapshot_summary.recurring_topics_count,
            goals_count: persona.snapshot_summary.goals_count,
            sensitive_context_count: persona.snapshot_summary.sensitive_context_count,
        },
        personalization_enabled: persona.personalization_enabled,
    }))
}

/// POST /api/v1/chat/memory/{id}/disable
///
/// 在验证属于已认证用户后禁用该记忆
/// （防止 IDOR — 用户不能通过猜测 ID 来禁用其他用户的记忆）
/// 同步删除 Qdrant 索引。
pub async fn chat_disable_memory(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(memory_id): Path<u64>,
) -> Result<Json<DisableMemoryResponse>, AppError> {
    // MemoryService::disable verifies ownership (mem.user_id != user_id → 403)
    // and syncs vector index deletion.
    state
        .chat
        .chat
        .disable_memory(auth_user.user_id, memory_id)
        .await?;
    Ok(Json(DisableMemoryResponse {
        memory_id,
        disabled: true,
    }))
}

/// POST /api/v1/chat/persona/reset
///
pub async fn chat_persona_reset(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<PersonaResetResponse>, AppError> {
    let result = state
        .chat
        .chat
        .reset_persona(auth_user.user_id)
        .await?;
    Ok(Json(PersonaResetResponse {
        reset: result.reset,
    }))
}

/// POST /api/v1/chat/persona/rebuild
///
pub async fn chat_persona_rebuild(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<PersonaRebuildResponse>, AppError> {
    let result = state
        .chat
        .chat
        .rebuild_persona(auth_user.user_id)
        .await?;
    Ok(Json(PersonaRebuildResponse {
        snapshot_id: result.snapshot_id,
    }))
}

/// POST /api/v1/chat/transcript/clear
///
pub async fn chat_transcript_clear(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<TranscriptClearResponse>, AppError> {
    let result = state
        .chat
        .chat
        .clear_transcript(auth_user.user_id)
        .await?;
    Ok(Json(TranscriptClearResponse {
        cleared_messages: result.cleared_messages,
        cleared_summaries: result.cleared_summaries,
        memories_preserved: result.memories_preserved,
        persona_preserved: result.persona_preserved,
        post_risk_audits_cleared: result.post_risk_audits_cleared,
    }))
}

/// POST /api/v1/chat/forget
///
pub async fn chat_forget(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ForgetResponse>, AppError> {
    let result = state.chat.chat.forget(auth_user.user_id).await?;
    Ok(Json(ForgetResponse {
        messages_cleared: result.messages_cleared,
        summaries_cleared: result.summaries_cleared,
        memories_disabled: result.memories_disabled,
        persona_expired: result.persona_expired,
        post_risk_audits_deleted: result.post_risk_audits_deleted,
        personalization_disabled: result.personalization_disabled,
    }))
}
