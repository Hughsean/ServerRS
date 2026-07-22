use axum::{
    Json,
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use validator::Validate;

use crate::api::dto::chat_dto::*;
use crate::api::error::ApiError as AppError;
use crate::api::state::{ChatState, InternalState};
use crate::app::agent::chat_state::{ChatSuspendData, ToolApprovalDecision};
use crate::app::agent::graph::{CheckpointId, SuspendReason};
use crate::app::auth::auth_service::AuthenticatedUser;
use crate::app::session::chat_service::{ChatTurnOutcome, ChatTurnResponse};
use crate::domain::agent::PendingChatApproval;

/// POST /api/v1/chat/open
pub async fn chat_open(
    State(state): State<ChatState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ChatOpenResponse>, AppError> {
    let conv = state.chat.open(auth_user.user_id).await?;
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
    State(state): State<ChatState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Json(body): Json<ChatMessageRequest>,
) -> Result<Response, AppError> {
    // 校验请求参数
    body.validate().map_err(AppError::validation)?;
    let result = state
        .chat
        .send_message_checkpointed(auth_user.user_id, body.text, body.emotion, body.location)
        .await?;
    Ok(chat_turn_response(result))
}

/// POST /api/v1/chat/checkpoints/{checkpoint_id}/resume
pub async fn chat_resume_checkpoint(
    State(state): State<ChatState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(checkpoint_id): Path<String>,
    Json(body): Json<ChatCheckpointResumeRequest>,
) -> Result<Response, AppError> {
    body.validate().map_err(AppError::validation)?;
    let checkpoint_id = checkpoint_id
        .parse::<CheckpointId>()
        .map_err(|_| AppError::Validation("checkpoint_id must be a UUID".into()))?;
    let approval_id = uuid::Uuid::parse_str(&body.approval_id)
        .map_err(|_| AppError::Validation("approval_id must be a UUID".into()))?;
    let decision = match body.decision {
        ChatApprovalDecisionRequest::Approve => ToolApprovalDecision::Approve,
        ChatApprovalDecisionRequest::Reject => ToolApprovalDecision::Reject,
    };
    let result = state
        .chat
        .resume_checkpoint(auth_user.user_id, checkpoint_id, approval_id, decision)
        .await?;
    Ok(chat_turn_response(result))
}

/// GET /api/v1/chat/checkpoints/pending
///
/// 当前用户的待审批列表（非消费式查询）。只返回 pending 且未过期、
/// 属于当前用户的 Checkpoint；不包含完整 payload 或消息历史。
pub async fn chat_list_pending_approvals(
    State(state): State<ChatState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Query(query): Query<PendingApprovalsQuery>,
) -> Result<Json<PendingChatApprovalListResponse>, AppError> {
    let page = state
        .chat
        .list_pending_approvals(auth_user.user_id, query.conversation_id, query.limit)
        .await?;
    Ok(Json(PendingChatApprovalListResponse {
        items: page.items.into_iter().map(pending_approval_item).collect(),
    }))
}

/// GET /api/v1/chat/checkpoints/{checkpoint_id}
///
/// 当前用户的单个待审批详情（非消费式查询）。其他用户、已过期、已消费
/// 或不存在的 Checkpoint 统一返回 404，避免 ID 枚举。
pub async fn chat_get_checkpoint(
    State(state): State<ChatState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(checkpoint_id): Path<String>,
) -> Result<Json<PendingChatApprovalItem>, AppError> {
    let checkpoint_id = checkpoint_id
        .parse::<CheckpointId>()
        .map_err(|_| AppError::NotFound("Checkpoint 不存在或已失效".into()))?;
    let pending = state
        .chat
        .get_pending_approval(auth_user.user_id, checkpoint_id)
        .await?;
    Ok(Json(pending_approval_item(pending)))
}

fn pending_approval_item(pending: PendingChatApproval) -> PendingChatApprovalItem {
    PendingChatApprovalItem {
        status: "pending",
        checkpoint_id: pending.checkpoint_id.to_string(),
        run_id: pending.run_id.to_string(),
        conversation_id: pending.conversation_id,
        reason: suspend_reason_name(pending.reason),
        created_at: pending.created_at.to_rfc3339(),
        expires_at: pending.expires_at.to_rfc3339(),
        approval: ChatToolApprovalInfo {
            approval_id: pending.approval.approval_id.to_string(),
            prompt: pending.approval.prompt,
            tool_calls: pending
                .approval
                .tool_calls
                .into_iter()
                .map(|tool| ChatApprovalToolCallItem {
                    id: tool.id,
                    name: tool.name,
                    arguments: tool.arguments,
                })
                .collect(),
        },
    }
}

fn chat_turn_response(outcome: ChatTurnOutcome) -> Response {
    match outcome {
        ChatTurnOutcome::Completed(response) => {
            (StatusCode::OK, Json(completed_response(response))).into_response()
        }
        ChatTurnOutcome::Suspended(suspension) => {
            let ChatSuspendData::ToolApproval(approval) = suspension.data;
            (
                StatusCode::ACCEPTED,
                Json(ChatSuspendedResponse {
                    status: "suspended",
                    conversation_id: suspension.conversation_id,
                    checkpoint_id: suspension.checkpoint_id.to_string(),
                    run_id: suspension.run_id.to_string(),
                    reason: suspend_reason_name(suspension.reason),
                    approval: ChatToolApprovalInfo {
                        approval_id: approval.approval_id.to_string(),
                        prompt: approval.prompt,
                        tool_calls: approval
                            .tools
                            .into_iter()
                            .map(|tool| ChatApprovalToolCallItem {
                                id: tool.id,
                                name: tool.name,
                                arguments: tool.arguments,
                            })
                            .collect(),
                    },
                }),
            )
                .into_response()
        }
    }
}

fn completed_response(result: ChatTurnResponse) -> ChatMessageResponse {
    ChatMessageResponse {
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
    }
}

fn suspend_reason_name(reason: SuspendReason) -> &'static str {
    match reason {
        SuspendReason::ExternalInput => "external_input",
        SuspendReason::Approval => "approval",
        SuspendReason::ExternalEvent => "external_event",
        SuspendReason::Business => "business",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agent::agent_runtime::{AgentSuspension, ToolTrace};
    use crate::app::agent::chat_state::{ApprovalToolCall, ChatSuspendData, ToolApprovalRequest};
    use crate::app::agent::graph::RunId;
    use crate::domain::agent::{ChatApprovalPreview, ChatApprovalToolCallPreview};

    #[tokio::test]
    async fn completed_chat_keeps_the_existing_json_shape() {
        let response = chat_turn_response(ChatTurnOutcome::Completed(ChatTurnResponse {
            reply: "done".into(),
            conversation_id: 9,
            run_id: RunId::new(),
            tool_calls: vec![ToolTrace {
                tool_name: "clock".into(),
                arguments: serde_json::json!({"zone": "Asia/Shanghai"}),
                result: "12:00".into(),
            }],
        }));

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "conversation_id": 9,
                "reply": "done",
                "tool_calls": [{
                    "name": "clock",
                    "arguments": {"zone": "Asia/Shanghai"}
                }]
            })
        );
    }

    #[tokio::test]
    async fn suspended_chat_returns_accepted_with_resume_identifiers() {
        let checkpoint_id = CheckpointId::new();
        let run_id = RunId::new();
        let approval_id = uuid::Uuid::new_v4();
        let response = chat_turn_response(ChatTurnOutcome::Suspended(AgentSuspension {
            checkpoint_id,
            run_id,
            conversation_id: 9,
            reason: SuspendReason::Approval,
            data: ChatSuspendData::ToolApproval(ToolApprovalRequest {
                approval_id,
                prompt: "approve".into(),
                tools: vec![ApprovalToolCall {
                    id: "call-1".into(),
                    name: "controlled_tool".into(),
                    arguments: serde_json::json!({"value": 7}),
                }],
            }),
        }));

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "suspended");
        assert_eq!(json["checkpoint_id"], checkpoint_id.to_string());
        assert_eq!(json["run_id"], run_id.to_string());
        assert_eq!(json["approval"]["approval_id"], approval_id.to_string());
        assert_eq!(json["approval"]["tool_calls"][0]["name"], "controlled_tool");
    }

    fn pending() -> PendingChatApproval {
        PendingChatApproval {
            checkpoint_id: CheckpointId::new(),
            run_id: RunId::new(),
            conversation_id: 9,
            reason: SuspendReason::Approval,
            created_at: chrono::DateTime::parse_from_rfc3339("2026-07-22T01:00:00Z")
                .unwrap()
                .to_utc(),
            expires_at: chrono::DateTime::parse_from_rfc3339("2026-07-23T01:00:00Z")
                .unwrap()
                .to_utc(),
            approval: ChatApprovalPreview {
                approval_id: uuid::Uuid::parse_str("02f941ab-0fb8-4c44-999c-9ff896ef415a").unwrap(),
                prompt: "模型请求执行受控工具，请确认是否允许。".into(),
                tool_calls: vec![ChatApprovalToolCallPreview {
                    id: "call-1".into(),
                    name: "fetch_web_content".into(),
                    arguments: serde_json::json!({"url": "https://example.com"}),
                }],
            },
        }
    }

    #[test]
    fn pending_approval_item_exposes_only_safe_fields() {
        let item = pending_approval_item(pending());
        let json = serde_json::to_value(&item).unwrap();

        assert_eq!(json["status"], "pending");
        assert_eq!(json["conversation_id"], 9);
        assert_eq!(json["reason"], "approval");
        assert_eq!(json["created_at"], "2026-07-22T01:00:00+00:00");
        assert_eq!(json["expires_at"], "2026-07-23T01:00:00+00:00");
        assert_eq!(
            json["approval"]["approval_id"],
            "02f941ab-0fb8-4c44-999c-9ff896ef415a"
        );
        assert_eq!(
            json["approval"]["tool_calls"][0]["arguments"],
            serde_json::json!({"url": "https://example.com"})
        );
        // 安全边界：不得出现完整 Checkpoint payload、消息历史或内部 Trace 字段
        for forbidden in [
            "payload",
            "state",
            "messages",
            "recent_messages",
            "memories",
            "user_profile",
            "trace",
            "effect_receipts",
            "visited",
        ] {
            assert!(json.get(forbidden).is_none(), "响应不得包含 {forbidden}");
        }
    }

    #[test]
    fn pending_approval_list_wraps_items() {
        let response = PendingChatApprovalListResponse {
            items: vec![pending_approval_item(pending())],
        };
        let json = serde_json::to_value(&response).unwrap();

        assert_eq!(json["items"].as_array().unwrap().len(), 1);
        assert_eq!(json["items"][0]["status"], "pending");
    }
}

