//! 后端 API 响应的反序列化结构体。
//!
//! 字段名严格匹配后端 serde 默认的 snake_case 序列化
//! (auth/chat/user_dto 均未设 rename_all)。
//!
//! 字段需完整反映后端响应,即使当前未读取(反序列化需要),
//! 故整个模块允许 dead_code。

#![allow(dead_code)]

use serde::Deserialize;

// ── Auth ──
//
// 注意:后端 auth_handler.rs 重新定义了 LoginResponse/RefreshResponse
// (非 auth_dto.rs 的版本),且带 #[serde(rename_all = "camelCase")],
// 故实际 JSON 字段为 camelCase: accessToken / refreshToken / expiresIn / tokenType。
// /auth/me 返回 UserInfo {id, username, role}(无 rename_all,字段名原样)。

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub expires_in: u64,
    #[serde(default)]
    pub token_type: String,
    pub user: LoginUser,
}

#[derive(Debug, Deserialize)]
pub struct LoginUser {
    pub id: u64,
    pub username: String,
    #[serde(default)]
    pub role: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshResponse {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub expires_in: u64,
}

#[derive(Debug, Deserialize)]
pub struct AuthUser {
    pub id: u64,
    pub username: String,
    // 后端 UserInfo 其余字段(role)按需忽略,只取必要部分
}

// ── Chat ──

#[derive(Debug, Deserialize)]
pub struct ChatOpenResponse {
    pub conversation: ChatConversationInfo,
    pub personalization_enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct ChatConversationInfo {
    pub id: u64,
    pub message_count: u64,
    pub last_message_at: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ChatMessageRequest {
    pub text: String,
}

/// CLI 普通对话使用的语音请求，采用服务端支持的稳定默认音频规格。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageWithAudioRequest {
    pub text: String,
    pub format: &'static str,
    pub sample_rate: u32,
    pub channels: u8,
    pub sample_bits: u8,
}

impl ChatMessageWithAudioRequest {
    pub fn with_default_audio(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            format: "wav",
            sample_rate: 24_000,
            channels: 1,
            sample_bits: 16,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ChatMessageResponse {
    pub conversation_id: u64,
    pub reply: String,
    #[serde(default)]
    pub tool_calls: Vec<ChatToolCallItem>,
}

/// 语音对话完成时返回的音频描述。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAudioResponse {
    pub audio_url: String,
    pub format: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub sample_bits: u8,
}

/// 语音对话的完成响应；暂停响应仍沿用 `ChatSuspendedResponse`。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageWithAudioResponse {
    pub conversation_id: u64,
    pub reply: String,
    #[serde(default)]
    pub tool_calls: Vec<ChatToolCallItem>,
    pub audio: ChatAudioResponse,
}

#[derive(Debug, Deserialize)]
pub struct ChatToolCallItem {
    pub name: String,
    pub arguments: serde_json::Value,
}

// ── 工具审批（202 suspended 与待审批收件箱）──

/// 审批提示信息，与后端 `ChatToolApprovalInfo` 对应。
#[derive(Debug, Clone, Deserialize)]
pub struct ChatToolApprovalInfo {
    pub approval_id: String,
    pub prompt: String,
    #[serde(default)]
    pub tool_calls: Vec<ChatApprovalToolCallItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatApprovalToolCallItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

/// `202 Accepted` 的暂停响应。
#[derive(Debug, Clone, Deserialize)]
pub struct ChatSuspendedResponse {
    pub status: String,
    pub conversation_id: u64,
    pub checkpoint_id: String,
    pub run_id: String,
    pub reason: String,
    pub approval: ChatToolApprovalInfo,
}

/// 聊天联合响应：正常完成（200）或工具审批暂停（202）。
///
/// untagged 区分是安全的：Suspended 必须有 `approval`，Completed 必须有
/// `reply`，两个形状互不包含。
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ChatTurnResponse {
    Suspended(ChatSuspendedResponse),
    Completed(ChatMessageResponse),
}

/// 带语音聊天的联合响应。暂停时不包含音频，沿用审批响应。
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ChatTurnWithAudioResponse {
    Suspended(ChatSuspendedResponse),
    Completed(ChatMessageWithAudioResponse),
}

/// 待审批列表项，与后端 `PendingChatApprovalItem` 对应。
#[derive(Debug, Clone, Deserialize)]
pub struct PendingApprovalItem {
    pub status: String,
    pub checkpoint_id: String,
    pub run_id: String,
    pub conversation_id: u64,
    pub reason: String,
    pub created_at: String,
    pub expires_at: String,
    pub approval: ChatToolApprovalInfo,
}

#[derive(Debug, Deserialize)]
pub struct PendingApprovalsResponse {
    pub items: Vec<PendingApprovalItem>,
}

/// POST /checkpoints/{id}/resume 请求体。
#[derive(Debug, serde::Serialize)]
pub struct ChatResumeRequest {
    pub approval_id: String,
    pub decision: &'static str,
}

#[derive(Debug, Deserialize)]
pub struct ChatHistoryResponse {
    pub messages: Vec<ChatMessageItem>,
    pub next_before_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ChatMessageItem {
    pub id: u64,
    pub sender_role: String,
    pub message_type: String,
    pub content: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct ChatMemoryResponse {
    pub memories: Vec<ChatMemoryItem>,
    pub total_active: usize,
}

#[derive(Debug, Deserialize)]
pub struct ChatMemoryItem {
    pub memory_id: u64,
    pub memory_type: String,
    pub content: String,
    pub confidence: f64,
    pub reinforce_count: u32,
    pub created_at: String,
    #[serde(default)]
    pub reinforced_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChatPersonaResponse {
    pub has_active_persona: bool,
    pub generated_at: Option<String>,
    pub snapshot_summary: ChatPersonaSnapshotSummary,
    pub personalization_enabled: bool,
}

#[derive(Debug, Deserialize, Default)]
pub struct ChatPersonaSnapshotSummary {
    #[serde(default)]
    pub communication_preferences_count: usize,
    #[serde(default)]
    pub stable_facts_count: usize,
    #[serde(default)]
    pub recurring_topics_count: usize,
    #[serde(default)]
    pub goals_count: usize,
    #[serde(default)]
    pub sensitive_context_count: usize,
}

#[derive(Debug, Deserialize)]
pub struct PersonaRebuildResponse {
    pub snapshot_id: u64,
}

#[derive(Debug, Deserialize)]
pub struct PersonaResetResponse {
    pub reset: bool,
}

#[derive(Debug, Deserialize)]
pub struct TranscriptClearResponse {
    pub cleared_messages: bool,
    pub cleared_summaries: bool,
    pub memories_preserved: bool,
    pub persona_preserved: bool,
    pub post_risk_audits_cleared: bool,
}

#[derive(Debug, Deserialize)]
pub struct ForgetResponse {
    pub messages_cleared: bool,
    pub summaries_cleared: bool,
    pub memories_disabled: u64,
    pub persona_expired: bool,
    pub post_risk_audits_deleted: bool,
    pub personalization_disabled: bool,
}

// ── User Profile ──

#[derive(Debug, Deserialize)]
pub struct UserProfileResponse {
    pub id: u64,
    pub user_id: u64,
    #[serde(default)]
    pub interests: Option<Vec<String>>,
    #[serde(default)]
    pub personality_traits: Option<Vec<String>>,
    #[serde(default)]
    pub interaction_preferences: Option<Vec<String>>,
    #[serde(default)]
    pub emotional_tendency: Option<Vec<String>>,
    #[serde(default)]
    pub learning_records: Option<Vec<String>>,
    pub created_at: String,
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_response_deserializes() {
        // 后端实际返回 camelCase + 嵌套 user 对象
        let json = r#"{"accessToken":"a","refreshToken":"r","expiresIn":86400,"tokenType":"Bearer","user":{"id":1,"username":"alice","role":"user"}}"#;
        let r: LoginResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.access_token, "a");
        assert_eq!(r.refresh_token, "r");
        assert_eq!(r.user.id, 1);
        assert_eq!(r.user.username, "alice");
    }

    #[test]
    fn refresh_response_deserializes_camel_case() {
        let json = r#"{"accessToken":"na","refreshToken":"nr","expiresIn":86400}"#;
        let r: RefreshResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.access_token, "na");
        assert_eq!(r.refresh_token, "nr");
    }

    #[test]
    fn audio_request_uses_supported_default_wav_specification() {
        let request = ChatMessageWithAudioRequest::with_default_audio("你好");
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "text": "你好",
                "format": "wav",
                "sampleRate": 24000,
                "channels": 1,
                "sampleBits": 16,
            })
        );
    }