/// GET /api/v1/chat/history
pub async fn chat_history(
    State(state): State<ChatState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Query(query): Query<ChatHistoryQuery>,
) -> Result<Json<ChatHistoryResponse>, AppError> {
    let limit = query.limit.unwrap_or(50);
    if limit == 0 || limit > 100 {
        return Err(AppError::Validation(
            "history limit must be between 1 and 100".into(),
        ));
    }
    let page = state
        .history
        .history(auth_user.user_id, query.before_id, limit)
        .await?;

    let items: Vec<ChatMessageItem> = page
        .messages
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
        next_before_id: page.next_before_id,
    }))
}

/// GET /api/v1/chat/memories
pub async fn chat_memories(
    State(state): State<InternalState>,
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

    let memories = state
        .memory
        .find_by_user_id_filtered(auth_user.user_id, Some(1), &requested_types, limit)
        .await?;
    let total_active = memories.len();

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
    State(state): State<ChatState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ChatPersonaResponse>, AppError> {
    let persona = state.chat.persona(auth_user.user_id).await?;
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
    State(state): State<ChatState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(memory_id): Path<u64>,
) -> Result<Json<DisableMemoryResponse>, AppError> {
    // MemoryService::disable verifies ownership (mem.user_id != user_id → 403)
    // and syncs vector index deletion.
    state
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
    State(state): State<ChatState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<PersonaResetResponse>, AppError> {
    let result = state.chat.reset_persona(auth_user.user_id).await?;
    Ok(Json(PersonaResetResponse {
        reset: result.reset,
    }))
}

/// POST /api/v1/chat/persona/rebuild
///
pub async fn chat_persona_rebuild(
    State(state): State<ChatState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<PersonaRebuildResponse>, AppError> {
    let result = state.chat.rebuild_persona(auth_user.user_id).await?;
    Ok(Json(PersonaRebuildResponse {
        snapshot_id: result.snapshot_id,
    }))
}

/// POST /api/v1/chat/transcript/clear
///
pub async fn chat_transcript_clear(
    State(state): State<ChatState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<TranscriptClearResponse>, AppError> {
    let result = state.chat.clear_transcript(auth_user.user_id).await?;
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
    State(state): State<ChatState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
) -> Result<Json<ForgetResponse>, AppError> {
    let result = state.chat.forget(auth_user.user_id).await?;
    Ok(Json(ForgetResponse {
        messages_cleared: result.messages_cleared,
        summaries_cleared: result.summaries_cleared,
        memories_disabled: result.memories_disabled,
        persona_expired: result.persona_expired,
        post_risk_audits_deleted: result.post_risk_audits_deleted,
        personalization_disabled: result.personalization_disabled,
    }))
}