    #[test]
    fn chat_turn_with_audio_response_deserializes_completed() {
        let json = r#"{"conversationId":7,"reply":"hi","toolCalls":[],"audio":{"audioUrl":"https://example.com/audio","format":"wav","sampleRate":24000,"channels":1,"sampleBits":16}}"#;
        let turn: ChatTurnWithAudioResponse = serde_json::from_str(json).unwrap();
        match turn {
            ChatTurnWithAudioResponse::Completed(response) => {
                assert_eq!(response.conversation_id, 7);
                assert_eq!(response.audio.format, "wav");
                assert_eq!(response.audio.sample_rate, 24_000);
            }
            ChatTurnWithAudioResponse::Suspended(_) => {
                panic!("completed JSON 不得解析为 Suspended")
            }
        }
    }

    #[test]
    fn chat_turn_with_audio_response_deserializes_suspended() {
        let json = r#"{
            "status": "suspended",
            "conversation_id": 9,
            "checkpoint_id": "2bb282b3-f4ad-41a6-bf1b-bf5c51fdc760",
            "run_id": "90b4891f-cf68-4c1a-ad83-32d9d8494d18",
            "reason": "approval",
            "approval": {"approval_id": "a", "prompt": "确认", "tool_calls": []}
        }"#;
        assert!(matches!(
            serde_json::from_str::<ChatTurnWithAudioResponse>(json).unwrap(),
            ChatTurnWithAudioResponse::Suspended(_)
        ));
    }

    #[test]
    fn chat_message_response_defaults_empty_tool_calls() {
        // 后端可能省略空 tool_calls 字段
        let json = r#"{"conversation_id":7,"reply":"hi"}"#;
        let r: ChatMessageResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.reply, "hi");
        assert!(r.tool_calls.is_empty());
    }

    #[test]
    fn chat_turn_response_deserializes_completed() {
        let json = r#"{"conversation_id":7,"reply":"hi","tool_calls":[{"name":"clock","arguments":{"zone":"Asia/Shanghai"}}]}"#;
        let turn: ChatTurnResponse = serde_json::from_str(json).unwrap();
        match turn {
            ChatTurnResponse::Completed(resp) => {
                assert_eq!(resp.conversation_id, 7);
                assert_eq!(resp.reply, "hi");
                assert_eq!(resp.tool_calls.len(), 1);
            }
            ChatTurnResponse::Suspended(_) => panic!("completed JSON 不得解析为 Suspended"),
        }
    }

    #[test]
    fn chat_turn_response_deserializes_suspended() {
        let json = r#"{
            "status": "suspended",
            "conversation_id": 9,
            "checkpoint_id": "2bb282b3-f4ad-41a6-bf1b-bf5c51fdc760",
            "run_id": "90b4891f-cf68-4c1a-ad83-32d9d8494d18",
            "reason": "approval",
            "approval": {
                "approval_id": "02f941ab-0fb8-4c44-999c-9ff896ef415a",
                "prompt": "模型请求执行受控工具，请确认是否允许。",
                "tool_calls": [
                    {"id": "call-1", "name": "fetch_web_content", "arguments": {"url": "https://example.com"}}
                ]
            }
        }"#;
        let turn: ChatTurnResponse = serde_json::from_str(json).unwrap();
        match turn {
            ChatTurnResponse::Suspended(resp) => {
                assert_eq!(resp.status, "suspended");
                assert_eq!(resp.conversation_id, 9);
                assert_eq!(resp.checkpoint_id, "2bb282b3-f4ad-41a6-bf1b-bf5c51fdc760");
                assert_eq!(resp.approval.tool_calls.len(), 1);
                assert_eq!(resp.approval.tool_calls[0].name, "fetch_web_content");
            }
            ChatTurnResponse::Completed(_) => panic!("suspended JSON 不得解析为 Completed"),
        }
    }

    #[test]
    fn pending_approvals_response_deserializes_items() {
        let json = r#"{
            "items": [{
                "status": "pending",
                "checkpoint_id": "2bb282b3-f4ad-41a6-bf1b-bf5c51fdc760",
                "run_id": "90b4891f-cf68-4c1a-ad83-32d9d8494d18",
                "conversation_id": 9,
                "reason": "approval",
                "created_at": "2026-07-22T01:00:00+00:00",
                "expires_at": "2026-07-23T01:00:00+00:00",
                "approval": {
                    "approval_id": "02f941ab-0fb8-4c44-999c-9ff896ef415a",
                    "prompt": "模型请求执行受控工具，请确认是否允许。",
                    "tool_calls": [
                        {"id": "call-1", "name": "fetch_web_content", "arguments": {"url": "https://example.com"}}
                    ]
                }
            }]
        }"#;
        let resp: PendingApprovalsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.items.len(), 1);
        let item = &resp.items[0];
        assert_eq!(item.status, "pending");
        assert_eq!(item.expires_at, "2026-07-23T01:00:00+00:00");
        assert_eq!(
            item.approval.approval_id,
            "02f941ab-0fb8-4c44-999c-9ff896ef415a"
        );
    }

    #[test]
    fn resume_request_serializes_decision() {
        let req = ChatResumeRequest {
            approval_id: "a".into(),
            decision: "approve",
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"approval_id": "a", "decision": "approve"})
        );
    }

    #[test]
    fn persona_summary_defaults_all_zero_when_missing() {
        let json = r#"{"has_active_persona":false,"generated_at":null,"snapshot_summary":{},"personalization_enabled":false}"#;
        let r: ChatPersonaResponse = serde_json::from_str(json).unwrap();
        assert!(!r.has_active_persona);
        assert_eq!(r.snapshot_summary.stable_facts_count, 0);
    }

    #[test]
    fn user_profile_handles_null_fields() {
        let json = r#"{"id":1,"user_id":1,"interests":null,"personality_traits":null,"interaction_preferences":null,"emotional_tendency":null,"learning_records":null,"created_at":"t","updated_at":"t"}"#;
        let r: UserProfileResponse = serde_json::from_str(json).unwrap();
        assert!(r.interests.is_none());
    }

    #[test]
    fn memory_item_with_optional_reinforced_at() {
        let json = r#"{"memory_id":7,"memory_type":"fact","content":"x","confidence":0.9,"reinforce_count":2,"created_at":"t"}"#;
        let r: ChatMemoryItem = serde_json::from_str(json).unwrap();
        assert!(r.reinforced_at.is_none());
    }
}
